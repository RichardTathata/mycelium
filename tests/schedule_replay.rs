//! Adopter-surface Y2's last promise: **the tick that fires a run goes through the timer seam like every periodic
//! loop, so a recording replays it.** One node runs a `[[schedule]]` under the replay kernel — its own `last` entry
//! seeded three slots behind the slot due, so the first tick it holds decides a missed run (`run_once`) and runs it —
//! and the recording is replayed against a fresh node on a paused clock: the replay consumes the whole trace, and the
//! schedule's decisions come back the same, record for record, on their canonical tuples (strictly — a schedule
//! decision reads gossiped inputs, so `Comparison::reproduces` alone would let a difference through as
//! unattributable). The loop's seam streams: the wall clock it reads the slot from, the monotonic clock its warm-up is
//! measured on, its `sched/{id}` ticker, and the per-run deadline (`sched/{id}/deadline`, raced and dropped when the
//! work finishes first).
//!
//! The recording is the corpus bundle `tests/replay-corpus/y2-schedule-run/`, with the decisions it produced as its
//! `decisions.jsonl`; re-record with `-- --ignored record_the_schedule_run_into_the_corpus` when a change is intended.
#![cfg(feature = "sim")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mycelium::decision::{DecisionRecord, DecisionSink};
use mycelium::sim_seam::{install, take, SimContext};
use mycelium::{GossipAgent, GossipConfig, NodeCapabilityConfig, NodeId};
use mycelium_sim::bundle::{Build, Bundle, DECISION_ATTACHMENT};
use mycelium_sim::{Kernel, Sources, Trace};

const CORPUS: &str = "y2-schedule-run";
const SEED: u64 = 2;
/// Where the simulated wall clock starts: slot 1 700 000 000 of a one-second schedule.
const WALL_START_MS: u64 = 1_700_000_000_000;
/// How far the simulated monotonic clock moves per read: the warm-up (three one-second lease refreshes) passes in
/// thirty reads rather than three thousand.
const MONO_STEP_NS: u64 = 100_000_000;
/// How long the first run's work takes: past the schedule's one-second deadline.
const OVERRUN_MS: u64 = 3_000;
/// The port the recorded node binds and is named by — below `test_util::alloc_port`'s 16 000..32 000 and the OS
/// ephemeral floor, so no allocated or ephemeral port lands on it.
const RECORDED_PORT: u16 = 15_946;
/// One node on [`RECORDED_PORT`] at a time: the two tests here run on parallel threads when both are selected.
static ONE_NODE: std::sync::Mutex<()> = std::sync::Mutex::new(());

const UNIT: &str = r#"
principal = "books"
[[capability]]
ns = "ledger"
name = "closer"
ttl_secs = 30

[[schedule]]
id = "close"
period_secs = 1
on_missed = "run_once"
[schedule.holders]
ns = "ledger"
name = "closer"
"#;

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("replay-corpus").join(CORPUS)
}

/// What one schedule decision is, as a replay must reproduce it: the rule and its revision, the trigger and target,
/// how it ended and why, what it caused, and every input it read — source and value, so a replay that reached the same
/// outcome from a different clock, slot or gap is caught (the canonical tuple alone names the outcome, not its
/// inputs). The node's name, which carries its port, is written `n1` wherever it appears: a replay runs on another
/// port than its recording, and the node's name is not a decision.
fn projection(r: &serde_json::Value) -> serde_json::Value {
    let node = r["node"].as_str().unwrap_or_default().to_string();
    let neutral = |v: &serde_json::Value| -> serde_json::Value {
        match v.as_str() {
            Some(s) if !node.is_empty() => serde_json::Value::String(s.replace(&node, "n1")),
            _ => v.clone(),
        }
    };
    let inputs: Vec<serde_json::Value> = r["inputs"].as_array().cloned().unwrap_or_default().iter()
        .map(|i| serde_json::json!({ "source": i["source"], "value": neutral(&i["value"]), "provenance": i["provenance"] }))
        .collect();
    serde_json::json!({
        "rule": r["rule"], "rule_revision": r["rule_revision"], "trigger": r["trigger"], "target": r["target"],
        "outcome": r["outcome"], "reason": r["reason"], "effect": neutral(&r["effect"]),
        "has_parent": !r["parent_seq"].is_null(), "inputs": inputs,
    })
}

/// The schedule's decisions out of everything the sink recorded, as JSON (the form a bundle carries them in).
fn schedule_decisions(all: &[DecisionRecord]) -> Vec<serde_json::Value> {
    all.iter().filter(|r| r.rule.starts_with("sched.")).map(|r| serde_json::to_value(r).unwrap()).collect()
}

/// One node, one schedule, under `kernel`. Returns the trace, the schedule's decisions and whether a replay consumed
/// every recorded choice.
async fn one_node(kernel: Kernel, replaying: bool) -> (Trace, Vec<serde_json::Value>, bool) {
    let _one = ONE_NODE.lock().unwrap_or_else(|p| p.into_inner());
    let mut sources = Sources::seeded(SEED, WALL_START_MS);
    sources.mono_step_ns = MONO_STEP_NS;
    install(SimContext { kernel, sources, node: "n1".into(), offsets: Default::default() });
    if replaying {
        mycelium::sim_seam::pause_clock_for_replay();
    }
    let sink = Arc::new(DecisionSink::default());
    let mut cfg = GossipConfig::auto();
    // The node's name is fixed: it is inside the keys the gossip shards hash and the streams the persist task ticks
    // on (`kv-persist/cap/{node}/…`), so a recording made under one name replays only under the same one — a fresh
    // port per run diverges at the first sharded send. `start()` requires the name to be the bind address, so the
    // port is fixed too.
    cfg.bind_port = RECORDED_PORT;
    let node = NodeId::new("127.0.0.1", RECORDED_PORT).unwrap();
    let a = GossipAgent::new(node.clone(), cfg);
    a.with_decision_trace(Arc::clone(&sink));
    a.start().await.unwrap();

    let unit = NodeCapabilityConfig::from_toml_str(UNIT).unwrap();
    let _cap = a.capabilities().advertise_capability(mycelium::Capability::new("ledger", "closer"), Duration::from_secs(30));
    // This node's own `last`, three slots behind the slot the simulated clock starts in: the first tick it holds
    // finds a gap and decides it under `run_once`.
    let due = WALL_START_MS / 1_000;
    assert!(a.kv().set(format!("sched/close/last/{node}"), format!("{} 1000 0", due - 3)), "seed this node's last");
    // The first run overruns its one-period deadline (the deadline is a sleep through the seam, so the recording holds
    // it and the replay's paused clock reproduces it winning); every later run finishes at once.
    let ran = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let r = Arc::clone(&ran);
    let handle = a
        .run_schedule(&unit.schedules[0], move |_run| {
            let first = r.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0;
            async move {
                if first {
                    tokio::time::sleep(Duration::from_millis(OVERRUN_MS)).await;
                }
                Ok(())
            }
        })
        .unwrap();
    for _ in 0..400 {
        if sink.snapshot().iter().any(|d| d.rule == "sched.run" && d.reason == "ran_for_gap") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    handle.stop();
    a.shutdown().await;
    let ctx = take().expect("the kernel this thread ran under");
    let exhausted = ctx.kernel.is_exhausted();
    (ctx.kernel.trace().clone(), schedule_decisions(&sink.drain()), exhausted)
}

/// What a recording depends on beyond its trace, as this build has it: the library's features (the guarantee report's
/// own `cfg!` list — what cargo resolved, dev-dependency unification included — plus `sim`) and the gossip shard
/// count a node starts with. The gateway and TLS tasks make choices of their own, and the shard count is the channel
/// stream of every gossip send, so a recording made under different ones would diverge at a bare stream mismatch.
fn this_build() -> (Vec<String>, usize) {
    let agent = GossipAgent::new(NodeId::new("127.0.0.1", RECORDED_PORT).unwrap(), GossipConfig::auto());
    let mut features: Vec<String> = agent.guarantee_report().features.iter().map(|f| f.to_string()).collect();
    features.push("sim".into());
    features.sort();
    // `GossipAgent::new` rounds the configured count up to a power of two; the node runs that many.
    (features, GossipConfig::auto().gossip_shards.next_power_of_two())
}

/// The bundle's build fact under which this test records the shard count.
const SHARDS_KEY: &str = "gossip_shards";

/// Refuse, by name, to replay a recording made by another build — a different feature set or shard count — rather
/// than let it diverge at its first choice that differs.
fn refuse_unless_recorded_here(bundle: &Bundle, (features, shards): &(Vec<String>, usize)) -> Result<(), String> {
    let mut recorded = bundle.build.features.clone();
    recorded.sort();
    if &recorded != features {
        return Err(format!("refused: the recording was made with features {recorded:?}; this build has {features:?}"));
    }
    let recorded_shards = bundle.config.get(SHARDS_KEY).cloned().unwrap_or_else(|| "unrecorded".into());
    if recorded_shards != shards.to_string() {
        return Err(format!("refused: the recording was made with {SHARDS_KEY} = {recorded_shards}; this build starts with {shards}"));
    }
    Ok(())
}

/// The paths the recording holds: the warm-up, a missed-run decision, a run that overran its deadline, the gap decided
/// again and run, and the on-time run.
fn assert_ran_the_gap(decisions: &[serde_json::Value]) {
    let reasons: Vec<_> = decisions.iter().map(|d| format!("{} {}", d["rule"].as_str().unwrap_or("-"), d["reason"].as_str().unwrap_or("-"))).collect();
    for want in ["sched.run warming_up", "sched.missed ran_once", "sched.run work_overran", "sched.run ran_for_gap", "sched.run ran"] {
        assert!(reasons.iter().any(|r| r == want), "{want}: {reasons:?}");
    }
    let overran = reasons.iter().position(|r| r == "sched.run work_overran").unwrap();
    let gap_run = reasons.iter().position(|r| r == "sched.run ran_for_gap").unwrap();
    assert!(overran < gap_run, "the overrun first, then the gap run on a later tick: {reasons:?}");
}

/// Re-record the corpus. Ignored, so it never counts as run when it did nothing: re-record on purpose with
/// `cargo test --features sim --test schedule_replay -- --ignored record_the_schedule_run_into_the_corpus`.
#[tokio::test]
#[ignore = "re-records the checked-in corpus: run on purpose with -- --ignored"]
async fn record_the_schedule_run_into_the_corpus() {
    let (trace, decisions, _) = one_node(Kernel::recording(), false).await;
    assert_ran_the_gap(&decisions);
    let jsonl: String = decisions.iter().map(|r| r.to_string() + "\n").collect();
    let mut bundle = Bundle::new(trace)
        .with_attachment(DECISION_ATTACHMENT, jsonl.into_bytes())
        .witnessed_by("schedule_replay::the_checked_in_schedule_run_replays_decision_for_decision", None);
    let (features, shards) = this_build();
    bundle.build = Build::current(&features.iter().map(String::as_str).collect::<Vec<_>>());
    bundle.config.insert(SHARDS_KEY.into(), shards.to_string());
    bundle.config.insert("schedule".into(), format!(
        "node=n1 sources=seeded({SEED}, {WALL_START_MS}) mono_step_ns={MONO_STEP_NS}; one node, `close` every second, its own last three slots behind, on_missed=run_once"
    ));
    let dir = corpus_dir();
    let _ = std::fs::remove_dir_all(&dir);
    bundle.write(&dir).expect("write the corpus bundle");
    eprintln!("corpus re-recorded at {}", dir.display());
}

/// **A recorded schedule run replays decision for decision.** The checked-in recording replayed against a fresh node
/// on a paused clock: the whole trace consumed, the missed-run decision and the run it caused made again, and every
/// schedule decision the recording made — the warm-up, the gap, the run — equal on its canonical tuple.
#[tokio::test]
async fn the_checked_in_schedule_run_replays_decision_for_decision() {
    let dir = corpus_dir();
    let bundle = Bundle::read(&dir).unwrap_or_else(|e| panic!("corpus bundle at {}: {e:?} — record it with `-- --ignored record_the_schedule_run_into_the_corpus`", dir.display()));
    let recorded: Vec<serde_json::Value> = std::str::from_utf8(&bundle.attachments[DECISION_ATTACHMENT])
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_ran_the_gap(&recorded);
    if let Err(why) = refuse_unless_recorded_here(&bundle, &this_build()) {
        panic!("{why} — re-record with `-- --ignored record_the_schedule_run_into_the_corpus` under this build, or run it under the recorded one");
    }
    let streams: std::collections::BTreeSet<_> = bundle.trace.entries().iter().map(|c| c.stream.clone()).collect();
    assert!(streams.iter().any(|s| s == "sched/close"), "the schedule's ticker is a recorded stream: {streams:?}");
    assert!(streams.iter().any(|s| s == "sched/close/deadline"), "the run's deadline is a recorded stream: {streams:?}");

    let choices = bundle.trace.len();
    let (_, replayed, exhausted) = one_node(Kernel::replaying(bundle.trace.clone()), true).await;
    assert!(exhausted, "the replay consumed all {choices} recorded choices");
    assert_ran_the_gap(&replayed);
    assert_eq!(recorded.len(), replayed.len(), "the same number of schedule decisions");
    for (i, (a, b)) in recorded.iter().zip(&replayed).enumerate() {
        assert_eq!(projection(a), projection(b), "schedule decision {i} differs between the recording and its replay");
    }
}

/// A recording made by another build is refused by name before it is replayed — its features or its shard count
/// changed in the bundle, the same bundle otherwise.
#[tokio::test]
async fn a_recording_from_another_build_is_refused_by_name() {
    let bundle = Bundle::read(&corpus_dir()).expect("the corpus bundle");
    let here = this_build();
    assert_eq!(refuse_unless_recorded_here(&bundle, &here), Ok(()), "the checked-in recording is this build's");

    let mut other_features = bundle.clone();
    other_features.build.features.retain(|f| f != "tls");
    let why = refuse_unless_recorded_here(&other_features, &here).unwrap_err();
    assert!(why.contains("features") && why.contains("tls"), "named: {why}");

    let mut other_shards = bundle.clone();
    other_shards.config.insert(SHARDS_KEY.into(), "16".into());
    let why = refuse_unless_recorded_here(&other_shards, &here).unwrap_err();
    assert!(why.contains("gossip_shards = 16"), "named: {why}");

    let mut unrecorded = bundle.clone();
    unrecorded.config.remove(SHARDS_KEY);
    assert!(refuse_unless_recorded_here(&unrecorded, &here).unwrap_err().contains("unrecorded"));
}
