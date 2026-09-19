//! **Curator handover** — v3 item 5's decisive demonstration (`docs/plans/v3-contracts-axis.md`
//! §12.1).
//!
//! ```text
//! cargo run -p mycelium-wiki --example curator_handover
//! ```
//!
//! # The claim this exists to make checkable
//!
//! *Authority is checked by the protected resource, inside the resource's own atomic boundary —
//! never inferred from a role advertisement, and never checked somewhere earlier and trusted
//! later.*
//!
//! A council's wiki has one curator at a time. The curator's appointment is a git ref
//! (`refs/mycelium/mandate/{council}`); a write is a **single `git update-ref --stdin`
//! transaction** that verifies that ref *and* moves the content ref, or does neither. So the
//! interesting moment — the appointment changing while a write is in flight — has exactly one
//! outcome, and this example runs it.
//!
//! # Why this shape and not a permission check
//!
//! A check that runs before the write and a write that happens afterwards are two events, and an
//! appointment can change between them. The gap is small and real, and it is precisely where a
//! revoked curator's write lands. Putting the check *inside* the transaction removes the gap
//! rather than narrowing it: git either verifies the appointment and commits the content, or
//! aborts both.
//!
//! # What it does not demonstrate
//!
//! Two curators writing **concurrently** — this is a single process doing one thing at a time, and
//! the race it removes is argued structurally above rather than raced here. Nor the consensus-side
//! handover (`LockService` under replay scenario B, which item 5's D4 audit covers), nor anything
//! about a remote: `push` carries the same fence as `--force-with-lease`, and that path needs a
//! second repository to be worth showing.

use mycelium_wiki::mandate_fence::MandateFence;
use mycelium_wiki::{GitStore, GitStoreConfig, Section, WikiStore};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

fn step(n: u8, title: &str) {
    println!("\n\x1b[1m{n}. {title}\x1b[0m");
}

fn note(s: impl AsRef<str>) {
    println!("   {}", s.as_ref());
}

/// Run a git command in `dir` and return its trimmed stdout.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").current_dir(dir).args(args).output().expect("git runs");
    if !out.status.success() {
        return format!("<git {:?} failed: {}>", args, String::from_utf8_lossy(&out.stderr).trim());
    }
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

const COUNCIL: &str = "norfolk";
const MANDATE_REF: &str = "refs/mycelium/mandate/norfolk";

fn section(body: &str) -> Section {
    Section {
        id: "minutes".into(),
        heading: "Meeting minutes".into(),
        body: body.to_string(),
        attributes: BTreeMap::new(),
    }
}

/// A store as one curator sees it: their name on the commits, and the appointment they believe
/// they hold.
fn store_for(dir: &Path, curator: &str, expected_mandate: &str) -> GitStore {
    let mut cfg = GitStoreConfig::for_group(dir, COUNCIL);
    cfg.author_name = curator.to_string();
    cfg.mandate = Some(MandateFence {
        refname: MANDATE_REF.to_string(),
        expected: expected_mandate.to_string(),
    });
    GitStore::open(cfg).expect("the store opens")
}

fn main() {
    let dir = std::env::temp_dir().join(format!("curator-handover-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");

    println!("\x1b[1mCurator handover — the appointment is checked inside the write's own transaction\x1b[0m");

    // ── 1. A council repo, and an appointment ─────────────────────────────────────────────────
    step(1, "appoint a curator — the appointment is a git ref, not a role advertisement");
    // The store inits the repo; it needs one commit before a ref can point anywhere.
    let bootstrap = store_for_bootstrap(&dir);
    // `write_page` is the non-CAS bootstrap path. The curator path below is
    // `write_section` + `update_manifest`, and it matters that they are separate: the store
    // publishes the manifest **last**, so a half-applied multi-section edit is never observable.
    // A section with no manifest entry is invisible to `read` — which is the store working as
    // designed, and was this example's first bug.
    bootstrap
        .write_page("minutes", &[section("Council formed.")], &BTreeMap::new())
        .expect("the page is bootstrapped");
    let appointment_a = git(&dir, &["rev-parse", "HEAD"]);
    git(&dir, &["update-ref", MANDATE_REF, &appointment_a]);
    note(format!("{MANDATE_REF} -> {}", &appointment_a[..12.min(appointment_a.len())]));
    note("An appointment nobody can forge is an object id in the same repository the writes go to.");
    note("That matters for the next step: the check and the write can then be one transaction.");

    // ── 2. The appointed curator writes ───────────────────────────────────────────────────────
    step(2, "curator A writes — the appointment verified and the content committed, together");
    let curator_a = store_for(&dir, "Curator A", &appointment_a);
    let v = current_version(&curator_a);
    match curator_a.write_section("minutes", &section("Agreed: repair café on Saturdays."), v) {
        Ok(_) => {
            note("write accepted.");
            note(format!("content now: {:?}", read_body(&dir)));
        }
        Err(e) => note(format!("unexpected refusal: {e:?}")),
    }
    note("One `git update-ref --stdin` transaction: `verify <mandate-ref> <expected>` beside");
    note("`update <content-ref> <new> <old>`, then prepare and commit. Both, or neither.");

    // ── 3. The appointment changes ────────────────────────────────────────────────────────────
    step(3, "the council re-appoints — the mandate ref moves");
    let appointment_b = git(&dir, &["rev-parse", "HEAD"]);
    git(&dir, &["update-ref", MANDATE_REF, &appointment_b]);
    note(format!("{MANDATE_REF} -> {}", &appointment_b[..12.min(appointment_b.len())]));
    note("Curator A still holds a store configured with the old appointment. Nothing told it.");

    // ── 4. The revoked curator writes anyway ──────────────────────────────────────────────────
    step(4, "curator A writes again — refused, and nothing is written");
    let before = read_body(&dir);
    let head_before = git(&dir, &["rev-parse", "HEAD"]);
    // The *current* CAS token, so a version conflict cannot be mistaken for the fence: this write
    // is correct in every way except the appointment behind it.
    let v = current_version(&curator_a);
    match curator_a.write_section("minutes", &section("Agreed: sell the hall."), v) {
        Ok(_) => note("UNEXPECTED: a revoked curator's write was accepted"),
        Err(e) => {
            match e.as_mandate_revoked() {
                Some(r) => {
                    note(format!("refused: {r}"));
                    note("");
                    note("Read the refusal, not just the failure. It says **revoked**, not *conflict* —");
                    note("and the difference is the remedy. A conflict means another writer landed first:");
                    note("re-read, re-apply, and you will succeed. A revocation means you are no longer the");
                    note("curator: re-applying refuses forever, and the right move is to stop and find out");
                    note("who holds the appointment. Until 2026-09-19 this said \"re-read and retry\", which");
                    note("is exactly the fix that does not help — found by building this demonstration.");
                }
                None => note(format!("refused, but not as a revocation: {e}")),
            }
            note("");
            note("The refusal is the point, but so is what it left behind:");
            note(format!("  content before : {before:?}"));
            note(format!("  content after  : {:?}", read_body(&dir)));
            note(format!("  HEAD before    : {}", &head_before[..12.min(head_before.len())]));
            let head_after = git(&dir, &["rev-parse", "HEAD"]);
            note(format!("  HEAD after     : {}", &head_after[..12.min(head_after.len())]));
            note("Not a rolled-back write — a write that never happened. git aborted the whole");
            note("transaction at `prepare`, because the appointment it was told to verify had moved.");
        }
    }

    // ── 5. The new curator ────────────────────────────────────────────────────────────────────
    step(5, "curator B writes under the current appointment");
    let curator_b = store_for(&dir, "Curator B", &appointment_b);
    let v = current_version(&curator_b);
    match curator_b.write_section("minutes", &section("Agreed: repair café, and a seed library."), v) {
        Ok(_) => {
            note("write accepted.");
            note(format!("content now: {:?}", read_body(&dir)));
        }
        Err(e) => note(format!("unexpected refusal: {e:?}")),
    }

    // ── 6. Attribution survives the handover ──────────────────────────────────────────────────
    step(6, "attribution survives — the record says who wrote what, across the handover");
    let log = git(&dir, &["log", "--format=%an — %s", "-n", "5"]);
    for line in log.lines() {
        note(line);
    }
    note("");
    note("A handover is not a rewrite. The previous curator's accepted work keeps their name on");
    note("it; what changed is who may write *next*. Authority moved; history did not.");

    // ── 7. What this did not show ─────────────────────────────────────────────────────────────
    step(7, "what this demonstration does not establish");
    note("· two curators writing CONCURRENTLY — one process, one thing at a time; the race the");
    note("  transaction removes is argued structurally, not raced here");
    note("· the consensus-side handover (`LockService` under replay scenario B — item 5's D4 audit)");
    note("· the remote half: `push` carries the same fence as `--force-with-lease`, and showing it");
    note("  honestly needs a second repository");
    note("· expiry by wall-clock: a `Mandate`'s window and `ResourceAuthority::check` are in-memory");
    note("  decisions with their own tests; this example is about the resource's atomic boundary");
    println!();

    let _ = std::fs::remove_dir_all(&dir);
}

/// The council's own store with no fence — used once, to create the commit an appointment can
/// point at. A repository with no commits has no object for a mandate ref to name.
fn store_for_bootstrap(dir: &Path) -> GitStore {
    let mut cfg = GitStoreConfig::for_group(dir, COUNCIL);
    cfg.author_name = "Council".to_string();
    GitStore::open(cfg).expect("the store opens")
}

fn read_body(dir: &Path) -> String {
    let cfg = GitStoreConfig::for_group(dir, COUNCIL);
    let store = GitStore::open(cfg).expect("the store opens");
    store
        .read("minutes")
        .ok()
        .flatten()
        .and_then(|p| p.sections.into_iter().next())
        .map(|s| s.body)
        .unwrap_or_else(|| "<no section>".to_string())
}

/// The section's current CAS token, which `write_section` needs so the write is a compare-and-swap
/// rather than a create. Without it every write after the first is a version conflict — which is a
/// *different* refusal from the mandate fence, and telling them apart is the whole point here.
fn current_version(store: &GitStore) -> Option<u64> {
    store
        .read_versioned("minutes")
        .ok()
        .flatten()
        .and_then(|p| p.sections.get("minutes").map(|(v, _)| *v))
}
