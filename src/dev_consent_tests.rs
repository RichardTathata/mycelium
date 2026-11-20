//! `mycelium dev consent`'s own rules (identity-and-lifecycle I7, PR 3a), without a fleet: where the key is kept,
//! which certificate it signs under, what a prompt shows, what counts as an answer, and the queue's bound. The walk
//! against a node — a decision typed, signed, verified and recorded — is `agent/dev_consent_tests.rs`; the terminal
//! itself is `tests/dev_consent_cli.rs`.

use super::*;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mycelium-dev-consent-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// **With the provisional gate closed, `dev consent` refuses by name before it touches anything** (a shipped build):
/// the directory is not resolved, no store is opened, no certificate is checked, and nothing is created. In a build
/// without `provisional-consent-grade` the gate is closed as built; with it, this thread closes it.
#[test]
fn with_the_provisional_gate_closed_dev_consent_refuses_by_name_before_touching_anything() {
    let root = scratch("gate");
    let home = root.join("home");
    let closed = || {
        let var = |name: &str| (name == "HOME").then(|| home.display().to_string());
        assert_eq!(consent_dir(&var).expect_err("no directory is resolved").code(), "consent_grade_oid_provisional");
        assert_eq!(Store::open(&var, "fleet", "alex").expect_err("no store").code(), "consent_grade_oid_provisional");
        assert_eq!(Store::at(home.join("consent"), "fleet", "alex").expect_err("no store").code(), "consent_grade_oid_provisional");
        assert!(!home.exists(), "nothing was created");
    };
    #[cfg(feature = "provisional-consent-grade")]
    mycelium_core::tls::with_provisional_consent_grade_closed(closed);
    #[cfg(not(feature = "provisional-consent-grade"))]
    closed();
    let _ = std::fs::remove_dir_all(&root);
}

/// **Only `approve` and `refuse`, typed whole, are an answer.** Nothing shorter, no default, no other case: an empty
/// line, `y`, `yes`, a prefix, another word and a longer line each sign nothing.
#[test]
fn only_approve_and_refuse_typed_whole_are_an_answer() {
    for line in ["approve", "approve\n", "  approve \r\n"] {
        assert_eq!(Approval::of_line(line), Approval::Approve, "{line:?}");
    }
    assert_eq!(Approval::of_line("refuse\n"), Approval::Refuse);
    for line in ["", "\n", "y", "yes", "Y", "a", "appr", "Approve", "APPROVE", "approve all", "approved", "ok", "refuse?", "approve\napprove"] {
        assert_eq!(Approval::of_line(line), Approval::Dismissed, "{line:?} is no answer");
    }
}

/// **The queue answers `busy` past its bound** (I7, exit 9's queue): `bound` ids wait; the next is refused at once
/// and is not in the queue afterwards; a slot freed is a slot offered again.
#[test]
fn the_queue_answers_busy_past_its_bound() {
    let (handle, mut waiting) = queue(3);
    for id in ["r1", "r2", "r3"] {
        assert_eq!(handle.offer(id), Ok(()), "{id} waits");
    }
    assert_eq!(handle.offer("r4"), Err(Busy), "the fourth is answered busy");
    assert_eq!(handle.offer("r5"), Err(Busy));
    assert_eq!(waiting.0.try_recv().ok().as_deref(), Some("r1"), "first in, first out");
    assert_eq!(handle.offer("r6"), Ok(()), "a freed slot is offered again");
    assert_eq!(handle.offer("r7"), Err(Busy));
    drop(handle);
    let rest: Vec<String> = std::iter::from_fn(|| waiting.0.try_recv().ok()).collect();
    assert_eq!(rest, ["r2", "r3", "r6"], "a busy id was never queued");
    assert_eq!(Answer::Busy.line("r4"), serde_json::json!({"schema": SCHEMA, "request": "r4", "answer": "busy"}));
}

fn row(request: &str) -> serde_json::Value {
    let arguments = serde_json::json!({"city": "Lyon"});
    let digest: String = crate::arguments_digest(&arguments).iter().map(|b| format!("{b:02x}")).collect();
    serde_json::json!({
        "schema": "mycelium.grant-request/1", "request": request, "agent": "token:fleet/forecaster", "node": "127.0.0.1:7946",
        "tool": "weather.forecast", "operation": "tools/call", "resource": "tool:weather.forecast@127.0.0.1:7946",
        "arguments_digest": digest, "arguments": arguments, "requested_at_ms": 1_000, "expires_at_ms": 61_000,
        "key": format!("grant/746f6b656e/{request}"),
    })
}

/// **A prompt is rendered from the record alone, and a request with no usable record has no prompt** (I7, exit 9).
/// The id selects a record among the gateway's open requests; an unknown id, an id two holders share, and a record
/// lacking a field the prompt shows each give no record. The prompt names the request, the holder, the tool, the
/// resource and the arguments' digest as the record states them. What a hostile record can and cannot do to the
/// prompt — the digest check, the order, the bounds, the escapes — is `tests/dev_consent_cli.rs`'s.
#[test]
fn a_prompt_is_rendered_from_the_record_alone() {
    let pending = serde_json::json!([row("r1"), row("r2"), row("dup"), row("dup")]);
    assert_eq!(PromptRecord::find(&pending, "nope"), Err(NoRecord::Unknown));
    assert_eq!(PromptRecord::find(&serde_json::Value::Null, "r1"), Err(NoRecord::Unknown), "a gateway that listed nothing");
    assert_eq!(PromptRecord::find(&pending, "dup"), Err(NoRecord::Ambiguous(2)), "dev does not choose between two holders");
    // A caller's text is not an id: nothing matches it, so nothing of it is shown.
    assert_eq!(PromptRecord::find(&pending, "r1\nAPPROVED: tool harmless"), Err(NoRecord::Unknown));
    for field in ["agent", "tool", "resource", "node", "arguments_digest", "key", "expires_at_ms"] {
        let mut lacking = row("r1");
        lacking.as_object_mut().unwrap().remove(field);
        assert_eq!(PromptRecord::find(&serde_json::json!([lacking]), "r1"), Err(NoRecord::Unreadable(field)), "without {field}");
    }
    let mut elsewhere = row("r1");
    elsewhere["key"] = "grant/746f6b656e/another".into();
    assert_eq!(PromptRecord::find(&serde_json::json!([elsewhere]), "r1"), Err(NoRecord::Unreadable("key")), "a key naming another request");

    let record = PromptRecord::find(&pending, "r1").expect("the record");
    assert_eq!((record.holder.as_str(), record.tool.as_str(), record.holder_segment()), ("token:fleet/forecaster", "weather.forecast", "746f6b656e"));
    let prompt = Prompt::render(&record, "consent:fleet/alex", ConsentGrade::Tty, "http://127.0.0.1:8300", "127.0.0.1:7946", 300, 1_000);
    for said in ["r1", "token:fleet/forecaster", "weather.forecast", "tool:weather.forecast@127.0.0.1:7946", record.arguments_digest.as_str(), r#"{"city":"Lyon"}"#, "consent:fleet/alex", "grade tty", "in 60 s", "for 300 s", "a software key in a file"] {
        assert!(prompt.text.contains(said), "the prompt names {said:?}:\n{}", prompt.text);
    }
    assert!(!prompt.text.contains("os_presence"), "a software key's prompt claims no presence grade");
}

#[cfg(all(unix, feature = "provisional-consent-grade"))]
mod with_a_key {
    use super::*;

    /// **The consent directory is under the user's home, never relative, and never the directory `dev` runs in**
    /// (I7, exit 10). `HOME` (and, off macOS, an absolute `XDG_DATA_HOME`) names it; an unset or relative `HOME` is
    /// refused by name rather than resolved against the current directory.
    #[test]
    fn the_consent_directory_is_under_the_users_home_and_never_relative() {
        let env = |pairs: &'static [(&'static str, &'static str)]| move |name: &str| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string());
        let dir = consent_dir(&env(&[("HOME", "/home/r")])).expect("resolved");
        if cfg!(target_os = "macos") {
            assert_eq!(dir, Path::new("/home/r/Library/Application Support/mycelium/consent"));
            assert_eq!(consent_dir(&env(&[("HOME", "/home/r"), ("XDG_DATA_HOME", "/data")])).unwrap(), dir, "macOS keeps the key under Application Support");
        } else {
            assert_eq!(dir, Path::new("/home/r/.local/share/mycelium/consent"));
            assert_eq!(consent_dir(&env(&[("HOME", "/home/r"), ("XDG_DATA_HOME", "/data")])).unwrap(), Path::new("/data/mycelium/consent"));
            assert_eq!(consent_dir(&env(&[("HOME", "/home/r"), ("XDG_DATA_HOME", "data")])).unwrap(), dir, "a relative XDG_DATA_HOME is not a place");
        }
        for unset in [&[][..], &[("HOME", "")], &[("HOME", ".")], &[("HOME", "home/r")], &[("XDG_DATA_HOME", "relative")]] {
            let pairs: &'static [(&'static str, &'static str)] = Box::leak(unset.to_vec().into_boxed_slice());
            let refused = consent_dir(&env(pairs)).expect_err("no home, no directory");
            assert_eq!(refused.code(), "consent_dir_unresolved", "{pairs:?}: {refused}");
        }
    }

    /// **The key is never kept in a worktree** (I7, exit 10: "the key off the worktree"). A consent directory under a
    /// directory holding `.git` — a repository, a linked worktree whose `.git` is a file, however far above — is
    /// refused by name, on every open, and nothing is created there. Outside one, the key is created `0600` in a
    /// `0700` directory with its public half beside it.
    #[test]
    fn a_consent_directory_inside_a_worktree_is_refused_and_nothing_is_created() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = scratch("worktree");
        let (repo, linked, plain) = (root.join("repo"), root.join("linked"), root.join("plain"));
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(&linked).unwrap();
        std::fs::write(linked.join(".git"), "gitdir: /elsewhere/.git/worktrees/linked\n").unwrap();
        for home in [repo.clone(), repo.join("deep").join("er"), linked.join("home")] {
            let var = |name: &str| (name == "HOME").then(|| home.display().to_string());
            let refused = Store::open(&var, "fleet", "alex").expect_err("a worktree");
            assert_eq!(refused.code(), "consent_dir_in_worktree", "{}: {refused}", home.display());
            assert!(refused.to_string().contains(".git"), "{refused}");
        }
        assert_eq!(std::fs::read_dir(&repo).unwrap().count(), 1, "nothing was created in the repository beside .git");
        assert_eq!(std::fs::read_dir(&linked).unwrap().count(), 1);

        let var = |name: &str| (name == "HOME").then(|| plain.display().to_string());
        let store = Store::open(&var, "fleet", "alex").expect("outside a worktree");
        let key = store.create_key(false).expect("created");
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o7777;
        assert_eq!((mode(&store.key_path()), mode(store.key_path().parent().unwrap())), (0o600, 0o700));
        assert!(store.key_path().starts_with(std::fs::canonicalize(&plain).unwrap()) && store.key_path().file_name().unwrap().to_string_lossy().starts_with("consent+fleet+alex+") && store.key_path().to_string_lossy().ends_with(".key.p8"));
        let public = String::from_utf8(std::fs::read(store.public_key_path()).unwrap()).unwrap();
        assert_eq!(mycelium_core::tls::p256_point_from_spki_pem(&public).unwrap(), key.point(), "the public half beside it is the key's");
        assert!(!public.contains("PRIVATE"), "and holds nothing private");
        // The directory becomes a worktree afterwards (a `git init` in the home): refused from then on.
        std::fs::create_dir_all(plain.join(".git")).unwrap();
        assert_eq!(Store::open(&var, "fleet", "alex").expect_err("now a worktree").code(), "consent_dir_in_worktree");
        assert_eq!(Store::open(&|_| Some("/home/r".to_string()), "fleet", "al/ex").expect_err("a name with a slash").code(), "bad_name");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **With the provisional gate closed no certificate is checked and nothing is signed**, by name — the entries a
    /// closed directory lookup does not already stop: `Enrolment::check` on a key and a certificate in hand, and
    /// `Enrolment::sign` on an enrolment made while the gate was open.
    #[test]
    fn with_the_gate_closed_no_certificate_is_checked_and_nothing_is_signed() {
        let root = scratch("gate-enrol");
        let (ca, out) = (root.join("ca"), root.join("out"));
        crate::mint_ca(&ca).expect("a CA");
        let store = Store::at(root.join("home").join("consent"), "fleet", "alex").unwrap();
        let key = store.create_key(false).unwrap();
        let pem = std::fs::read_to_string(crate::issue_consent_cert(&ca, "fleet", "alex", &key.spki_pem(), ConsentGrade::Tty, &out).unwrap()).unwrap();
        let enrolment = store.enrol(&pem).expect("enrolled while the gate is open");
        let record = PromptRecord::find(&serde_json::json!([row("r1")]), "r1").unwrap();
        enrolment.sign("n", &record, "issue", Some(60), None, 1).expect("and it signs");
        mycelium_core::tls::with_provisional_consent_grade_closed(|| {
            assert_eq!(Enrolment::check(key, "consent:fleet/alex", &pem).expect_err("no certificate is checked").code(), "consent_grade_oid_provisional");
            assert_eq!(enrolment.sign("n", &record, "issue", Some(60), None, 1).expect_err("nothing is signed").code(), "consent_grade_oid_provisional");
            assert_eq!(store.enrolment().expect_err("no enrolment is read").code(), "consent_grade_oid_provisional");
            assert_eq!(store.enrol(&pem).expect_err("no certificate is kept").code(), "consent_grade_oid_provisional");
        });
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **`dev` signs only under a certificate that names its key, its principal and `tty`** (the promise lens: the
    /// device says how its key is guarded and claims nothing more; I7 exit 3's "the key on use", for the software
    /// key). A software key certified `os_presence(macos, unbound)` or `recording` is refused by name at enrolment
    /// and on use — `dev` will not sign under a certificate that says the key is something it is not; so is a
    /// certificate over another key, a certificate for another principal, and a service's client certificate. A
    /// refused certificate is not stored. After the key is replaced, the old certificate no longer names it: refused
    /// on use, before anything is asked.
    #[test]
    fn dev_signs_only_under_a_certificate_that_names_its_key_its_principal_and_tty() {
        let root = scratch("enrol");
        let (ca, out) = (root.join("ca"), root.join("out"));
        crate::mint_ca(&ca).expect("a CA");
        let store = Store::at(root.join("home").join("consent"), "fleet", "alex").unwrap();
        assert_eq!(store.enrolment().expect_err("no key yet").code(), "no_consent_key");
        let key = store.create_key(false).unwrap();
        assert_eq!(store.enrolment().expect_err("a key, no certificate").code(), "not_enrolled");
        let issue = |spki: &str, name: &str, grade: ConsentGrade, tag: &str| {
            let cert = crate::issue_consent_cert(&ca, "fleet", name, spki, grade, &out.join(tag)).expect("issued");
            std::fs::read_to_string(cert).unwrap()
        };
        let other = SoftwareConsentKey::generate().unwrap();
        let service = { let (cert, _) = crate::issue_client_cert(&ca, "fleet", "alex", &out.join("service")).unwrap(); std::fs::read_to_string(cert).unwrap() };
        let tty = issue(&key.spki_pem(), "alex", ConsentGrade::Tty, "tty");
        let mut wrong: Vec<String> = Vec::new();
        for (what, pem, code) in [
            ("certified as an OS-presence key", issue(&key.spki_pem(), "alex", ConsentGrade::OsPresenceMacosUnbound, "os"), "grade_not_this_keys"),
            ("certified as a recording key", issue(&key.spki_pem(), "alex", ConsentGrade::Recording, "rec"), "grade_not_this_keys"),
            ("issued over another key", issue(&other.spki_pem(), "alex", ConsentGrade::Tty, "other"), "enrolled_key_mismatch"),
            ("another person's", issue(&key.spki_pem(), "dana", ConsentGrade::Tty, "dana"), "certificate_for_another_principal"),
            ("a service's client certificate", service, "not_a_consent_certificate"),
            ("two certificates", format!("{tty}{tty}"), "not_a_consent_certificate"),
            ("not a certificate", "hello".to_string(), "not_a_consent_certificate"),
        ] {
            // Every wrong case is collected, so one failing run names each of them.
            match store.enrol(&pem) {
                Err(refused) if refused.code() == code => {}
                Err(refused) => wrong.push(format!("{what}: refused {}, not {code}", refused.code())),
                Ok(_) => wrong.push(format!("{what}: ENROLLED, not refused {code}")),
            }
            if store.cert_path().exists() {
                wrong.push(format!("{what}: the certificate was stored"));
                std::fs::remove_file(store.cert_path()).unwrap();
            }
        }
        assert!(wrong.is_empty(), "ENROL: {wrong:#?}");
        let said = store.enrol(&issue(&key.spki_pem(), "alex", ConsentGrade::OsPresenceMacosUnbound, "os2")).unwrap_err().to_string();
        assert!(said.contains("os_presence(macos, unbound)") && said.contains("--grade tty") && said.contains("software key"), "the refusal says what the key is: {said}");

        let enrolled = store.enrol(&tty).expect("its own key, its own principal, tty");
        assert_eq!((enrolled.principal(), enrolled.grade()), ("consent:fleet/alex", ConsentGrade::Tty));
        assert_eq!(store.enrolment().expect("checked again on use").cert_pem(), tty);

        // A certificate swapped in beside the key afterwards is caught on use, whatever wrote it.
        for (what, pem, code) in [
            ("an OS-presence certificate written beside the key", issue(&key.spki_pem(), "alex", ConsentGrade::OsPresenceMacosUnbound, "os3"), "grade_not_this_keys"),
            ("another key's certificate written beside the key", issue(&other.spki_pem(), "alex", ConsentGrade::Tty, "other2"), "enrolled_key_mismatch"),
        ] {
            consent_key::store_public(&store.cert_path(), pem.as_bytes()).unwrap();
            match store.enrolment() {
                Err(refused) if refused.code() == code => {}
                other => wrong.push(format!("{what}: on use, {:?}, not refused {code}", other.map(|e| e.principal().to_string()).map_err(|r| r.code()))),
            }
        }
        assert!(wrong.is_empty(), "ON USE: {wrong:#?}");
        consent_key::store_public(&store.cert_path(), tty.as_bytes()).unwrap();
        store.enrolment().expect("the right certificate again");
        // The key replaced: the certificate names the old one.
        assert_eq!(store.create_key(false).expect_err("not replaced unasked").code(), "consent_key_exists");
        let replaced = store.create_key(true).expect("replaced when asked");
        assert_ne!(replaced.point(), key.point());
        assert_eq!(store.enrolment().expect_err("the certificate names the old key").code(), "enrolled_key_mismatch");
        let _ = std::fs::remove_dir_all(&root);
    }
}
