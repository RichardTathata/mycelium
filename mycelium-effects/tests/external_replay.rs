//! **A recorded run that reached the destination once is replayed, and the destination is reached zero
//! times in replay** (adopter-surface X5, rev 0.11): the reconciler's apply and probe are `Input`-class
//! choices, written down by a recording kernel and returned by a replaying one without the call. Shown by
//! the mock destination's own counter. Seen failing first under a plant that made the call in replay.
#![cfg(feature = "sim")]

mod mock_http;

use std::sync::Arc;
use std::time::Duration;

use mock_http::MockHttp;
use mycelium::sim_seam::{install, take, SimContext};
use mycelium_effects::{
    AttemptId, DedupOutcome, Effect, EffectClass, EffectRefusal, EffectState, EgressPolicy, ExternalEffect, ExternalRefusal,
    HttpDestination, OperationId, ProbeAnswer, ProbeBudget, Reconciler,
};
use mycelium_sim::trace::ChoiceKind;
use mycelium_sim::{Kernel, Sources};

fn ledger_path(tag: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("mycelium-x5r-{tag}-{}.sqlite", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}

async fn run(kernel: Kernel, mock: &MockHttp, tag: &str) -> (mycelium_sim::Trace, bool) {
    install(SimContext { kernel, sources: Sources::seeded(5, 1_700_000_000_000), node: "n1".into(), offsets: Default::default() });
    let dest = Arc::new(HttpDestination::new("payments", "127.0.0.1", mock.port, "", Duration::from_millis(700)));
    let r = Reconciler::open(ledger_path(tag), dest, EgressPolicy::default()).unwrap();
    let op = OperationId::new("pay-30");
    let ext = ExternalEffect {
        effect: Effect::new(op.clone(), AttemptId::of(&op, 1), b"lose:30".to_vec()),
        class: EffectClass::Idempotent { compensation: None },
        probe_budget: ProbeBudget { max_probes: 3, max_age_ms: 60_000 },
        composition: None,
    };
    // The run: a lost answer, then a probe that finds it done.
    assert_eq!(r.apply(&ext, None).await, Err(ExternalRefusal::Effect(EffectRefusal::DeliveryUnknown)));
    assert_eq!(r.probe(&op).await, Ok(ProbeAnswer::Done));
    assert_eq!(r.record(&op).unwrap().unwrap().state, EffectState::Committed);
    assert_eq!(r.apply(&ext, None).await.unwrap().dedup, DedupOutcome::Replayed);
    let ctx = take().expect("the kernel this thread ran under");
    (ctx.kernel.trace().clone(), ctx.kernel.is_exhausted())
}

#[tokio::test]
async fn a_recorded_run_reaches_the_destination_once_and_its_replay_zero_times() {
    let mock = MockHttp::start();
    let (trace, _) = run(Kernel::recording(), &mock, "record").await;
    let reached_by_recording = mock.reached();
    assert_eq!(reached_by_recording, 2, "the apply and the probe reached the destination in the recording");
    let inputs: Vec<_> = trace.entries().iter().filter(|c| c.kind == ChoiceKind::Input).collect();
    assert_eq!(inputs.len(), 2, "two input choices: {inputs:?}");
    assert!(inputs[0].stream == "effects/apply" && inputs[0].result == "unknown", "{:?}", inputs[0]);
    assert!(inputs[1].stream == "effects/probe" && inputs[1].result == "done", "{:?}", inputs[1]);

    let (_, exhausted) = run(Kernel::replaying(trace), &mock, "replay").await;
    assert!(exhausted, "the replay consumed the whole recording");
    assert_eq!(mock.reached(), reached_by_recording, "the destination was reached zero times in replay");
}
