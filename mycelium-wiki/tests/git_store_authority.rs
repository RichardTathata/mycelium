//! **Authority at execution for the wiki store** (Boundary H item A1).
//!
//! The mandate fence stops a *superseded* curator inside the git transaction. These tests are for
//! the curators it cannot see: one whose mandate has **expired**, one **revoked** by a checkpoint,
//! and one that has **heard nothing** from its authority for longer than the freshness bound. Each
//! must write nothing and publish nothing, and the refusal must name itself: it is not a conflict
//! (retrying will not help) and not a gate refusal (the content is not at fault, so the curator
//! must not drop the proposals).
//!
//! The clock is the test's: an `AtomicU64` the authority reads through its `now_ms` closure.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::{Signer, SigningKey};
use mycelium::knowledge::issuer::{MemberKeys, TrustedExternalIssuers};
use mycelium::knowledge::IssuerId;
use mycelium::mandate::authority::{
    CheckpointOffer, ClockModel, ExecutionGate, FreshnessPolicy, ResourceTier, RevocationCheckpoint,
    SignedRevocationCheckpoint,
};
use mycelium::mandate::{Mandate, PrincipalId, ResourceAuthority, TermId};
use mycelium::{GossipAgent, GossipConfig, NodeId};
use mycelium_wiki::{
    mint_section_id, ExecutionGateAuthority, FsStore, GitStore, GitStoreConfig, Section, Wiki, WikiConfig, WikiError,
    WikiRole, WikiStore, WIKI_WRITE,
};

const AUTHORITY: &str = "operator:council";
const SCOPE: &str = "testville";
/// *s* = 100 ms, *F* = 60 s: a checkpoint issued at `a` is fresh until `a + F − 2s`.
const FRESH_FOR_MS: u64 = 60_000 - 200;

struct Rig {
    clock: Arc<AtomicU64>,
    authority: Arc<ExecutionGateAuthority>,
    key: SigningKey,
    seq: AtomicU64,
}

impl Rig {
    /// A curator term `t1` at epoch 1, valid from 0 until `valid_until_ms`. The clock starts at 1 s.
    fn new(valid_until_ms: u64) -> Self {
        Self::starting_at(valid_until_ms, 1_000)
    }

    /// The same, with the clock (and so the authority's start) at `start_ms`: a restarted curator.
    fn starting_at(valid_until_ms: u64, start_ms: u64) -> Self {
        let key = SigningKey::from_bytes(&[71u8; 32]);
        let mut external = TrustedExternalIssuers::new();
        external.trust(IssuerId::new(AUTHORITY).unwrap(), key.verifying_key().to_bytes()).unwrap();
        let gate = ExecutionGate::strict(
            ResourceAuthority::new(SCOPE, 1),
            ResourceTier::Transactional,
            ClockModel { skew_ms: 100 },
            FreshnessPolicy { freshness_ms: 60_000, interval_ms: 20_000, delivery_ms: 5_000 },
        )
        .unwrap();
        let mandate = Mandate {
            holder: PrincipalId::new("node:curator-a").unwrap(),
            established_by: PrincipalId::new(AUTHORITY).unwrap(),
            purpose: "curate the council wiki".into(),
            scope: SCOPE.into(),
            operations: vec![WIKI_WRITE.into()],
            epoch: 1,
            term: TermId::new("t1").unwrap(),
            valid_from_ms: 0,
            valid_until_ms,
        };
        let clock = Arc::new(AtomicU64::new(start_ms));
        let c = Arc::clone(&clock);
        let authority =
            Arc::new(ExecutionGateAuthority::new(gate, mandate, external, move || c.load(Ordering::SeqCst)));
        Self { clock, authority, key, seq: AtomicU64::new(0) }
    }

    fn at(&self, now_ms: u64) {
        self.clock.store(now_ms, Ordering::SeqCst);
    }

    /// Offer a checkpoint issued now, revoking `revoked`.
    fn checkpoint(&self, revoked: &[&str]) -> CheckpointOffer {
        self.checkpoint_issued_at(self.clock.load(Ordering::SeqCst), revoked)
    }

    /// Offer a checkpoint issued at `issued_at_ms` (a replay, when that is in the past).
    fn checkpoint_issued_at(&self, issued_at_ms: u64, revoked: &[&str]) -> CheckpointOffer {
        let c = RevocationCheckpoint {
            authority: PrincipalId::new(AUTHORITY).unwrap(),
            scope: SCOPE.into(),
            seq: self.seq.fetch_add(1, Ordering::SeqCst) + 1,
            issued_at_ms,
            revoked: revoked.iter().map(|t| TermId::new(t).unwrap()).collect(),
        };
        let signed = SignedRevocationCheckpoint {
            signature: self.key.sign(&c.canonical_bytes()).to_bytes().to_vec(),
            checkpoint: c,
        };
        let no_members: std::collections::HashMap<NodeId, MemberKeys> = Default::default();
        self.authority.offer_checkpoint(&signed, &no_members)
    }

    fn store(&self, dir: &Path) -> GitStore {
        self.store_with(GitStoreConfig { dir: dir.to_path_buf(), ..Default::default() })
    }

    fn store_with(&self, cfg: GitStoreConfig) -> GitStore {
        GitStore::open(GitStoreConfig {
            subdir: "councils/testville".into(),
            message_prefix: "wiki(testville)".into(),
            authority: Some(Arc::clone(&self.authority) as _),
            ..cfg
        })
        .unwrap()
    }
}

fn minutes(body: &str) -> Section {
    Section { id: Arc::from("minutes"), heading: "Minutes".into(), body: body.into(), attributes: BTreeMap::new() }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").current_dir(dir).args(args).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn write(store: &impl WikiStore, body: &str) -> Result<(), WikiError> {
    store.write_page("minutes", &[minutes(body)], &BTreeMap::new()).map(|_| ())
}

/// The refusal names itself, and nothing reached the branch.
fn assert_refused_as(err: &WikiError, denial: &str) {
    let reason = err.as_authority_refused().unwrap_or_else(|| panic!("refused as an authority refusal: {err}"));
    assert!(reason.contains(denial), "the reason names {denial}: {reason}");
    assert!(!matches!(err, WikiError::Conflict), "not a retry signal");
    assert!(err.as_gate_refusal().is_none(), "not a content fault: the proposals must not be dropped");
    assert!(err.as_mandate_revoked().is_none(), "not the fence: the appointment ref never moved");
}

/// **Silence is not evidence.** A curator that has never heard from its authority does not know
/// whether it was revoked, so it writes nothing.
#[test]
fn without_a_checkpoint_nothing_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let rig = Rig::new(1_000_000);
    let store = rig.store(dir.path());

    let err = write(&store, "sell the hall").expect_err("no revocation view, no write");
    assert_refused_as(&err, "RevocationUnknown");
    assert_eq!(store.read("minutes").unwrap(), None, "nothing written");
    assert_eq!(git(dir.path(), &["rev-list", "--all", "--count"]), "0", "no commit reached any ref");
}

/// The plant: with present authority the same write lands. Without it, the refusals above would
/// pass on a store that refused everything.
#[test]
fn with_present_authority_the_write_lands() {
    let dir = tempfile::tempdir().unwrap();
    let rig = Rig::new(1_000_000);
    let store = rig.store(dir.path());
    assert_eq!(rig.checkpoint(&[]), CheckpointOffer::Accepted);

    write(&store, "the council met").expect("an authorised curator writes");
    assert!(store.read("minutes").unwrap().is_some());
}

/// **Expiry is checked at the write, not at acceptance.** A curator authorised a moment ago, with a
/// perfectly fresh checkpoint, writes nothing once its window has closed.
#[test]
fn an_expired_mandate_writes_nothing_even_with_a_fresh_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let rig = Rig::new(50_000);
    let store = rig.store(dir.path());
    rig.checkpoint(&[]);
    write(&store, "first").expect("inside the window");
    let head = git(dir.path(), &["rev-parse", "HEAD"]);

    rig.at(50_001);
    rig.checkpoint(&[]); // fresh: the refusal is expiry, not silence
    let err = write(&store, "after hours").expect_err("the window has closed");
    assert_refused_as(&err, "WorkExpired");
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), head, "no commit landed");
}

/// **A revocation stops the next write.** The fence cannot see this: the appointment ref never
/// moved.
#[test]
fn a_revoked_curator_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let rig = Rig::new(1_000_000);
    let store = rig.store(dir.path());
    rig.checkpoint(&[]);
    write(&store, "first").unwrap();
    let head = git(dir.path(), &["rev-parse", "HEAD"]);

    rig.at(2_000);
    assert_eq!(rig.checkpoint(&["t1"]), CheckpointOffer::Accepted);
    let err = write(&store, "sell the hall").expect_err("revoked");
    assert_refused_as(&err, "Revoked");
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), head, "no commit landed");

    // A revocation, once seen, stands: a later checkpoint that omits it restores nothing.
    rig.at(3_000);
    rig.checkpoint(&[]);
    assert_refused_as(&write(&store, "again").unwrap_err(), "Revoked");
}

/// **A partition fails closed.** Once the newest checkpoint is older than the freshness bound, the
/// curator can no longer know it has not been revoked, and stops writing. Exactly at the bound it
/// still writes; one millisecond past, it does not.
#[test]
fn silence_past_the_freshness_bound_stops_writes() {
    let dir = tempfile::tempdir().unwrap();
    let rig = Rig::new(1_000_000);
    let store = rig.store(dir.path());
    rig.checkpoint(&[]); // issued at 1 000

    rig.at(1_000 + FRESH_FOR_MS);
    write(&store, "at the bound").expect("still fresh at a + F − 2s");

    rig.at(1_000 + FRESH_FOR_MS + 1);
    assert_refused_as(&write(&store, "past the bound").unwrap_err(), "RevocationUnknown");

    // The authority's next checkpoint restores it: the stop was for want of news, not a verdict.
    rig.checkpoint(&[]);
    write(&store, "news again").expect("a fresh checkpoint restores present authority");
}

/// **A superseded epoch stops the next write** at the store's own check, before git is asked.
#[test]
fn a_superseded_epoch_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let rig = Rig::new(1_000_000);
    let store = rig.store(dir.path());
    rig.checkpoint(&[]);
    write(&store, "first").unwrap();

    assert!(rig.authority.install_epoch(2));
    let err = write(&store, "second").expect_err("superseded");
    assert_refused_as(&err, "Refused");
}

/// **A commit made under authority does not carry that authority to the remote.** The curator
/// commits while authorised; it is revoked before it publishes; the push does not happen, and the
/// shared remote never sees the commit.
#[test]
fn a_commit_made_under_authority_is_not_published_after_revocation() {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin.git");
    std::fs::create_dir_all(&origin).unwrap();
    git(&origin, &["init", "-q", "--bare", "-b", "main"]);
    let remote = origin.to_string_lossy().into_owned();

    let rig = Rig::new(1_000_000);
    let store = rig.store_with(GitStoreConfig { dir: tmp.path().join("clone"), ..Default::default() }.with_remote(&remote));
    rig.checkpoint(&[]);
    write(&store, "the council met").unwrap();

    rig.at(2_000);
    rig.checkpoint(&["t1"]);
    let err = store.publish().expect_err("a revoked curator does not publish");
    assert_refused_as(&err, "Revoked");
    assert_eq!(git(&origin, &["for-each-ref"]), "", "the remote never received the commit");
}

/// **End to end through the curator.** Proposals queued while the curator has no present
/// authority are **left queued**, not dropped as a gate refusal would drop them, and nothing is
/// committed. When the authority's checkpoint arrives, the same queued proposal lands: the
/// requirement travelled with the queued work, and the work was not lost to it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_curator_without_present_authority_keeps_proposals_queued_until_it_has_it() {
    let tmp = tempfile::tempdir().unwrap();
    let rig = Rig::new(u64::MAX);
    let store = Arc::new(rig.store(tmp.path()));

    let agent = loop {
        let port = mycelium::test_util::alloc_port();
        let cfg = GossipConfig { bind_port: port, ..Default::default() };
        let a = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
        if a.start().await.is_ok() {
            break a;
        }
    };
    let wiki =
        Wiki::new(Arc::clone(&agent), WikiConfig::new(SCOPE).role(WikiRole::Curator), Arc::clone(&store)).await;

    let page = "minutes/2026-09-25";
    wiki.propose(page, mint_section_id(SCOPE, page, 1, 1), "Opening", "The council opened.", Default::default());

    // Several drain rounds pass with no revocation view: nothing lands, the proposal stays.
    let queued = || agent.kv().scan_prefix(&format!("wiki/{SCOPE}/proposal/")).len();
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(store.read(page).unwrap(), None, "no present authority, no write");
    assert_eq!(queued(), 1, "the proposal is kept for a curator with authority, not dropped");
    assert_eq!(git(tmp.path(), &["rev-list", "--all", "--count"]), "0", "no commit reached any ref");

    // The authority speaks; the queued proposal lands on a later drain.
    rig.checkpoint(&[]);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while store.read(page).unwrap().is_none() {
        assert!(tokio::time::Instant::now() < deadline, "the queued proposal never landed");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while queued() != 0 {
        assert!(tokio::time::Instant::now() < deadline, "the applied proposal was never tombstoned");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    wiki.shutdown().await;
}

/// **A stated limit, pinned so it cannot pass for a closed one.** The authority is asked, then git
/// updates the ref. A process that pauses between the two writes after its mandate expired, by the
/// length of the pause. This wrapper passes the real check and then moves the clock past expiry,
/// which is what a pause does. The write lands: nothing local can refuse it, because git has no clock
/// the check can sit inside. The next write is refused, so the overrun is bounded by one pause.
/// Prevention for published writes belongs at the remote (closure plan C9).
#[test]
fn a_pause_after_the_check_is_not_caught_locally() {
    struct PausesAfterTheCheck {
        inner: Arc<ExecutionGateAuthority>,
        clock: Arc<AtomicU64>,
        resume_at_ms: u64,
    }
    impl mycelium_wiki::mandate_fence::WriteAuthority for PausesAfterTheCheck {
        fn authorize_write(&self) -> Result<(), String> {
            let answer = mycelium_wiki::mandate_fence::WriteAuthority::authorize_write(&*self.inner);
            self.clock.store(self.resume_at_ms, Ordering::SeqCst); // the pause
            answer
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let rig = Rig::new(50_000);
    rig.at(49_000);
    rig.checkpoint(&[]);
    let paused = Arc::new(PausesAfterTheCheck {
        inner: Arc::clone(&rig.authority),
        clock: Arc::clone(&rig.clock),
        resume_at_ms: 60_000,
    });
    let store = GitStore::open(GitStoreConfig {
        dir: dir.path().to_path_buf(),
        subdir: "councils/testville".into(),
        message_prefix: "wiki(testville)".into(),
        authority: Some(paused as _),
        ..Default::default()
    })
    .unwrap();

    write(&store, "checked at 49 s, committed at 60 s").expect("the limit: nothing local refuses it");
    assert!(rig.clock.load(Ordering::SeqCst) > 50_000, "the write landed after the mandate expired");
    assert_refused_as(&write(&store, "the next one").unwrap_err(), "WorkExpired");
}

/// **Closure plan C6: the filesystem store takes the same authority.** `FsStore` has no appointment
/// fence, so the authority is its whole check. The cases that do not depend on git: silence, the
/// plant, expiry and revocation, each writing nothing when refused.
#[test]
fn the_fs_store_asks_the_same_authority_and_writes_nothing_when_refused() {
    let dir = tempfile::tempdir().unwrap();
    let rig = Rig::new(50_000);
    let store = FsStore::open(dir.path(), "testville").unwrap().with_authority(Arc::clone(&rig.authority) as _);

    assert_refused_as(&write(&store, "no news yet").unwrap_err(), "RevocationUnknown");
    assert_eq!(store.read("minutes").unwrap(), None, "silence writes nothing");

    rig.checkpoint(&[]);
    write(&store, "the council met").expect("the plant: with present authority the write lands");
    let written = store.read("minutes").unwrap();

    rig.at(50_001);
    rig.checkpoint(&[]);
    assert_refused_as(&write(&store, "after hours").unwrap_err(), "WorkExpired");

    let fresh = Rig::new(1_000_000);
    let store2 = FsStore::open(dir.path(), "other").unwrap().with_authority(Arc::clone(&fresh.authority) as _);
    fresh.checkpoint(&[]);
    fresh.at(2_000);
    fresh.checkpoint(&["t1"]);
    assert_refused_as(&write(&store2, "sell the hall").unwrap_err(), "Revoked");
    assert_eq!(store2.read("minutes").unwrap(), None);
    assert_eq!(store.read("minutes").unwrap(), written, "the refused writes changed nothing");
}

/// **Closure plan C8: a restart does not restore a revoked curator.** Before the restart the curator
/// was revoked (at 2 s). After it (the process starts at 5 s), its memory of that is gone, and a
/// checkpoint from 1 s, before the revocation and still fresh, is replayed at it. That checkpoint
/// refreshes nothing, so the curator writes nothing; the authority's next, cumulative checkpoint
/// names the revocation, and it still writes nothing.
#[test]
fn a_restarted_curator_is_not_restored_by_a_replayed_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let rig = Rig::starting_at(1_000_000, 5_000);
    let store = rig.store(dir.path());

    assert!(matches!(rig.checkpoint_issued_at(1_000, &[]), CheckpointOffer::IssuedBeforeStart { .. }));
    assert_refused_as(&write(&store, "restored?").unwrap_err(), "RevocationUnknown");

    rig.at(5_100);
    assert_eq!(rig.checkpoint(&["t1"]), CheckpointOffer::Accepted);
    assert_refused_as(&write(&store, "restored?").unwrap_err(), "Revoked");
    assert_eq!(store.read("minutes").unwrap(), None, "nothing was written");
}

