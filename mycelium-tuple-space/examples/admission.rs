//! Worked example — **admission control at a queue, reported beside completions** (v3 item 4 PR 5,
//! `docs/design/adaptive-stability.md` §1's service objective, §3's "the companion that owns the
//! queue owns the deficit").
//!
//! The redistribution hub's `intake` stage has a high watermark. A donation burst larger than the
//! watermark arrives before any sorter is working: the puts past the bound are **refused** — a
//! `TupleError::Backpressure` each — and, since this PR, **counted** at the primary, where the
//! `admission` view reports them beside what was admitted and what was taken. A rejection is a
//! visible outcome, not a silence: an operator reading `admitted`, `rejected`, `taken` sees the
//! bound doing its work, and how much work it turned away. The sorters then drain the stage, and a
//! second burst is admitted.
//!
//! The bound is **self-imposed** (Tier B in the record's vocabulary): the primary enforces it on
//! its own queue, at the point where work is admitted. It is not a fleet ceiling — nothing here
//! claims exclusive rights — and `BackpressureMode::Block` would wait at the same bound instead of
//! returning it.
//!
//! Run: `cargo run -p mycelium-tuple-space --example admission`. Exits 0 on success.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use mycelium::{GossipAgent, GossipConfig, NodeId};
use mycelium_tuple_space::{BackpressureMode, TupleConfig, TupleError, TupleRole, TupleSpace};

const WATERMARK: u32 = 8;
const BURST: u64 = 12;

#[tokio::main]
async fn main() {
    let port = mycelium::test_util::alloc_port();
    let agent = Arc::new(GossipAgent::new(
        NodeId::new("127.0.0.1", port).unwrap(),
        GossipConfig { bind_port: port, ..Default::default() },
    ));
    agent.start().await.unwrap();
    let ts = TupleSpace::new(
        Arc::clone(&agent),
        TupleConfig {
            namespace: Arc::from("admission"),
            role: TupleRole::Primary,
            high_watermark: WATERMARK,
            backpressure_mode: BackpressureMode::Raise,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // ── A burst past the bound, with nobody draining: the excess is refused and counted.
    let (mut admitted, mut refused) = (0u64, 0u64);
    for i in 0..BURST {
        match ts.put("intake", Bytes::from(format!("donation-{i}"))).await {
            Ok(_) => admitted += 1,
            Err(TupleError::Backpressure { retry_after_ms }) => {
                refused += 1;
                println!("dock: donation-{i} refused at admission (retry after {retry_after_ms} ms)");
            }
            Err(e) => panic!("unexpected: {e}"),
        }
    }
    assert_eq!(admitted, WATERMARK as u64, "the bound admits exactly the watermark");
    assert_eq!(refused, BURST - WATERMARK as u64);

    let view = ts.admission(Some("intake")).expect("the primary reports admission");
    let intake = view.iter().find(|s| s.stage.as_ref() == "intake").expect("the stage exists");
    println!(
        "primary: intake admitted={} rejected={} taken={} (watermark {})",
        intake.admitted, intake.rejected, intake.taken, intake.high_watermark
    );
    assert_eq!((intake.admitted, intake.rejected, intake.taken), (WATERMARK as u64, BURST - WATERMARK as u64, 0));

    // ── A sorter drains the stage; the refusals stay on the record beside the completions.
    let mut taken = 0u64;
    loop {
        match ts.take("intake", Duration::from_millis(200)).await {
            Ok((id, _payload)) => {
                ts.complete(id, "sorted", Bytes::from("ok")).await.unwrap();
                taken += 1;
            }
            Err(TupleError::Timeout) => break,
            Err(e) => panic!("unexpected: {e}"),
        }
    }
    assert_eq!(taken, WATERMARK as u64);
    let intake = ts.admission(Some("intake")).unwrap().into_iter().find(|s| s.stage.as_ref() == "intake").unwrap();
    println!("primary: after draining — admitted={} rejected={} taken={}", intake.admitted, intake.rejected, intake.taken);
    assert_eq!((intake.admitted, intake.rejected, intake.taken), (WATERMARK as u64, BURST - WATERMARK as u64, WATERMARK as u64));

    // ── With room again, a second burst is admitted; the count of what was refused does not reset.
    for i in 0..3 {
        ts.put("intake", Bytes::from(format!("late-{i}"))).await.expect("room again");
    }
    let intake = ts.admission(Some("intake")).unwrap().into_iter().find(|s| s.stage.as_ref() == "intake").unwrap();
    assert_eq!((intake.admitted, intake.rejected), (WATERMARK as u64 + 3, BURST - WATERMARK as u64));
    println!("primary: second burst admitted — admitted={} rejected={} (cumulative)", intake.admitted, intake.rejected);

    ts.shutdown().await;
    agent.shutdown().await;
    println!("All assertions passed");
}
