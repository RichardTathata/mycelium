//! The schedule run stream (adopter-surface Y12): `GET /gateway/schedules/run` and `POST /gateway/schedules/ack`.
//!
//! An SDK agent cannot run a `[[schedule]]` in process — `GossipAgent::run_schedule` takes the work as a Rust closure —
//! so the gateway's node **stands in the holder band for the client**. The stream declares one schedule and keeps it
//! running while it is open: this node is a candidate, elected by the same band and rendezvous as any holder
//! (`mycelium::schedule`), and each run it decides is handed to the client as a `run` event carrying the schedule id,
//! the slot and the run's idempotency key `sched/{id}/{slot}`. The run is done when the client acknowledges it
//! (`schedules/ack`, `ok` or the error), within one period like any run's work.
//!
//! **A client's schedule is its principal's.** The stream runs the declaration under
//! [`client_scoped_id`](crate::schedule::client_scoped_id) — `{id}~{principal}`, the principal as this gateway derived
//! it (never from the request), percent-encoded — so its ring, advertisements, `sched/` entries and trace are its own:
//! a client reaches neither an application's `run_schedule` of the same id (whose ids cannot hold `~`) nor another
//! principal's. Clients sharing one principal — an open gateway's `anonymous`, one bearer, one scoped token — share one
//! namespace. The client sees its own id in `schedule`; a run's `operation_id` is the scoped key
//! `sched/{id}~{principal}/{slot}`, so two principals' runs of one slot are two effects.
//!
//! **A run the client never acknowledged is not a run.** If the stream closes while a run is handed out, or the client
//! never answers, the run is traced `unacknowledged` (or `work_overran` past its period), the slot is not recorded,
//! and this node withdraws its holding: the next holder — another client's node, or this one when a client comes
//! back — decides the slot as one not yet run, on time or as a missed run under the policy, traced with its reason.
//! Closing the stream ends the loop and withdraws the candidacy. **A client that stays connected and stalls** — its run
//! overruns the period — makes this node **stand down**: the holding and the candidacy are withdrawn for
//! [`stand_down_ms`](crate::schedule::stand_down_ms) (ten periods, at least twice the warm-up), so another node's
//! candidate holds the slots in the meantime.
//!
//! **What the client is told.** Besides its runs, the stream sends a `status` event each time what the loop says when it
//! runs nothing changes — `not_eligible` (this node does not match `holders`), `warming_up`, `not_holder`,
//! `stood_down`, `declaration_mismatch`, `last_ahead` — so a stream that hands out nothing says why.
//!
//! **A `GET` that changes state**, as `GET /gateway/rpc/serve/{kind}` is: opening the stream makes this node a candidate
//! and its answers record slots under `sched/`. On a gateway with a token model it needs `schedule:run`; on an open
//! gateway — only on loopback unless `gateway_allow_unauthenticated` — any local process, a web page's request
//! included, can open one, as it can open a serve stream.

use super::*;

use crate::capability_config::ScheduleDecl;
use crate::schedule::{spawn_schedule, LoopOptions, RunFailure, RunKind, ScheduledRun, SpawnError};

/// A run handed to a client and not yet acknowledged: the principal that opened the stream, and where its answer goes.
pub(super) struct PendingRun {
    principal: Arc<str>,
    answer: tokio::sync::mpsc::Sender<Result<(), String>>,
}

/// The runs handed out on every open stream, by run id (`{schedule}/{slot}/{random}`). Each entry is removed by the answer
/// that takes it, or when its run ends unanswered — overran or abandoned — so the table holds only runs in flight.
pub(super) type PendingRuns = papaya::HashMap<String, PendingRun>;

/// The seam stream a run id's random part is drawn on (a draw happens only when a client stream hands out a run).
const RUN_ID_STREAM: &str = "sched/run-id";

/// `GET /gateway/schedules/run?toml=…` — the query names one `[[schedule]]` in the unit file's own shape, and nothing
/// else.
#[derive(Deserialize)]
pub(super) struct ScheduleRunQuery {
    toml: String,
}

/// The stream's declaration: one `[[schedule]]` table. Any other section is refused by name (`deny_unknown_fields`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OneSchedule {
    #[serde(default)]
    schedule: Vec<ScheduleDecl>,
}

fn kind_name(kind: RunKind) -> &'static str {
    match kind {
        RunKind::OnTime => "on_time",
        RunKind::Late => "late",
        RunKind::ForGap { .. } => "for_gap",
    }
}

/// Removes a run's entry when its work future ends, by any path — acknowledged, abandoned at the deadline, dropped.
struct ClearRun {
    pending: Arc<PendingRuns>,
    run_id: String,
}

impl Drop for ClearRun {
    fn drop(&mut self) {
        self.pending.pin().remove(&self.run_id);
    }
}

/// `GET /gateway/schedules/run?toml=<one [[schedule]]>` — declare a schedule and receive its runs (Y12).
///
/// Events: `declared` once (`{"schedule", "node"}`), then a `run` per run this node decides while holding:
/// `{"run_id", "schedule", "slot", "kind": "on_time"|"late"|"for_gap", "first_slot", "slot_start_ms",
/// "operation_id"}`, and a `status` (`{"schedule", "reason"}`) each time what it says when it runs nothing changes.
/// Answer each run with `POST /gateway/schedules/ack`. Refused: an unreadable query or TOML, or a section other than
/// `[[schedule]]` (400, naming it); not exactly one `[[schedule]]`, or a declaration the unit-file loader refuses
/// (422, by name); an id this node already runs (409 `schedule_running`). Scope `schedule:run`.
pub(super) async fn gw_schedule_run(
    State(ctx): State<Arc<HttpCtx>>,
    caller: Option<Extension<ResolvedPrincipal>>,
    Query(q): Query<ScheduleRunQuery>,
) -> axum::response::Response {
    let parsed: OneSchedule = match toml::from_str(&q.toml) {
        Ok(p) => p,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid schedule", "detail": e.to_string()}))).into_response(),
    };
    let decl = match parsed.schedule.as_slice() {
        [one] => one.clone(),
        other => return (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({
            "error": "one schedule",
            "detail": format!("a run stream carries exactly one [[schedule]]; this declares {}", other.len()),
        }))).into_response(),
    };
    let principal: Arc<str> = Arc::from(
        caller.map(|Extension(c)| c.principal).unwrap_or_else(|| gateway_caller::PRINCIPAL_ANONYMOUS.to_string()).as_str(),
    );
    let scope = Arc::clone(&principal);
    // The stream's channel: one event at a time, so a run is handed out only once the one before it has been taken.
    let (tx, rx) = tokio::sync::mpsc::channel::<Event>(1);
    let pending = Arc::clone(&ctx.schedule_runs);
    let work_tx = tx.clone();
    let own_id: Arc<str> = Arc::from(decl.id.as_str());
    let work = move |run: ScheduledRun| {
        let (tx, pending, principal, own_id) = (work_tx.clone(), Arc::clone(&pending), Arc::clone(&principal), Arc::clone(&own_id));
        async move {
            // Unguessable, as `rpc/respond`'s nonce is: a run is answered by whoever holds its id and the principal it
            // was handed to, and under one shared bearer the principal alone tells two clients apart from nothing.
            let draw = mycelium_core::sim_seam::rng_u64_from(RUN_ID_STREAM, 0);
            let run_id = format!("{own_id}/{}/{draw:016x}", run.slot);
            let (answer, mut answered) = tokio::sync::mpsc::channel(1);
            pending.pin().insert(run_id.clone(), PendingRun { principal, answer });
            let _clear = ClearRun { pending, run_id: run_id.clone() };
            let event = Event::default().event("run").data(json!({
                "run_id":        run_id,
                "schedule":      own_id.as_ref(),
                "slot":          run.slot,
                "kind":          kind_name(run.kind),
                "first_slot":    run.covers().start(),
                "slot_start_ms": run.slot_start_ms,
                "operation_id":  run.operation_id().as_str(),
            }).to_string());
            if tx.send(event).await.is_err() {
                return Err(RunFailure::Unacknowledged("the client's stream had closed".into()));
            }
            tokio::select! {
                a = answered.recv() => match a {
                    Some(Ok(())) => Ok(()),
                    Some(Err(e)) => Err(RunFailure::Failed(e)),
                    None => Err(RunFailure::Unacknowledged("the run's answer was dropped".into())),
                },
                _ = tx.closed() => Err(RunFailure::Unacknowledged("the client's stream closed before it acknowledged".into())),
            }
        }
    };
    let id = decl.id.clone();
    // What the loop says when it runs nothing, as it changes: rare (once per change), so unbounded.
    let (status_tx, status_rx) = tokio::sync::mpsc::unbounded_channel::<&'static str>();
    let options = LoopOptions {
        stand_down_on_stall: true,
        client_principal: Some(scope),
        on_status: Some(Arc::new(move |reason| {
            let _ = status_tx.send(reason);
        })),
    };
    // One loop per id per node, whoever runs it — an application's `run_schedule` or another client's stream. The
    // check is the loop's own insert, so two streams racing for one id cannot both pass it.
    let handle = match spawn_schedule(&ctx.agent_ctx, &decl, options, work) {
        Ok(h) => h,
        Err(SpawnError::Running(e)) => {
            return (StatusCode::CONFLICT, Json(json!({
                "error": "schedule_running",
                "schedule": id,
                "detail": e.to_string(),
            }))).into_response();
        }
        Err(SpawnError::Invalid(e)) => {
            return (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error": "invalid schedule", "detail": e.to_string()}))).into_response();
        }
    };
    let declared = Event::default().event("declared").data(json!({
        "schedule": id,
        "node": ctx.agent_ctx.node_id.to_string(),
    }).to_string());
    // The stream owns the loop: dropping it — the client went away — drops the handle, which ends the loop and
    // withdraws the candidacy; a run handed out sees `tx.closed()` and is traced `unacknowledged`.
    drop(tx);
    let first = futures_util::stream::once(async move { Ok::<_, Infallible>(declared) });
    let runs = futures_util::stream::unfold((rx, handle), |(mut rx, handle)| async move {
        let event = rx.recv().await?;
        Some((Ok::<_, Infallible>(event), (rx, handle)))
    });
    let status_id = id.clone();
    let statuses = futures_util::stream::unfold(status_rx, move |mut status_rx| {
        let id = status_id.clone();
        async move {
            let reason = status_rx.recv().await?;
            let event = Event::default().event("status").data(json!({"schedule": id, "reason": reason}).to_string());
            Some((Ok::<_, Infallible>(event), status_rx))
        }
    });
    let events = futures_util::stream::select(runs, statuses);
    Sse::new(futures_util::StreamExt::chain(first, events)).keep_alive(KeepAlive::default()).into_response()
}

/// `POST /gateway/schedules/ack` — answer a run your own stream handed you.
///
/// Body: `{"run_id": "…", "ok": true}` or `{"run_id": "…", "ok": false, "error": "…"}`. `ok` records the slot as run;
/// an error is the work's failure (an on-time slot retried at the next tick). A run that was not handed to this
/// principal, has been answered, or ended without an answer is `403 unserved_run`, and nothing is recorded.
pub(super) async fn gw_schedule_ack(
    State(ctx): State<Arc<HttpCtx>>,
    caller: Option<Extension<ResolvedPrincipal>>,
    Json(body): Json<serde_json::Value>,
) -> axum::response::Response {
    let Some(run_id) = body["run_id"].as_str() else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "missing run_id"}))).into_response();
    };
    let Some(ok) = body["ok"].as_bool() else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "missing ok"}))).into_response();
    };
    let result = if ok { Ok(()) } else { Err(body["error"].as_str().unwrap_or("the client reported a failure").to_string()) };
    let principal = caller.map(|Extension(c)| c.principal).unwrap_or_else(|| gateway_caller::PRINCIPAL_ANONYMOUS.to_string());
    let unserved = || (StatusCode::FORBIDDEN, Json(json!({
        "ok": false,
        "error": "unserved_run",
        "message": "schedules/ack answers only a run your own schedules/run stream handed you and that is still waiting for its answer",
    }))).into_response();
    let answer = {
        let runs = ctx.schedule_runs.pin();
        match runs.get(run_id) {
            Some(p) if p.principal.as_ref() == principal => {
                let answer = p.answer.clone();
                // One answer per run: the first to take the entry answers; a second finds it gone and is refused.
                if runs.remove(run_id).is_none() {
                    return unserved();
                }
                answer
            }
            _ => return unserved(),
        }
    };
    // The run's work is still waiting (its entry was there), and the channel is empty: this send does not wait. A run
    // that ended between the two is refused, as one that ended before.
    if answer.send(result).await.is_err() {
        return unserved();
    }
    Json(json!({"ok": true})).into_response()
}
