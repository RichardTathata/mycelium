//! `GitStore` contract tests — the `FsStore` contract suite (`src/fs/tests.rs`) mirrored onto the
//! git-as-truth store, plus git-specific properties (history retention, scoped commits, the
//! two-instance ref-CAS race, the unborn branch). Two deliberate adaptations from the FsStore
//! suite, both consequences of **content-hash version tokens** (equality-only, unordered):
//! - "the committed write advanced the version" becomes `fresh != base` (tokens don't order);
//! - the FsStore GC-gap test has no analogue (there are no version-numbered objects to gap) — the
//!   equivalent guarantee (a two-generations-stale writer conflicts) is asserted directly.
//!
//! Contention constants are smaller than FsStore's: every committed write is a real git commit
//! (~6 subprocesses), so the same properties are proved at lower volume.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use mycelium_wiki::{GitStore, GitStoreConfig, Predicate, Section, SectionId, WikiError, WikiStore};

fn store() -> (tempfile::TempDir, GitStore) {
    let dir = tempfile::tempdir().unwrap();
    let s = GitStore::open(GitStoreConfig {
        dir: dir.path().to_path_buf(),
        subdir: "councils/testville".into(),
        message_prefix: "wiki(testville)".into(),
        ..Default::default()
    })
    .unwrap();
    (dir, s)
}

fn attrs(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn sec(id: &str, heading: &str, body: &str, a: &[(&str, &str)]) -> Section {
    Section { id: Arc::from(id), heading: heading.into(), body: body.into(), attributes: attrs(a) }
}

/// Test-harness git runner (read-side assertions only).
fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

// ── the FsStore contract, mirrored ──────────────────────────────────────────────

#[test]
fn write_then_read_round_trips_in_order_with_attributes() {
    let (_d, s) = store();
    let sa = sec("s-a", "Symptoms", "gateway 503s", &[("node", "e_rl_rk")]);
    let sb = sec("s-b", "Resolution", "rolled cert", &[("node", "e_rl_rk"), ("topic", "resolution")]);
    s.write_page("incidents/cert-rotation", &[sa.clone(), sb.clone()], &attrs(&[("domain", "retail-lending")]))
        .unwrap();

    let page = s.read("incidents/cert-rotation").unwrap().unwrap();
    assert_eq!(page.path, "incidents/cert-rotation");
    assert_eq!(page.attributes.get("domain").map(String::as_str), Some("retail-lending"));
    assert_eq!(page.sections, vec![sa, sb], "sections round-trip in manifest order");
    assert_eq!(s.read("nope").unwrap(), None, "absent page reads as None");
}

#[test]
fn bodies_round_trip_byte_exactly() {
    // The char-exact parser: tricky bodies (trailing newlines, leading blanks, '#' lines, '---'
    // lines, empty) survive the render→commit→parse cycle unchanged.
    let (_d, s) = store();
    let bodies = ["x", "x\n", "", "\nleading blank", "# looks like a heading\n---\nnot front-matter", "a\n\nb\n\n"];
    let sections: Vec<Section> =
        bodies.iter().enumerate().map(|(i, b)| sec(&format!("s{i}"), "H", b, &[])).collect();
    s.write_page("p", &sections, &BTreeMap::new()).unwrap();
    let page = s.read("p").unwrap().unwrap();
    for (i, b) in bodies.iter().enumerate() {
        assert_eq!(&page.sections[i].body, b, "body {i} must round-trip byte-exactly");
    }
}

#[test]
fn reserved_marker_in_a_body_is_refused_not_mangled() {
    let (_d, s) = store();
    let evil = sec("s1", "H", "text\n<!-- mycelium-section {} -->\nmore", &[]);
    let r = s.write_page("p", &[evil], &BTreeMap::new());
    assert!(matches!(r, Err(WikiError::Io(_))), "a body carrying the reserved marker is refused: {r:?}");
    assert_eq!(s.read("p").unwrap(), None, "nothing was written");
}

#[test]
fn read_is_manifest_authoritative_a_stray_section_is_invisible() {
    let (_d, s) = store();
    let a = sec("s-a", "H", "b", &[]);
    s.write_page("p", std::slice::from_ref(&a), &BTreeMap::new()).unwrap();
    // An in-flight orphan: a section written before its membership add commits.
    s.write_section("p", &sec("s-stray", "X", "y", &[]), None).unwrap();

    let page = s.read("p").unwrap().unwrap();
    assert_eq!(page.sections, vec![a], "only the manifest-referenced section is visible");
    // …but read_versioned (the curator's view) does see the orphan.
    let vp = s.read_versioned("p").unwrap().unwrap();
    assert!(vp.sections.contains_key(&SectionId::from("s-stray")), "the curator sees in-flight orphans");
}

#[test]
fn editing_a_page_drops_removed_sections_from_the_read() {
    let (_d, s) = store();
    let a = sec("s-a", "A", "1", &[]);
    let b = sec("s-b", "B", "2", &[]);
    s.write_page("p", &[a.clone(), b.clone()], &BTreeMap::new()).unwrap();
    s.write_page("p", std::slice::from_ref(&a), &BTreeMap::new()).unwrap();
    let page = s.read("p").unwrap().unwrap();
    assert_eq!(page.sections, vec![a], "the dropped section is gone from the manifest → invisible");
}

#[test]
fn query_filters_sections_by_attribute_across_pages() {
    let (_d, s) = store();
    s.write_page("retail-lending/deps",
        &[sec("s1", "feature-data", "Central Data dependency", &[("node", "e_rl_rk"), ("topic", "coupling")])],
        &BTreeMap::new()).unwrap();
    s.write_page("retail-lending/risk",
        &[sec("s2", "sign-off gate", "Risk Lead authorises", &[("node", "risk"), ("topic", "governance")])],
        &BTreeMap::new()).unwrap();
    s.write_page("platform/compute",
        &[sec("s3", "compute", "shared platform", &[("node", "platform"), ("topic", "coupling")])],
        &BTreeMap::new()).unwrap();

    let by_node = s.query(&Predicate::new().with("node", "e_rl_rk")).unwrap();
    assert_eq!(by_node.len(), 1);
    assert_eq!(&*by_node[0].id, "s1");
    assert_eq!(by_node[0].page, "retail-lending/deps");

    let mut by_topic: Vec<String> =
        s.query(&Predicate::new().with("topic", "coupling")).unwrap().into_iter().map(|r| r.id.to_string()).collect();
    by_topic.sort();
    assert_eq!(by_topic, ["s1", "s3"], "tag query spans pages");
    assert_eq!(s.query(&Predicate::new()).unwrap().len(), 3, "empty predicate matches all");
}

#[test]
fn list_pages_returns_every_page_sorted() {
    let (_d, s) = store();
    s.write_page("b/second", &[sec("s1", "H", "x", &[])], &BTreeMap::new()).unwrap();
    s.write_page("a/first", &[sec("s2", "H", "y", &[])], &BTreeMap::new()).unwrap();
    assert_eq!(s.list_pages().unwrap(), vec!["a/first".to_string(), "b/second".to_string()]);
}

#[test]
fn page_path_traversal_is_rejected() {
    let (_d, s) = store();
    assert!(matches!(s.write_page("../escape", &[], &BTreeMap::new()), Err(WikiError::BadPath(_))));
    assert!(matches!(s.read("a/../../etc"), Err(WikiError::BadPath(_))));
    assert!(matches!(s.read("a//b"), Err(WikiError::BadPath(_))), "empty component rejected");
    assert!(matches!(s.read(".git/config"), Err(WikiError::BadPath(_))), ".git component rejected");
}

// ── compare-and-swap: the airtight concurrent-writer contract ───────────────────

#[test]
fn concurrent_curators_editing_different_sections_dont_clobber() {
    // The single-file twist on the FsStore lost-update test: both sections live in ONE file, yet a
    // commit that changed x leaves y's content-hash token valid — per-section independence
    // preserved through content-addressed versions.
    let (_d, s) = store();
    s.write_page("p", &[sec("x", "X", "x0", &[]), sec("y", "Y", "y0", &[])], &BTreeMap::new()).unwrap();

    let vp = s.read_versioned("p").unwrap().unwrap();
    let vx = vp.sections.get(&SectionId::from("x")).unwrap().0;
    let vy = vp.sections.get(&SectionId::from("y")).unwrap().0;

    s.write_section("p", &sec("x", "X", "x1", &[]), Some(vx)).unwrap();
    s.write_section("p", &sec("y", "Y", "y1", &[]), Some(vy)).unwrap(); // vy still valid post-x-commit

    let page = s.read("p").unwrap().unwrap();
    let body = |id: &str| page.sections.iter().find(|s| &*s.id == id).map(|s| s.body.clone());
    assert_eq!(body("x").as_deref(), Some("x1"));
    assert_eq!(body("y").as_deref(), Some("y1"), "the different-section edit was NOT lost");
}

#[test]
fn same_section_stale_write_is_rejected_not_silently_lost() {
    let (_d, s) = store();
    s.write_page("p", &[sec("x", "X", "x0", &[])], &BTreeMap::new()).unwrap();
    let base = s.read_versioned("p").unwrap().unwrap().sections.get(&SectionId::from("x")).unwrap().0;

    s.write_section("p", &sec("x", "X", "first", &[]), Some(base)).unwrap();
    let stale = s.write_section("p", &sec("x", "X", "second", &[]), Some(base));
    assert!(matches!(stale, Err(WikiError::Conflict)), "stale-based write conflicts: {stale:?}");

    assert_eq!(s.read("p").unwrap().unwrap().sections[0].body, "first");
    let fresh = s.read_versioned("p").unwrap().unwrap().sections.get(&SectionId::from("x")).unwrap().0;
    // Content-hash tokens are equality-only (unordered) — the FsStore suite's `fresh > base`
    // becomes inequality here, documented in the module docs.
    assert_ne!(fresh, base, "the committed write changed the token");
    s.write_section("p", &sec("x", "X", "second-retried", &[]), Some(fresh)).unwrap();
    assert_eq!(s.read("p").unwrap().unwrap().sections[0].body, "second-retried");
}

#[test]
fn a_two_generations_stale_writer_still_conflicts() {
    // The GC-gap analogue: however far behind, a writer whose base content is no longer the
    // committed content must Conflict, never shadow.
    let (_d, s) = store();
    s.write_page("p", &[sec("x", "X", "v0", &[])], &BTreeMap::new()).unwrap();
    let stale = s.read_versioned("p").unwrap().unwrap().sections.get(&SectionId::from("x")).unwrap().0;
    let v2 = s.write_section("p", &sec("x", "X", "a", &[]), Some(stale)).unwrap();
    let _v3 = s.write_section("p", &sec("x", "X", "b", &[]), Some(v2)).unwrap();
    let gap = s.write_section("p", &sec("x", "X", "shadow", &[]), Some(stale));
    assert!(matches!(gap, Err(WikiError::Conflict)), "a two-generations-stale write conflicts: {gap:?}");
    assert_eq!(s.read("p").unwrap().unwrap().sections[0].body, "b", "the head is untouched");
}

#[test]
fn creating_a_section_that_already_exists_conflicts() {
    let (_d, s) = store();
    s.write_page("p", &[sec("x", "X", "x0", &[])], &BTreeMap::new()).unwrap();
    let dup = s.write_section("p", &sec("x", "X", "dupe", &[]), None);
    assert!(matches!(dup, Err(WikiError::Conflict)), "create-over-existing conflicts: {dup:?}");
    assert_eq!(s.read("p").unwrap().unwrap().sections[0].body, "x0");
}

#[test]
fn manifest_membership_is_compare_and_swap() {
    let (_d, s) = store();
    s.write_page("p", &[sec("x", "X", "x0", &[])], &BTreeMap::new()).unwrap();
    let vp = s.read_versioned("p").unwrap().unwrap();
    let mver = vp.manifest_version;
    let mut order = vp.order.clone();

    s.write_section("p", &sec("y", "Y", "y0", &[]), None).unwrap();
    let mut order_a = order.clone();
    order_a.push(SectionId::from("y"));
    s.update_manifest("p", &order_a, &vp.attributes, Some(mver)).unwrap();

    s.write_section("p", &sec("z", "Z", "z0", &[]), None).unwrap();
    order.push(SectionId::from("z"));
    let stale = s.update_manifest("p", &order, &vp.attributes, Some(mver));
    assert!(matches!(stale, Err(WikiError::Conflict)), "stale manifest add conflicts: {stale:?}");

    let vp2 = s.read_versioned("p").unwrap().unwrap();
    let mut order2 = vp2.order.clone();
    order2.push(SectionId::from("z"));
    s.update_manifest("p", &order2, &vp2.attributes, Some(vp2.manifest_version)).unwrap();

    let ids: Vec<String> = s.read("p").unwrap().unwrap().sections.iter().map(|s| s.id.to_string()).collect();
    assert_eq!(ids, ["x", "y", "z"], "both concurrent membership adds survived");
}

#[test]
fn concurrent_idempotent_appends_deliver_every_edit_exactly_once() {
    // The FsStore airtightness proof at reduced volume (every committed write is a real git
    // commit): threads race the SAME section via read → idempotent merge → CAS → retry. Every
    // edit lands exactly once.
    let dir = tempfile::tempdir().unwrap();
    let s = Arc::new(
        GitStore::open(GitStoreConfig { dir: dir.path().to_path_buf(), ..Default::default() }).unwrap(),
    );
    s.write_page("p", &[sec("c", "Log", "", &[])], &BTreeMap::new()).unwrap();

    const THREADS: usize = 4;
    const LINES: usize = 6;
    std::thread::scope(|scope| {
        for t in 0..THREADS {
            let s = Arc::clone(&s);
            scope.spawn(move || {
                for l in 0..LINES {
                    let line = format!("t{t}-l{l}");
                    loop {
                        let vp = s.read_versioned("p").unwrap().unwrap();
                        let (ver, cur) = vp.sections.get(&SectionId::from("c")).cloned().unwrap();
                        if cur.body.lines().any(|x| x == line) {
                            break;
                        }
                        let mut body = cur.body.clone();
                        if !body.is_empty() {
                            body.push('\n');
                        }
                        body.push_str(&line);
                        match s.write_section("p", &sec("c", "Log", &body, &[]), Some(ver)) {
                            Ok(_) => break,
                            Err(WikiError::Conflict) => continue,
                            Err(e) => panic!("unexpected store error: {e:?}"),
                        }
                    }
                }
            });
        }
    });

    let body = s.read("p").unwrap().unwrap().sections[0].body.clone();
    let mut lines: Vec<&str> = body.lines().collect();
    let total = lines.len();
    lines.sort_unstable();
    lines.dedup();
    assert_eq!(lines.len(), THREADS * LINES, "every edit landed — none lost");
    assert_eq!(total, THREADS * LINES, "no edit duplicated — idempotent merge deduped the retries");
}

#[test]
fn two_store_instances_race_the_ref_cas_not_each_other() {
    // The cross-instance backstop: two GitStore instances on ONE checkout (each with its own
    // internal mutex) race the same section from the same base — `update-ref <new> <old>` must
    // elect exactly one; the loser re-reads, sees moved content, and Conflicts.
    let dir = tempfile::tempdir().unwrap();
    let mk = || {
        GitStore::open(GitStoreConfig { dir: dir.path().to_path_buf(), ..Default::default() }).unwrap()
    };
    let a = mk();
    let b = mk();
    a.write_page("p", &[sec("x", "X", "x0", &[])], &BTreeMap::new()).unwrap();
    let base = b.read_versioned("p").unwrap().unwrap().sections.get(&SectionId::from("x")).unwrap().0;

    let outcomes: Vec<Result<u64, WikiError>> = std::thread::scope(|scope| {
        let ha = scope.spawn(|| a.write_section("p", &sec("x", "X", "from-a", &[]), Some(base)));
        let hb = scope.spawn(|| b.write_section("p", &sec("x", "X", "from-b", &[]), Some(base)));
        vec![ha.join().unwrap(), hb.join().unwrap()]
    });
    let oks = outcomes.iter().filter(|r| r.is_ok()).count();
    let conflicts = outcomes.iter().filter(|r| matches!(r, Err(WikiError::Conflict))).count();
    assert_eq!((oks, conflicts), (1, 1), "exactly one instance won, the other Conflicted: {outcomes:?}");
    let body = a.read("p").unwrap().unwrap().sections[0].body.clone();
    assert!(body == "from-a" || body == "from-b", "survivor is one clean value: {body:?}");
}

// ── git-specific properties ─────────────────────────────────────────────────────

#[test]
fn history_is_retained_and_the_prior_text_is_recoverable() {
    // The property the whole design exists for: git show HEAD~1 recovers the pre-edit document.
    let (d, s) = store();
    s.write_page("minutes/2026-08", &[sec("s1", "Opening", "The meeting opened.", &[])], &BTreeMap::new())
        .unwrap();
    let v = s.read_versioned("minutes/2026-08").unwrap().unwrap();
    let ver = v.sections.get(&SectionId::from("s1")).unwrap().0;
    s.write_section("minutes/2026-08", &sec("s1", "Opening", "Amended text.", &[]), Some(ver)).unwrap();

    let old = git(d.path(), &["show", "HEAD~1:councils/testville/minutes/2026-08.md"]);
    assert!(old.contains("The meeting opened."), "history retains the pre-edit text");
    let new = git(d.path(), &["show", "HEAD:councils/testville/minutes/2026-08.md"]);
    assert!(new.contains("Amended text."));
    assert!(!new.contains("The meeting opened."));
}

#[test]
fn commits_are_scoped_and_carry_the_message_prefix() {
    // The scoped-commit discipline (councils/<slug>, never -A): each commit touches exactly the
    // written path, and messages carry the configured prefix.
    let (d, s) = store();
    s.write_page("a", &[sec("s1", "H", "x", &[])], &BTreeMap::new()).unwrap();
    s.write_page("b", &[sec("s2", "H", "y", &[])], &BTreeMap::new()).unwrap();

    let files = git(d.path(), &["show", "--name-only", "--format=", "HEAD"]);
    assert_eq!(files.trim(), "councils/testville/b.md", "the commit touches exactly the written path");
    let msg = git(d.path(), &["log", "-1", "--format=%s"]);
    assert_eq!(msg.trim(), "wiki(testville): replace b");
}

#[test]
fn an_idempotent_rewrite_makes_no_empty_commit() {
    let (d, s) = store();
    let a = sec("s1", "H", "x", &[]);
    s.write_page("p", std::slice::from_ref(&a), &BTreeMap::new()).unwrap();
    s.write_page("p", std::slice::from_ref(&a), &BTreeMap::new()).unwrap(); // byte-identical re-apply
    let count = git(d.path(), &["rev-list", "--count", "HEAD"]);
    assert_eq!(count.trim(), "1", "a no-op rewrite records nothing");
}

#[test]
fn the_unborn_branch_is_an_empty_store() {
    let (_d, s) = store();
    assert_eq!(s.read("p").unwrap(), None);
    assert_eq!(s.list_pages().unwrap(), Vec::<String>::new());
    assert_eq!(s.query(&Predicate::new()).unwrap(), vec![]);
    // …and the first write creates the root commit.
    s.write_page("p", &[sec("s1", "H", "x", &[])], &BTreeMap::new()).unwrap();
    assert_eq!(s.read("p").unwrap().unwrap().sections.len(), 1);
}

#[test]
fn the_worktree_mirrors_head_for_written_pages() {
    // Humans and external validators (the council-wiki Node validator) read files; the store syncs
    // each written page into the worktree, byte-equal to the committed blob.
    let (d, s) = store();
    s.write_page("m/note", &[sec("s1", "Heading", "Visible body.", &[])], &attrs(&[("k", "v")])).unwrap();
    let on_disk = std::fs::read_to_string(d.path().join("councils/testville/m/note.md")).unwrap();
    let committed = git(d.path(), &["show", "HEAD:councils/testville/m/note.md"]);
    assert_eq!(on_disk, committed, "worktree file equals the committed blob");
    assert!(on_disk.contains("# Heading"), "the document is real markdown: {on_disk}");
    assert!(!on_disk.contains("\"v\":"), "no version tokens in the document");
}

// ── the write gate (council-substrate Phase 3) ──────────────────────────────────

/// A stub validator: refuses any candidate containing "FORBIDDEN", with a finding on stdout —
/// the contract stand-in for the council-wiki Node validator.
fn gate_script(dir: &Path) -> Vec<String> {
    let script = dir.join("stub-validate.sh");
    // A file-LIST validator (P6.1: the gate receives the whole batch's paths as argv).
    std::fs::write(&script, "#!/bin/sh\nfor f in \"$@\"; do\n  if grep -q FORBIDDEN \"$f\"; then echo \"GATE001/forbidden-term: $f\"; exit 1; fi\ndone\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    vec!["/bin/sh".into(), script.to_string_lossy().into_owned()]
}

#[test]
fn the_write_gate_refuses_before_commit_and_leaves_no_residue() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let s = GitStore::open(GitStoreConfig {
        dir: repo.clone(),
        subdir: "councils/testville".into(),
        validate_cmd: Some(gate_script(tmp.path())),
        ..Default::default()
    })
    .unwrap();

    // A clean write passes the gate and commits.
    s.write_page("p", &[sec("s1", "H", "wholesome text", &[])], &BTreeMap::new()).unwrap();
    assert_eq!(git(&repo, &["rev-list", "--count", "HEAD"]).trim(), "1");

    // A forbidden write is refused: the findings surface, no commit lands, the worktree is restored.
    let vp = s.read_versioned("p").unwrap().unwrap();
    let ver = vp.sections.get(&SectionId::from("s1")).unwrap().0;
    let refused = s.write_section("p", &sec("s1", "H", "FORBIDDEN text", &[]), Some(ver));
    let findings = match &refused {
        Err(e) => e.as_gate_refusal().map(str::to_owned),
        Ok(_)  => None,
    };
    assert!(findings.as_deref().is_some_and(|f| f.contains("GATE001/forbidden-term")),
        "the refusal carries the validator's finding: {refused:?}");
    assert_eq!(git(&repo, &["rev-list", "--count", "HEAD"]).trim(), "1", "no commit was created");
    let on_disk = std::fs::read_to_string(repo.join("councils/testville/p.md")).unwrap();
    assert!(on_disk.contains("wholesome text") && !on_disk.contains("FORBIDDEN"),
        "the worktree was restored to the committed state");

    // A refused NEW page leaves no file at all.
    let refused_new = s.write_page("q", &[sec("s2", "H", "also FORBIDDEN", &[])], &BTreeMap::new());
    assert!(matches!(&refused_new, Err(e) if e.as_gate_refusal().is_some()));
    assert!(!repo.join("councils/testville/q.md").exists(), "a refused create leaves no residue");

    // The store keeps working after refusals.
    s.write_section("p", &sec("s1", "H", "amended wholesome text", &[]), Some(ver)).unwrap();
    assert_eq!(s.read("p").unwrap().unwrap().sections[0].body, "amended wholesome text");
}

// ── bulk ingest (council-substrate Phase 4) ─────────────────────────────────────

use mycelium_wiki::{apply_batch, IngestBatch, IngestPage};

fn meeting_batch() -> IngestBatch {
    IngestBatch {
        source: "pipeline/run-42".into(),
        pages: vec![
            IngestPage {
                path: "minutes/2026-08-15/decisions".into(),
                attributes: attrs(&[("meeting", "2026-08-15")]),
                sections: vec![
                    sec("d1", "Retrofit approved", "RESOLVED: the retrofit scheme proceeds.", &[("decision-id", "TV-1")]),
                    sec("d2", "Budget noted", "The budget report was noted.", &[("decision-id", "TV-2")]),
                ],
            },
            IngestPage {
                path: "minutes/2026-08-15/statements".into(),
                attributes: attrs(&[("meeting", "2026-08-15")]),
                sections: vec![sec("s1", "Cllr Reed", "Spoke in support of the scheme.", &[])],
            },
        ],
    }
}

#[test]
fn ingest_is_byte_identical_to_the_serial_writer_and_lands_as_one_commit() {
    // The Phase-4 determinism gate, updated for P6.1: the same meeting applied (a) by a serial
    // per-page writer and (b) via apply_batch produces IDENTICAL git trees — and the batch lands
    // as ONE commit (the deployment's per-meeting boundary commit), while resubmitting records
    // nothing (idempotent recovery after a partial failure).
    let tmp = tempfile::tempdir().unwrap();
    let batch = meeting_batch();

    let serial_repo = tmp.path().join("serial");
    let serial = GitStore::open(GitStoreConfig::for_group(&serial_repo, "testville")).unwrap();
    for page in &batch.pages {
        serial.write_page(&page.path, &page.sections, &page.attributes).unwrap();
    }

    let ingest_repo = tmp.path().join("ingest");
    let ingest = GitStore::open(GitStoreConfig::for_group(&ingest_repo, "testville")).unwrap();
    let summary = apply_batch(&ingest, &batch).unwrap();
    assert_eq!((summary.applied, summary.refused), (2, 0));

    let tree = |repo: &Path| git(repo, &["rev-parse", "HEAD^{tree}"]).trim().to_string();
    assert_eq!(tree(&serial_repo), tree(&ingest_repo), "ingest is byte-identical to the serial writer");
    let count = |repo: &Path| git(repo, &["rev-list", "--count", "HEAD"]).trim().to_string();
    assert_eq!(count(&serial_repo), "2", "the serial per-page writer commits per page");
    assert_eq!(count(&ingest_repo), "1", "P6.1: the batch is ONE commit — per-meeting granularity");
    let msg = git(&ingest_repo, &["log", "-1", "--format=%s"]);
    assert_eq!(msg.trim(), "wiki(testville): batch(pipeline/run-42) — 2 page(s)", "batch provenance");

    // Resubmission: everything already applied → no new commits, tree unchanged.
    let again = apply_batch(&ingest, &batch).unwrap();
    assert_eq!(again.applied, 2, "re-applies report success (no-ops)");
    assert_eq!(count(&ingest_repo), "1", "an idempotent resubmit records nothing");
}

#[test]
fn a_gate_refusal_refuses_the_whole_batch_atomically() {
    // Phase 3 × P6.1 — the recorded semantics change from Phase 4: a gate refusal refuses the
    // WHOLE batch, and nothing commits. The repository only ever holds whole meetings — a batch
    // with one invalid page must not land a partial meeting (the deployment's crash invariant).
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let s = GitStore::open(GitStoreConfig {
        validate_cmd: Some(gate_script(tmp.path())),
        ..GitStoreConfig::for_group(&repo, "testville")
    })
    .unwrap();

    let mut batch = meeting_batch();
    batch.pages.push(IngestPage {
        path: "minutes/2026-08-15/notes".into(),
        attributes: BTreeMap::new(),
        sections: vec![sec("n1", "Note", "FORBIDDEN claim.", &[])],
    });

    let summary = apply_batch(&s, &batch).unwrap();
    assert_eq!((summary.applied, summary.refused), (0, 3), "the whole batch is refused");
    assert!(summary.findings.iter().any(|f| f.contains("GATE001/forbidden-term")),
        "the gate's finding is carried: {:?}", summary.findings);
    assert_eq!(s.list_pages().unwrap(), Vec::<String>::new(), "NOTHING committed — no partial meeting");
    assert_eq!(s.read("minutes/2026-08-15/decisions").unwrap(), None, "clean pages did not land alone");

    // Fixing the batch (dropping the invalid page) applies wholly, as one commit.
    batch.pages.pop();
    let fixed = apply_batch(&s, &batch).unwrap();
    assert_eq!((fixed.applied, fixed.refused), (2, 0));
    assert_eq!(git(&repo, &["rev-list", "--count", "HEAD"]).trim(), "1", "one commit, whole meeting");
}

// ── the read plane at corpus scale (P6.2) ───────────────────────────────────────

#[test]
fn the_read_plane_scales_without_per_page_process_spawns() {
    // P6.2 gate: corpus-scale `list_pages` + `query` must not cost process spawns per page.
    // Pre-P6.2 each page read spawned `rev-parse` + `git show` (~3 spawns/page → tens of seconds
    // over 600 pages); with the persistent `cat-file --batch` child it is one `rev-parse` + one
    // `ls-tree` + pipe round-trips. The bound is generous for CI noise but far below pre-fix cost.
    use mycelium_wiki::PageWrite;
    let (_d, s) = store();
    for b in 0..3 {
        let pages: Vec<PageWrite> = (0..200)
            .map(|i| PageWrite {
                path: format!("m/{b}/{i}"),
                attributes: attrs(&[("meeting", "m")]),
                sections: vec![sec(
                    &format!("s{b}-{i}"),
                    "H",
                    &format!("body {b}-{i}"),
                    &[("topic", if i % 2 == 0 { "even" } else { "odd" })],
                )],
            })
            .collect();
        s.write_pages(&pages, &format!("bulk-{b}")).unwrap();
    }
    let t0 = std::time::Instant::now();
    assert_eq!(s.list_pages().unwrap().len(), 600);
    let hits = s.query(&Predicate::new().with("topic", "even")).unwrap();
    let elapsed = t0.elapsed();
    assert_eq!(hits.len(), 300, "the attribute query is correct at corpus scale");
    eprintln!("P6.2 measurement: list_pages + query over 600 pages in {elapsed:?}");
    assert!(elapsed < std::time::Duration::from_secs(10),
        "corpus-scale reads must not spawn per page: {elapsed:?}");
}
