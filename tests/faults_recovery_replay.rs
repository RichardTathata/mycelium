//! The `recovery` class's **seamed form** (adopter-surface X9, rev 0.8 and 0.11), which exists only under `sim`:
//! the fleet built and the effects landed for real, then the two recovery phases recorded under the kernel —
//! the reconcilers' sends and probes are `Input`-class choices — and **replayed with the destination reached
//! zero times**, every check made again on the replay. The recording is the corpus bundle
//! `tests/replay-corpus/x9-selective-recovery/`: a fresh recording must ask the same things in the same order,
//! and the checked-in trace must replay here. Re-record with `MYCELIUM_RECORD_CORPUS=1` when a change is intended.
#![cfg(all(feature = "sim", feature = "gateway", feature = "tls"))]

use std::path::{Path, PathBuf};

use mycelium::config::{OnUnreadable, PersistenceConfig, SyncMode};
use mycelium::faults::{run_recovery_seamed, ClassState, Seamed};
use mycelium::wire_check::Unit;
use mycelium::GossipConfig;
use mycelium_sim::bundle::{Build, Bundle};
use mycelium_sim::{Kernel, Trace};

const CORPUS: &str = "x9-selective-recovery";
const SEED: u64 = 9;

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("replay-corpus").join(CORPUS)
}

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mycelium-x9-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn units() -> Vec<Unit> {
    let unit = |name: &str, cap: &str| Unit {
        name: name.into(),
        config: mycelium::NodeCapabilityConfig::from_toml_str(&format!("principal = \"{name}\"\n[[capability]]\nns = \"shop\"\nname = \"{cap}\"\nttl_secs = 5\n")).unwrap(),
    };
    vec![unit("orders", "orders"), unit("fulfilment", "labels"), unit("loyalty", "points")]
}

fn persisted(dir: &Path) -> GossipConfig {
    let mut cfg = GossipConfig::default();
    cfg.persistence = Some(PersistenceConfig { base_path: dir.join("persist"), sync_mode: SyncMode::Flush, snapshot_wal_threshold: 1_000, snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse });
    cfg
}

fn run(tag: &str, kernel: Kernel, replaying: bool) -> (mycelium::faults::ClassReport, mycelium::faults::SeamedOutcome) {
    let dir = scratch(tag);
    let units = units();
    let out = run_recovery_seamed(&units, &persisted(&dir), &dir, Seamed { kernel, seed: SEED, replaying }).expect("the recovery phases ran");
    let _ = std::fs::remove_dir_all(&dir);
    out
}

/// Re-record the corpus (`MYCELIUM_RECORD_CORPUS=1`); otherwise a no-op that says so.
#[test]
fn record_the_recovery_into_the_corpus() {
    if std::env::var_os("MYCELIUM_RECORD_CORPUS").is_none() {
        eprintln!("MYCELIUM_RECORD_CORPUS not set — not re-recording the corpus");
        return;
    }
    let (rep, out) = run("corpus-rec", Kernel::recording(), false);
    assert_eq!(rep.state, ClassState::Passed, "{rep:?}");
    let mut bundle = Bundle::new(out.trace).witnessed_by("faults_recovery_replay::the_checked_in_recovery_bundle_replays_here_and_matches_a_fresh_recording", None);
    bundle.build = Build::current(&["sim", "gateway", "tls"]);
    bundle.config.insert("recovery".into(), format!("node=recovery sources=seeded({SEED}, 1700000000000); the fleet's three nodes and their gossip are outside the kernel"));
    let dir = corpus_dir();
    let _ = std::fs::remove_dir_all(&dir);
    bundle.write(&dir).expect("write the corpus bundle");
    eprintln!("corpus re-recorded at {}", dir.display());
}

/// **The recording reaches the destination; its replay does not, and every check holds on the replay.** The
/// corpus bundle is read, replayed against a fresh fleet, and compared with a fresh recording choice for choice.
#[test]
fn the_checked_in_recovery_bundle_replays_here_and_matches_a_fresh_recording() {
    let dir = corpus_dir();
    let bundle = Bundle::read(&dir).unwrap_or_else(|e| panic!("corpus bundle at {}: {e:?} — record it with MYCELIUM_RECORD_CORPUS=1", dir.display()));
    assert!(bundle.trace.len() >= 10, "a corpus entry with fewer than 10 choices is not the recovery");

    let (rep, replayed) = run("corpus-rep", Kernel::replaying(bundle.trace.clone()), true);
    assert_eq!(rep.state, ClassState::Passed, "the replay's checks: {rep:?}");
    assert!(replayed.exhausted, "the replay consumed the whole recording (position {} of {})", replayed.position, bundle.trace.len());
    assert_eq!(replayed.destination_reached_in_recovery, 0, "the destination was reached in replay");
    assert!(rep.checks.iter().any(|k| k.name.contains("reached zero times during the recovery's replay") && k.ok == Some(true)), "{rep:?}");

    let (fresh_rep, fresh) = run("corpus-fresh", Kernel::recording(), false);
    assert_eq!(fresh_rep.state, ClassState::Passed, "{fresh_rep:?}");
    assert!(fresh.destination_reached_in_recovery > 0, "the recording reached the destination");
    // The five calls that reach the destination in the two recovery phases, each an input choice: the root's
    // probe, the idempotent dependent's probe and its one retry (the second is refused before the seam), the
    // compensatable dependent's probe (its retry is refused before the seam), the compensation. The irreversible
    // dependent is neither probed nor sent, and a retry without a probe never reaches the seam.
    let inputs: Vec<_> = fresh.trace.entries().iter().filter(|c| c.kind == mycelium_sim::ChoiceKind::Input).map(|c| format!("{} {}", c.stream, c.request)).collect();
    assert_eq!(inputs.len(), 5, "the recovery's sends and probes are input choices: {inputs:?}");
    assert!(inputs.iter().filter(|i| i.starts_with("effects/probe")).count() == 3 && inputs.iter().any(|i| i.starts_with("effects/retry")) && inputs.iter().any(|i| i.starts_with("effects/compensate")), "{inputs:?}");
    assert_eq!(fresh.trace.len(), bundle.trace.len(), "a different number of choices than the corpus records");
    for (i, (a, b)) in fresh.trace.entries().iter().zip(bundle.trace.entries()).enumerate() {
        assert!(a.same_request(b), "choice {i} differs from the corpus:\n  fresh:  {}\n  corpus: {}\nre-record with MYCELIUM_RECORD_CORPUS=1 if the change is intended", a.to_line(), b.to_line());
    }
    let _ = Trace::default();
}
