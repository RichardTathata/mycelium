//! A2A (Agent-to-Agent) protocol adapter — `a2a` feature.
//!
//! Exposes these HTTP endpoints:
//! - `GET  /.well-known/agent-card.json` — A2A discovery (the AgentCard), where A2A 0.3 and 1.0 clients
//!   read it
//! - `GET  /.well-known/agent.json`      — the same card at the pre-0.3 path, **kept for one release**
//!   (`deprecations.md` §46)
//! - `POST /a2a`                         — JSON-RPC 2.0 task dispatch
//!
//! The card is built dynamically from the live `cap/` KV prefix so late-joining
//! nodes are immediately discoverable without re-configuration. It names `/a2a` once per protocol
//! version it serves (`supportedInterfaces`), so a current client picks the 1.0 methods.
//!
//! ## JSON-RPC methods
//!
//! Three generations of the protocol are served on one endpoint, and every send runs the same doors —
//! the federation edge, the task reservation, the action evaluator, the presented mandate, the
//! evidence record — through [`dispatch_send`] (plan row N). A task is answered in the shapes of the
//! generation that created it: `tasks/get` on a task `message/send` created answers in 0.3's shapes.
//!
//! | A2A 1.0                | A2A 0.3        | pre-0.2 (one release)  | Behaviour                                   |
//! |------------------------|----------------|------------------------|---------------------------------------------|
//! | `SendMessage`          | `message/send` | `tasks/send`           | resolve the skill, call it, return the task |
//! | `SendStreamingMessage` | `message/stream` | `tasks/sendSubscribe` | SSE: submitted → working → completed/failed |
//! | `GetTask`              | `tasks/get`    | `tasks/get`            | the task, to the identity that created it   |
//! | `CancelTask`           | `tasks/cancel` | `tasks/cancel`         | cancel a pending task; completed is refused |
//!
//! **Which skill.** The pre-0.2 methods take Mycelium's own `skillId` parameter. The 0.3 and 1.0 methods
//! read `metadata.skillId` — the message's, or the request's — and a message that names none reaches the
//! node's declared default skill (`GossipAgent::with_a2a_default_skill`), or is refused `-32602`. The
//! card's description says which. A presented mandate and an operation id ride where `tasks/send` takes
//! them in `params._meta`: in the request's `metadata._meta`. Each message is its own task: a message
//! naming a `taskId` is refused `-32004`, since every task here is complete when its send returns.

use axum::{
    Router,
    extract::State,
    response::{IntoResponse, Json, Sse},
    response::sse::{Event, KeepAlive},
    routing::{get, post},
};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    convert::Infallible,
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::warn;

use crate::agent::TaskCtx;
use super::gateway_caller::{self, GatewayDispatchError, ResolvedPrincipal};
use axum::Extension;
use crate::capability::CapFilter;
use crate::store::scan_kv_prefix;
use super::capability_ops::{is_cap_locality_key, parse_cap_key_or_warn};

// ── Wire types ────────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub(crate) struct AgentCard {
    pub name:         String,
    /// What the agent is and how a message reaches a skill here (which `metadata.skillId` names, or
    /// the default skill) — the place A2A gives an agent to say so.
    pub description:  String,
    /// `/a2a`, once per protocol version served. A 0.3 or 1.0 client reads this and ignores `url`.
    #[serde(rename = "supportedInterfaces")]
    pub supported_interfaces: Vec<AgentInterface>,
    /// The pre-0.3 field, kept for clients that read only it (it names `/a2a` too).
    pub url:          String,
    pub version:      String,
    pub capabilities: A2aCapabilities,
    #[serde(rename = "defaultInputModes")]
    pub default_input_modes: Vec<&'static str>,
    #[serde(rename = "defaultOutputModes")]
    pub default_output_modes: Vec<&'static str>,
    pub skills:       Vec<AgentSkill>,
}

#[derive(Serialize)]
pub(crate) struct AgentInterface {
    pub url: String,
    #[serde(rename = "protocolBinding")]
    pub protocol_binding: &'static str,
    #[serde(rename = "protocolVersion")]
    pub protocol_version: &'static str,
}

#[derive(Serialize)]
pub(crate) struct A2aCapabilities {
    pub streaming: bool,
}

#[derive(Serialize)]
pub(crate) struct AgentSkill {
    pub id:          String,
    pub name:        String,
    pub description: String,
    /// Machine-readable JSON Schema for the skill's input, taken from the
    /// gossiped `skills/{ns}/{name}/{node}/input` entry when present.
    /// Additive extension to the A2A card: tool-calling frameworks use it to
    /// build properly-typed tools instead of guessing field names from prose.
    #[serde(rename = "inputSchema", skip_serializing_if = "Option::is_none", default)]
    pub input_schema: Option<Value>,
    /// Required by A2A 0.3's card schema (a 0.3 client refused the card without it — #633 review):
    /// the skill's namespace.
    pub tags: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) struct Task {
    pub id:        String,
    pub status:    TaskStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<Artifact>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) struct TaskStatus {
    pub state: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) struct Artifact {
    pub parts: Vec<Part>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[allow(dead_code)]
pub(crate) struct Message {
    pub role:  String,
    pub parts: Vec<Part>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "lowercase")]
pub(crate) enum Part {
    Text { text: String },
}

// ── Protocol generations ──────────────────────────────────────────────────────

/// The generation of the A2A protocol a method belongs to, which fixes the shapes its answers take
/// (plan row N). The pre-0.2 shapes are the ones `/a2a` has always answered; 0.3 tags each object
/// with a `kind` and names states in lower case; 1.0 is the protobuf JSON form, with
/// `TASK_STATE_*` names and a send's task wrapped as `{ "task": … }`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Generation {
    /// `tasks/send`, `tasks/sendSubscribe` — and `tasks/get`/`tasks/cancel` on a task they created.
    Legacy,
    /// `message/send`, `message/stream` — and `tasks/get`/`tasks/cancel` on a task they created.
    V03,
    /// `SendMessage`, `SendStreamingMessage`, `GetTask`, `CancelTask`.
    V10,
}

/// The protocol versions the card names for `/a2a`, newest first: a client takes the first it speaks.
const PROTOCOL_VERSIONS: [&str; 2] = ["1.0", "0.3"];

/// A task state as the generation spells it: the stored (pre-0.2) name, the same in 0.3, and
/// `TASK_STATE_*` in 1.0.
fn state_name(state: &str, generation: Generation) -> String {
    match generation {
        Generation::Legacy | Generation::V03 => state.to_string(),
        // `input-required` is `TASK_STATE_INPUT_REQUIRED` in 1.0: the hyphen becomes an underscore (#679's review).
        Generation::V10 => format!("TASK_STATE_{}", state.to_ascii_uppercase().replace('-', "_")),
    }
}

/// A text part: `{type: text}` before 0.2, `{kind: text}` in 0.3, the bare `{text}` in 1.0.
fn text_part(text: &str, generation: Generation) -> Value {
    match generation {
        Generation::Legacy => json!({ "type": "text", "text": text }),
        Generation::V03 => json!({ "kind": "text", "text": text }),
        Generation::V10 => json!({ "text": text }),
    }
}

/// The artifact a completed task carries, in the generation's shape. 0.3 and 1.0 require an
/// `artifactId`; a task has at most one artifact here, so it is derived from the task id.
fn render_artifact(task_id: &str, index: usize, artifact: &Artifact, generation: Generation) -> Value {
    let parts: Vec<Value> = artifact
        .parts
        .iter()
        .map(|Part::Text { text }| text_part(text, generation))
        .collect();
    match generation {
        Generation::Legacy => json!({ "parts": parts }),
        Generation::V03 | Generation::V10 => json!({ "artifactId": format!("{task_id}-{index}"), "parts": parts }),
    }
}

/// A task in the generation's shape. The pre-0.2 shape is the serialised [`Task`], unchanged.
fn render_task(task: &Task, context_id: &str, generation: Generation) -> Value {
    if generation == Generation::Legacy {
        return serde_json::to_value(task).unwrap_or(Value::Null);
    }
    let mut v = json!({
        "id": task.id,
        "contextId": context_id,
        "status": { "state": state_name(&task.status.state, generation) },
    });
    if generation == Generation::V03 {
        v["kind"] = json!("task");
    }
    if !task.artifacts.is_empty() {
        v["artifacts"] = Value::Array(
            task.artifacts.iter().enumerate().map(|(i, a)| render_artifact(&task.id, i, a, generation)).collect(),
        );
    }
    v
}

/// A send's result: the task itself before 1.0, `{ "task": … }` in 1.0 (`SendMessageResponse`).
fn render_send_result(task: &Task, context_id: &str, generation: Generation) -> Value {
    let rendered = render_task(task, context_id, generation);
    match generation {
        Generation::V10 => json!({ "task": rendered }),
        Generation::Legacy | Generation::V03 => rendered,
    }
}

/// One send, whatever generation asked for it: the shape [`dispatch_send`] and the stream run on.
#[derive(Clone)]
pub(crate) struct SendRequest {
    pub task_id:    String,
    pub context_id: String,
    pub skill_id:   String,
    /// What the skill receives: the first part's text, or its `data` re-serialised as compact JSON —
    /// keys sorted and numbers as `serde_json` holds them (an integer past `u64`/`i64` becomes an `f64`), so
    /// a `data` part does not reach the skill byte for byte, and a mandate's possession proof digests
    /// this form (guide 08, `deprecations.md` §46; #633 review, finding 3).
    pub text:       String,
    /// What the AE preflight and the mandate reader read: `{ "_meta": … }`, from `params._meta`
    /// before 0.3 and from the request's `metadata._meta` after.
    pub meta:       Value,
    pub generation: Generation,
}

impl SendRequest {
    /// A pre-0.2 `tasks/send` / `tasks/sendSubscribe`: the caller names the task id and the skill.
    fn legacy(state: &A2aState, params: &Value) -> Self {
        let task_id = params.get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| new_task_id(&state.task_ctx.hlc));
        let skill_id = params.get("skillId")
            .or_else(|| params.get("skill_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let text = text_from_message(params.get("message").unwrap_or(&Value::Null));
        SendRequest {
            context_id: task_id.clone(),
            task_id,
            skill_id,
            text,
            meta: params.clone(),
            generation: Generation::Legacy,
        }
    }

    /// A 0.3 `message/send` / `message/stream` or a 1.0 `SendMessage` / `SendStreamingMessage`
    /// (`params` is `MessageSendParams` / `SendMessageRequest`; the two agree on every field read
    /// here). `Err` is the JSON-RPC error to answer, before anything is reserved or dispatched.
    fn current(state: &A2aState, id: &Option<Value>, params: &Value, generation: Generation) -> Result<Self, Value> {
        let invalid = |why: &str| jsonrpc_error(id.clone(), -32602, why);
        let Some(message) = params.get("message").filter(|m| m.is_object()) else {
            return Err(invalid("params.message is required"));
        };
        // Every task here is complete when its send returns, so there is no task to continue: a
        // `taskId` would otherwise have to mean either "replace that completed task" (the pre-0.2
        // re-send) or nothing at all.
        if message.get("taskId").and_then(Value::as_str).is_some_and(|t| !t.is_empty()) {
            return Err(jsonrpc_error(
                id.clone(),
                -32004,
                "each message is its own task at this agent and its task is complete when the send returns: \
                 send without taskId (name an earlier task in referenceTaskIds)",
            ));
        }
        let named = |holder: Option<&Value>| -> Result<Option<String>, Value> {
            match holder.and_then(|m| m.get("metadata")).and_then(|m| m.get("skillId")) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(s)) => Ok(Some(s.clone())),
                Some(_) => Err(invalid("metadata.skillId must be a string \"ns/name\"")),
            }
        };
        let skill_id = match (named(Some(message))?, named(Some(params))?) {
            (Some(a), Some(b)) if a != b => {
                return Err(invalid("the message's and the request's metadata.skillId name different skills"))
            }
            (Some(a), _) | (None, Some(a)) => a,
            (None, None) => match state.task_ctx.a2a_default_skill.get() {
                Some(default) => default.clone(),
                None => {
                    return Err(invalid(
                        "name the skill in metadata.skillId (\"ns/name\", one the agent card lists): \
                         this agent declares no default skill",
                    ))
                }
            },
        };
        let text = match message.get("parts").and_then(Value::as_array).and_then(|p| p.first()) {
            None => return Err(invalid("the message has no parts")),
            Some(part) => match (part.get("text"), part.get("data")) {
                (Some(Value::String(t)), _) => t.clone(),
                (_, Some(data)) => data.to_string(),
                _ => {
                    return Err(jsonrpc_error(
                        id.clone(),
                        -32005,
                        "a skill here reads a text or a data part; this message's first part is neither",
                    ))
                }
            },
        };
        let context_id = message.get("contextId")
            .and_then(Value::as_str)
            .filter(|c| !c.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| new_task_id(&state.task_ctx.hlc));
        let meta = params.get("metadata").and_then(|m| m.get("_meta")).cloned().unwrap_or(Value::Null);
        Ok(SendRequest {
            task_id: new_task_id(&state.task_ctx.hlc),
            context_id,
            skill_id,
            text,
            meta: json!({ "_meta": meta }),
            generation,
        })
    }
}

// ── In-memory task store ──────────────────────────────────────────────────────

pub(crate) struct A2aTask {
    pub task:       Task,
    /// The A2A context the task belongs to: the message's `contextId`, or one minted for it. The
    /// pre-0.2 shapes have no context and do not carry it.
    pub context_id: String,
    /// The protocol generation whose method created the task; `tasks/get` and `tasks/cancel`, which
    /// pre-0.2 and 0.3 share by name, answer in its shapes.
    pub generation: Generation,
    pub created_at: Instant,
    /// The principal that created the task (`tasks/send` / `tasks/sendSubscribe`), or `None` for an
    /// anonymous caller. `tasks/get` and `tasks/cancel` answer **only this identity**: ids are
    /// caller-chosen and therefore enumerable, and before this field any client could read another
    /// caller's completed artifact or cancel its task by naming the id. An anonymous task has no
    /// identity to answer to, so it is read only on the response that created it.
    pub owner:      Option<String>,
    /// `Some(token)` while the task is a **reservation** — its id claimed before the dispatch, so a
    /// second sender under the same id is refused rather than racing it (#587 F5). The completed
    /// task replaces it with `None`; a reservation whose send ends without one is removed by
    /// [`TaskReservation`]'s `Drop`, only if the token is still its own.
    pub pending:    Option<u64>,
}

/// Reservation tokens: process-unique, never reused (not a draw — the replay seam holds no RNG here).
static RESERVATION_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// A task id claimed for one send. Dropped without [`complete`](Self::complete) — a refusal, a
/// failed dispatch, an unwind — it removes the pending entry if that entry is still this one.
pub(crate) struct TaskReservation {
    tasks: Arc<papaya::HashMap<String, A2aTask>>,
    id:    String,
    context_id: String,
    generation: Generation,
    owner: Option<String>,
    /// `None` when the id already belonged to this caller (a re-send): nothing of ours to remove.
    token: Option<u64>,
}

impl TaskReservation {
    /// Store the completed task under the reserved id; the reservation is spent.
    fn complete(mut self, task: Task) {
        self.token = None;
        self.tasks.pin().insert(self.id.clone(), A2aTask {
            task,
            context_id: self.context_id.clone(),
            generation: self.generation,
            created_at: mycelium_core::sim_seam::mono_instant(),
            owner: self.owner.clone(),
            pending: None,
        });
    }
}

impl Drop for TaskReservation {
    fn drop(&mut self) {
        let Some(token) = self.token else { return };
        // Conditional: the entry may since be a completed task or a cancel's removal; touch only ours.
        self.tasks.pin().compute(self.id.clone(), |existing| match existing {
            Some((_, t)) if t.pending == Some(token) => papaya::Operation::Remove,
            _ => papaya::Operation::Abort(()),
        });
    }
}

/// Claim `task_id` for `owner` **before** any dispatch: a new id gets a pending entry owned by the
/// caller (atomically — `compute`, so two senders racing one new id cannot both pass); an id this
/// identity already owns is a re-send; an id another identity owns is `Err`. The check and the
/// claim are one step, so the dispatch `await` no longer sits between them.
fn reserve_task(
    state:      &A2aState,
    task_id:    &str,
    context_id: &str,
    generation: Generation,
    owner:      &Option<String>,
) -> Result<TaskReservation, Unclaimable> {
    let token = RESERVATION_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let guard = state.tasks.pin();
    let claimed = guard.compute(task_id.to_string(), |existing| match existing {
        Some((_, t)) if t.owner != *owner => papaya::Operation::Abort(Err(Unclaimable::Foreign)),
        // A re-send is a pre-0.2 notion: a task a 0.3 or 1.0 method created is its own message's,
        // and a `tasks/send` naming its id must not run the skill again and replace it (#633 review).
        Some((_, t)) if t.generation != Generation::Legacy || generation != Generation::Legacy => {
            papaya::Operation::Abort(Err(Unclaimable::NotResendable))
        }
        Some(_) => papaya::Operation::Abort(Ok(())),
        None => papaya::Operation::Insert(A2aTask {
            task: Task { id: task_id.to_string(), status: TaskStatus { state: "submitted".into() }, artifacts: vec![] },
            context_id: context_id.to_string(),
            generation,
            created_at: mycelium_core::sim_seam::mono_instant(),
            owner: owner.clone(),
            pending: Some(token),
        }),
    });
    let token = match claimed {
        papaya::Compute::Inserted(..) => Some(token),
        papaya::Compute::Aborted(Ok(())) => None,
        papaya::Compute::Aborted(Err(why)) => return Err(why),
        _ => return Err(Unclaimable::Foreign),
    };
    Ok(TaskReservation {
        tasks: Arc::clone(&state.tasks),
        id: task_id.to_string(),
        context_id: context_id.to_string(),
        generation,
        owner: owner.clone(),
        token,
    })
}

/// Why a send could not claim its task id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unclaimable {
    /// Another identity owns it (or an anonymous caller's, which no one can name).
    Foreign,
    /// It is a task a 0.3 or 1.0 method created, which is never re-sent.
    NotResendable,
}

impl Unclaimable {
    fn refusal(self, id: Option<Value>) -> Value {
        match self {
            Unclaimable::Foreign => not_the_creator(id),
            Unclaimable::NotResendable => jsonrpc_error(
                id,
                -32004,
                "this task was created by message/send or SendMessage and is its own message's: send a new message instead",
            ),
        }
    }
}

/// The identity a task is owned by: the resolved principal, or `None` for an anonymous caller (and
/// for a handler reached without the auth layer, as in a bare-router unit test).
fn task_owner(caller: Option<&ResolvedPrincipal>) -> Option<String> {
    caller.filter(|c| c.principal != gateway_caller::PRINCIPAL_ANONYMOUS).map(|c| c.principal.clone())
}

/// The refusal `tasks/get`, `tasks/cancel` and a re-send under an existing id answer anyone but the
/// identity that created the task — the code the federated branch already uses for the same rule.
fn not_the_creator(id: Option<Value>) -> Value {
    jsonrpc_error(
        id,
        -32004,
        "a task is read (tasks/get, GetTask), cancelled (tasks/cancel, CancelTask) or re-sent under its id only by the identity that created it; an anonymous task is answered only on the response that created it",
    )
}


// ── Router context ────────────────────────────────────────────────────────────

#[derive(Clone)]
pub(crate) struct A2aState {
    pub task_ctx: Arc<TaskCtx>,
    pub tasks:    Arc<papaya::HashMap<String, A2aTask>>,
}

// ── Public constructor ────────────────────────────────────────────────────────

/// Spawns a background task that evicts A2A tasks older than 5 minutes, until the agent shuts
/// down. Call this once after creating the shared `tasks` map. It ran forever before row B
/// (post-360 hardening), holding the task map of a stopped agent.
pub(crate) fn spawn_cleanup(
    tasks: Arc<papaya::HashMap<String, A2aTask>>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    tokio::spawn(async move {
        // Through the timer seam (item 6). Tokio's default missed-tick behaviour, as before.
        let mut interval = mycelium_core::sim_seam::interval_ms(
            "a2a/sweep",
            60_000,
            tokio::time::MissedTickBehavior::Burst,
        );
        loop {
            tokio::select! { biased;
                _ = shutdown.wait_for(|v| *v) => break,
                _ = interval.tick() => {}
            }
            evict_stale_tasks(&tasks, Instant::now());
        }
    });
}

const A2A_TASK_TTL: Duration = Duration::from_secs(300);

/// One eviction sweep over `tasks`: removes entries older than
/// [`A2A_TASK_TTL`] as of `now`. The removal is a conditional `compute`:
/// a status update can re-insert a task id with a fresh `created_at`
/// between collect and removal, and an unconditional `remove()` would evict
/// the live task — clients polling it would get NotFound (M2 Run-18 sweep
/// finding, same family as the tombstone-GC conditional remove).
fn evict_stale_tasks(tasks: &papaya::HashMap<String, A2aTask>, now: Instant) {
    let to_remove: Vec<String> = tasks.pin().iter()
        .filter(|(_, v)| now.duration_since(v.created_at) > A2A_TASK_TTL)
        .map(|(k, _)| k.clone().to_string())
        .collect();
    let guard = tasks.pin();
    for k in to_remove {
        guard.compute(k, |existing| match existing {
            Some((_, v)) if now.duration_since(v.created_at) > A2A_TASK_TTL =>
                papaya::Operation::Remove,
            _ => papaya::Operation::Abort(()),
        });
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn new_task_id(hlc: &crate::hlc::Hlc) -> String {
    format!("{:016x}-{:016x}", hlc.tick(), fastrand::u64(1..))
}

fn text_from_message(msg: &Value) -> String {
    msg.get("parts")
        .and_then(|p| p.as_array())
        .and_then(|arr| arr.first())
        .and_then(|part| part.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string()
}

fn completed_task(id: String, reply: Bytes) -> Task {
    let text = String::from_utf8_lossy(&reply).into_owned();
    Task {
        id,
        status:    TaskStatus { state: "completed".into() },
        artifacts: vec![Artifact { parts: vec![Part::Text { text }] }],
    }
}

fn jsonrpc_error(id: Option<Value>, code: i32, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

/// A JSON-RPC error carrying a machine-readable `data` block.
///
/// Used for AE refusals, which a caller has to be able to *act on* differently: a denial and an
/// authority that was never established call for opposite responses, and neither is legible from a
/// prose message without parsing English.
///
/// Its only call site is the AE preflight's refusal branch, which is itself `cfg`'d on
/// `gateway` + `tls` — so in an `a2a`-without-`tls` build (CI's Demo smoke) this is dead, and
/// the gate must say so rather than leave a standing warning.
#[cfg(all(feature = "gateway", feature = "tls"))]
fn jsonrpc_error_with_data(
    id: Option<Value>,
    code: i32,
    message: &str,
    data: Value,
) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message, "data": data }
    })
}

fn jsonrpc_ok(id: Option<Value>, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// Resolves the first node for `skill_id` ("ns/name") from the live cap/ KV prefix, or says why not.
fn resolve_skill(ctx: &TaskCtx, skill_id: &str) -> Result<crate::node_id::NodeId, String> {
    let Some((ns, name)) = skill_id.split_once('/') else { return Err("skill not found".into()) };
    if let Some(why) = not_an_a2a_skill(&ctx.kv_state, ns, name) {
        return Err(why);
    }
    let filter = CapFilter::new(ns, name);
    let providers = resolve_providers(ctx, &filter);
    providers.into_iter().next().map(|(n, _)| n).ok_or_else(|| "skill not found".into())
}

fn resolve_providers(
    ctx:    &TaskCtx,
    filter: &CapFilter,
) -> Vec<(crate::node_id::NodeId, crate::capability::Capability)> {
    use crate::capability::Capability;
    let mut out = Vec::new();
    for (key, bytes) in scan_kv_prefix(&ctx.kv_state, "cap/") {
        if is_cap_locality_key(&key) { continue; }
        let Some((node_id, _ns, _name)) = parse_cap_key_or_warn("cap/", &key) else { continue };
        let Some(cap) = Capability::decode(&bytes) else { continue };
        if filter.matches(&cap) {
            out.push((node_id, cap));
        }
    }
    out
}

/// Builds a human/LLM-readable description for a skill from its gossiped
/// input schema (`skills/{ns}/{name}/{node}/input`, published by SkillRunner).
/// Tool-calling frameworks read the card's `description` to decide how to
/// invoke a skill — an empty description left LangChain agents passing plain
/// text to skills that require a JSON object (observed live: the orchestrator
/// rejected `"CRDTs"` with a JSON parse error and the agent silently fell
/// back to answering from its own weights).
fn skill_input_description(kv: &crate::store::KvState, skill_id: &str) -> (String, Option<Value>) {
    let prefix = format!("skills/{skill_id}/");
    for (key, bytes) in scan_kv_prefix(kv, &prefix) {
        if !key.ends_with("/input") { continue; }
        let Ok(schema) = serde_json::from_slice::<Value>(&bytes) else { continue };
        let required: Vec<&str> = schema.get("required")
            .and_then(|r| r.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        let props = schema.get("properties").and_then(|p| p.as_object());
        let mut fields: Vec<String> = Vec::new();
        if let Some(props) = props {
            for (name, spec) in props {
                let ty   = spec.get("type").and_then(|t| t.as_str()).unwrap_or("any");
                let desc = spec.get("description").and_then(|d| d.as_str()).unwrap_or("");
                let req  = if required.contains(&name.as_str()) { "required" } else { "optional" };
                if desc.is_empty() {
                    fields.push(format!("{name} ({ty}, {req})"));
                } else {
                    fields.push(format!("{name} ({ty}, {req}): {desc}"));
                }
            }
        }
        let text = if fields.is_empty() {
            format!("Mycelium mesh skill {skill_id}. Input: a JSON object.")
        } else {
            format!(
                "Mycelium mesh skill {skill_id}. Input MUST be a JSON object with: {}.",
                fields.join("; "),
            )
        };
        return (text, Some(schema));
    }
    (String::new(), None)
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// Whether an advertised capability is the fleet's plumbing rather than a skill an agent can be asked
/// for: a provisioning tier (`{ns}/loading`, `{ns}/installable`), a stem's shed mark (`prov-shed/*`),
/// model metadata (`llm-meta/*`), the artifact librarian (`artifact/librarian`), the reason blob cache
/// (`reason/blob-cache`), and a companion's election and role marks (`{ns}/{x}.primary`, `.secondary`,
/// `.candidate`, `.curator` — tuple space, blackboard, wiki) — each serves its own RPC or none, never
/// `skill.invoke`. A new kind of plumbing capability is added here, so the agent card does not list it.
pub fn is_infrastructure_capability(ns: &str, name: &str) -> bool {
    matches!(ns, "prov-shed" | "llm-meta")
        || matches!(name, "loading" | "installable")
        || (ns, name) == ("artifact", "librarian")
        || (ns, name) == ("reason", "blob-cache")
        || [".primary", ".secondary", ".candidate", ".curator"].iter().any(|s| name.ends_with(s))
}

/// Why `/a2a` cannot call `{ns}/{name}`, or `None` when it can. `/a2a` sends `skill.invoke`; plumbing answers
/// none, and a prompt skill (`register_prompt_skill`, with its template at `prompts/{ns}/{name}`) answers
/// `llm.invoke`, which is reached through `POST /gateway/llm/call` under `llm:invoke` — routing it here
/// would let an A2A caller reach a model without that scope. The card and `tasks/send` both use this, so
/// the card lists exactly what `tasks/send` will call.
fn not_an_a2a_skill(kv: &crate::store::KvState, ns: &str, name: &str) -> Option<String> {
    if is_infrastructure_capability(ns, name) {
        return Some("skill not found".into());
    }
    // A pair that cannot name a prompt skill has no template to find.
    let prompt_key = super::helpers::prompt_kv_key(ns, name)?;
    if kv.store.pin().get(prompt_key.as_str()).and_then(|e| e.data.clone()).is_some_and(|b| !b.is_empty()) {
        return Some(format!("{ns}/{name} is a prompt skill: call it through POST /gateway/llm/call (llm:invoke)"));
    }
    None
}

/// The skill ids an agent card lists: every advertised capability that is not infrastructure, once.
fn card_skill_ids(kv: &crate::store::KvState) -> std::collections::HashSet<String> {
    let mut skill_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (key, bytes) in scan_kv_prefix(kv, "cap/") {
        if is_cap_locality_key(&key) { continue; }
        let Some((_node_id, ns, name)) = parse_cap_key_or_warn("cap/", &key) else { continue };
        if not_an_a2a_skill(kv, &ns, &name).is_some() { continue; }
        use crate::capability::Capability;
        if Capability::decode(&bytes).is_some() {
            skill_ids.insert(format!("{}/{}", ns, name));
        }
    }
    skill_ids
}

/// The URL an A2A client reaches this node's `/a2a` at: the gateway's scheme (`https` under
/// `gateway_tls`), the advertised host and port, and `/a2a`. It used to be the node's **gossip**
/// address with no path, which an A2A client following the card could not use (plan row N).
///
/// The host is `http_addr` when the gateway binds one address. A gateway bound to every interface
/// (`0.0.0.0`, `::`) is advertised at the node's own address, the one its peers already reach it by.
fn a2a_endpoint_url(ctx: &TaskCtx) -> String {
    let cfg = &ctx.config;
    let scheme = if cfg.gateway_tls.is_some() { "https" } else { "http" };
    let node_ip = ctx.node_id.to_socket_addr().ip();
    let ip = match cfg.http_addr.parse::<std::net::IpAddr>() {
        Ok(ip) if !ip.is_unspecified() => ip,
        _ => node_ip,
    };
    let host = match ip {
        std::net::IpAddr::V6(v6) => format!("[{v6}]"),
        std::net::IpAddr::V4(v4) => v4.to_string(),
    };
    let port = cfg.http_port.unwrap_or_else(|| ctx.node_id.to_socket_addr().port());
    format!("{scheme}://{host}:{port}/a2a")
}

async fn agent_card_handler(State(state): State<A2aState>) -> impl IntoResponse {
    let kv = &state.task_ctx.kv_state;
    let skill_ids = card_skill_ids(kv);
    let skills: Vec<AgentSkill> = skill_ids.into_iter().map(|id| {
        let (description, input_schema) = skill_input_description(kv, &id);
        let tags = id.split_once('/').map(|(ns, _)| vec![ns.to_string()]).unwrap_or_default();
        AgentSkill { name: id.clone(), id, description, input_schema, tags }
    }).collect();

    let node_addr = state.task_ctx.node_id.to_string();
    let endpoint = a2a_endpoint_url(&state.task_ctx);
    let routing = match state.task_ctx.a2a_default_skill.get() {
        Some(default) => format!(
            "A message names its skill (\"ns/name\", one of the skills below) in metadata.skillId, the message's \
             or the request's; a message that names none goes to {default}."
        ),
        None => "A message names its skill (\"ns/name\", one of the skills below) in metadata.skillId, the \
                 message's or the request's; one that names none is refused."
            .to_string(),
    };
    let card = AgentCard {
        name:         format!("Mycelium/{}", node_addr),
        description:  format!(
            "Mycelium node {node_addr}: the skills its mesh advertises. {routing} A skill reads the first part \
             of the message, text or data. Each message is its own task."
        ),
        supported_interfaces: PROTOCOL_VERSIONS
            .iter()
            .map(|v| AgentInterface { url: endpoint.clone(), protocol_binding: "JSONRPC", protocol_version: v })
            .collect(),
        url:          endpoint,
        version:      "1.0.0".into(),
        capabilities: A2aCapabilities { streaming: true },
        default_input_modes: vec!["text/plain", "application/json"],
        default_output_modes: vec!["text/plain"],
        skills,
    };
    Json(card)
}

/// **The one send** (plan row N): `tasks/send`, `message/send` and `SendMessage` each parse their
/// request into a [`SendRequest`] and come here, so every generation runs the same doors in the same
/// order — the federation edge, the task reservation, skill resolution, the action evaluator, the
/// presented mandate travelling to the provider, the evidence record. `Ok` is the completed task, its
/// reservation spent; `Err` is the JSON-RPC error to answer.
///
/// `raw` is the request body **exactly as it arrived**, before parsing. A federated credential may
/// bind it (item 2 row 11), and that binding is worth nothing if it is checked against a
/// re-serialisation: two encodings of the same JSON are the same request and different bytes.
async fn dispatch_send(
    state:  &A2aState,
    caller: Option<&ResolvedPrincipal>,
    #[cfg(feature = "tls")]
    federated: Option<&super::federation_http::FederatedIdentity>,
    id:     Option<Value>,
    req:    &SendRequest,
    #[cfg_attr(not(feature = "tls"), allow(unused_variables))]
    raw:    &[u8],
) -> Result<Task, Value> {
    let skill_id = req.skill_id.as_str();
    let params = &req.meta;

    // Item 2 PR 8: a federated caller was authenticated at the auth layer; the export it named is
    // authorised here, now that the body has said which skill it is asking for. The credential
    // binds *what*, not only *who* — a credential for one export refused for another (PR 4).
    // The partner's slot at this gateway, held for the dispatch and released on drop — on the
    // reply, on a refusal further in, or on an unwind. `None` for a call that is not federated.
    #[cfg(feature = "tls")]
    let mut _slot: Option<crate::federation::edge::PartnerSlot> = None;
    #[cfg(feature = "tls")]
    if let Some(federated) = federated {
        let Some(edge) = state.task_ctx.federation_edge.get() else {
            return Err(jsonrpc_error(id, -32003, "federation is not enabled at this gateway"));
        };
        let accepted =
            match edge.authorize(&federated.presented, skill_id, raw, crate::federation::edge::now_ms()) {
                Ok(accepted) => accepted,
                Err(refusal) => return Err(jsonrpc_error(id, -32003, &format!("federated call refused: {refusal}"))),
            };
        // Capacity is asked **after** authority, so a partner we would refuse on authority cannot
        // occupy a slot belonging to one we would admit. Its own code: a capacity refusal is
        // transient and a retry resolves it, while -32003 says go and talk to your operator.
        match edge.admit(&accepted.origin_domain) {
            Ok(slot) => _slot = Some(slot),
            Err(refusal) => return Err(jsonrpc_error(id, -32004, &format!("federated call refused: {refusal}"))),
        }
    }
    // The id is claimed **after** the federation authorisation (#587 F6: an unauthorised partner
    // learns nothing about which ids exist) and **before** the dispatch (F5: two senders racing one
    // new id cannot both run). An id another identity owns is refused here, before anything is sent.
    let reservation = match reserve_task(state, &req.task_id, &req.context_id, req.generation, &task_owner(caller)) {
        Ok(reservation) => reservation,
        Err(why) => return Err(why.refusal(id)),
    };
    let text = req.text.clone();

    let target = match resolve_skill(&state.task_ctx, skill_id) {
        Ok(target) => target,
        Err(why) => return Err(jsonrpc_error(id, unresolved_skill_code(req.generation), &why)),
    };

    // Generous: A2A skills are frequently multi-step LLM pipelines
    // (orchestrator → researcher → writer takes ~90 s on local Ollama);
    // a 30 s cap made every such composition fail with -32603 while the
    // pipeline was still working. Clients enforce their own HTTP timeouts.
    let timeout = Duration::from_secs(120);
    // AE slice: the same evaluator preflight the MCP edge runs. `/a2a` is the *other* route a
    // foreign agent acts through, so leaving it unguarded would mean the enforcement point could be
    // walked around by choosing a different door — and the evidence would still have said
    // `coverage.complete: false`, truthfully, while the remit went unenforced.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    let preflight = super::http::ae_preflight(
        &state.task_ctx,
        caller,
        "skill.invoke",
        &format!("skill:{skill_id}@{target}"),
        &json!({ "text": text }),
        params,
        super::http::ENFORCEMENT_POINT_A2A,
    )
    .await;
    #[cfg(all(feature = "gateway", feature = "tls"))]
    if let super::http::Preflight::Refuse(refusal) = &preflight {
        // The same body `/mcp` sends. A partner domain calling across the federation edge gets the
        // machine-readable reason and the policy revision, not only a number and a sentence.
        return Err(jsonrpc_error_with_data(
            id,
            refusal.json_rpc_code(),
            &refusal.to_string(),
            refusal.error_data(),
        ));
    }

    // Item 7: the skill provider is told who called (the resolved bearer principal, or
    // `anonymous`), never just "the gateway node".
    // Closure plan C2/C3: the presented mandate, and the resource it is presented for, travel to the
    // provider, which verifies them itself.
    let resource_claim = format!("skill:{skill_id}@{target}");
    let dispatched = gateway_caller::gateway_rpc_call_with_mandate(
        &state.task_ctx, caller, target,
        "skill.invoke".into(), Bytes::from(text.into_bytes()), timeout, gateway_caller::presented_mandate(params),
        Some(&resource_claim),
    ).await;

    // A timeout is *unknown*, never a negative — a long-running skill may well have completed.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    {
        // The same mapping the MCP edge uses — a skill reply is raw output, not JSON-RPC, so there
        // is no error envelope to inspect.
        let observed = super::http::observed_execution(&dispatched);
        super::http::ae_record_execution(&state.task_ctx, &preflight, observed).await;
    }

    match dispatched {
        Ok(reply) => {
            let task = completed_task(req.task_id.clone(), reply);
            reservation.complete(task.clone());
            Ok(task)
        }
        Err(GatewayDispatchError::Rpc(e)) => {
            warn!("A2A send rpc_call failed for skill {}: {:?}", skill_id, e);
            Err(jsonrpc_error(id, -32603, "rpc call failed"))
        }
        Err(e) => {
            warn!("A2A send refused for skill {}: {e}", skill_id);
            Err(jsonrpc_error(id, e.json_rpc_code(), &e.to_string()))
        }
    }
}

/// The code a send answers for a skill it cannot resolve: `-32001` before 0.3, as it always has been;
/// `-32602` (invalid params) after, because 0.3 and 1.0 give `-32001` to *task* not found and a
/// client would read it as that.
fn unresolved_skill_code(generation: Generation) -> i32 {
    match generation {
        Generation::Legacy => -32001,
        Generation::V03 | Generation::V10 => -32602,
    }
}

/// A send in any generation, answered in its shapes.
async fn handle_send(
    state:  &A2aState,
    caller: Option<&ResolvedPrincipal>,
    #[cfg(feature = "tls")]
    federated: Option<&super::federation_http::FederatedIdentity>,
    id:     Option<Value>,
    req:    &SendRequest,
    raw:    &[u8],
) -> Value {
    match dispatch_send(
        state,
        caller,
        #[cfg(feature = "tls")]
        federated,
        id.clone(),
        req,
        raw,
    )
    .await
    {
        Ok(task) => jsonrpc_ok(id, render_send_result(&task, &req.context_id, req.generation)),
        Err(error) => error,
    }
}


/// `tasks/get` / `GetTask`: the task, **to the identity that created it**. Anyone else — another
/// principal, or an anonymous caller, who has no identity to match — is refused `-32004` naming the
/// rule. `asked` is the generation of the method (`GetTask` is 1.0); `None` for `tasks/get`, which
/// pre-0.2 and 0.3 share, so the task answers in the shapes of the generation that created it.
fn handle_tasks_get(
    state:  &A2aState,
    caller: Option<&ResolvedPrincipal>,
    id:     Option<Value>,
    params: &Value,
    asked:  Option<Generation>,
) -> Value {
    let task_id = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let owner = task_owner(caller);
    match state.tasks.pin().get(task_id) {
        Some(entry) if owner.is_some() && entry.owner == owner => {
            jsonrpc_ok(id, render_task(&entry.task, &entry.context_id, asked.unwrap_or(entry.generation)))
        }
        Some(_) => not_the_creator(id),
        None    => jsonrpc_error(id, -32001, "task not found"),
    }
}

/// `tasks/cancel` / `CancelTask`: the same owner rule as `tasks/get`, checked before the task's state,
/// and the same `asked`.
fn handle_tasks_cancel(
    state:  &A2aState,
    caller: Option<&ResolvedPrincipal>,
    id:     Option<Value>,
    params: &Value,
    asked:  Option<Generation>,
) -> Value {
    let task_id = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let owner = task_owner(caller);
    let guard = state.tasks.pin();
    match guard.get(task_id) {
        Some(entry) if owner.is_none() || entry.owner != owner => not_the_creator(id),
        Some(entry) if entry.task.status.state == "completed" => {
            jsonrpc_error(id, -32002, "task already completed")
        }
        Some(entry) => {
            let generation = asked.unwrap_or(entry.generation);
            let canceled = Task { id: task_id.to_string(), status: TaskStatus { state: "canceled".into() }, artifacts: vec![] };
            let answer = match generation {
                Generation::Legacy => json!({ "id": task_id, "status": { "state": "canceled" } }),
                Generation::V03 | Generation::V10 => render_task(&canceled, &entry.context_id, generation),
            };
            guard.remove(task_id);
            jsonrpc_ok(id, answer)
        }
        None => jsonrpc_error(id, -32001, "task not found"),
    }
}

// ── tasks/sendSubscribe SSE ───────────────────────────────────────────────────

/// One step of a streamed send, rendered per generation by [`stream_events`].
enum Step<'a> {
    Submitted,
    Working,
    /// The agent serving this task is waiting for a grant (adopter-surface X3, third PR): the status carries the
    /// pending request's id and nothing else, so an A2A client can be the supervisor's surface.
    InputRequired(&'a str),
    Completed(&'a Task),
    /// `error` and `detail` are the pre-0.2 event's fields, each present where it always was; 0.3 and
    /// 1.0 carry the same text as the failed status's message.
    Failed { error: Option<String>, detail: Option<String> },
}

/// The SSE events one step becomes. Before 0.3: one `task_status_update` event, its data the bare
/// object, exactly as `tasks/sendSubscribe` has always sent. In 0.3 and 1.0: unnamed events whose data
/// is a JSON-RPC response to the request (`id`) carrying a `Task`, a status update or an artifact
/// update — a completion is the artifact, then the terminal status.
fn stream_events(id: &Option<Value>, req: &SendRequest, step: Step<'_>) -> Vec<Event> {
    let (task_id, context_id, generation) = (req.task_id.as_str(), req.context_id.as_str(), req.generation);
    if generation == Generation::Legacy {
        let data = match step {
            Step::Submitted => json!({ "id": task_id, "status": { "state": "submitted" } }),
            Step::Working => json!({ "id": task_id, "status": { "state": "working" } }),
            Step::InputRequired(request) => json!({ "id": task_id, "status": { "state": "input-required", "message": { "role": "agent", "parts": [{ "type": "text", "text": format!("grant request {request}") }] } } }),
            Step::Completed(task) => serde_json::to_value(task).unwrap_or(Value::Null),
            Step::Failed { error, detail } => {
                let mut v = json!({ "id": task_id, "status": { "state": "failed" } });
                if let Some(error) = error { v["error"] = json!(error); }
                if let Some(detail) = detail { v["detail"] = json!(detail); }
                v
            }
        };
        return vec![Event::default().event("task_status_update").data(data.to_string())];
    }
    let v03 = generation == Generation::V03;
    let event = |result: Value| Event::default().data(jsonrpc_ok(id.clone(), result).to_string());
    let status_update = |state: &str, message: Option<&str>, last: bool| {
        let mut status = json!({ "state": state_name(state, generation) });
        if let Some(text) = message {
            status["message"] = if v03 {
                json!({ "kind": "message", "messageId": format!("{task_id}-status"), "role": "agent",
                        "parts": [text_part(text, generation)] })
            } else {
                json!({ "messageId": format!("{task_id}-status"), "role": "ROLE_AGENT",
                        "parts": [text_part(text, generation)] })
            };
        }
        let update = json!({ "taskId": task_id, "contextId": context_id, "status": status });
        if v03 {
            let mut update = update;
            update["kind"] = json!("status-update");
            update["final"] = json!(last);
            update
        } else {
            json!({ "statusUpdate": update })
        }
    };
    match step {
        Step::Submitted => {
            let task = Task { id: task_id.to_string(), status: TaskStatus { state: "submitted".into() }, artifacts: vec![] };
            vec![event(render_send_result(&task, context_id, generation))]
        }
        Step::Working => vec![event(status_update("working", None, false))],
        Step::InputRequired(request) => vec![event(status_update("input-required", Some(&format!("grant request {request}")), false))],
        Step::Completed(task) => {
            let mut events: Vec<Event> = task.artifacts.iter().enumerate().map(|(i, a)| {
                let update = json!({ "taskId": task_id, "contextId": context_id,
                                     "artifact": render_artifact(task_id, i, a, generation), "lastChunk": true });
                if v03 {
                    let mut update = update;
                    update["kind"] = json!("artifact-update");
                    event(update)
                } else {
                    event(json!({ "artifactUpdate": update }))
                }
            }).collect();
            events.push(event(status_update("completed", None, true)));
            events
        }
        Step::Failed { error, detail } => {
            let text = detail.or(error).unwrap_or_else(|| "the skill call failed".into());
            vec![event(status_update("failed", Some(&text), true))]
        }
    }
}

/// A streamed send in any generation — `tasks/sendSubscribe`, `message/stream`, `SendStreamingMessage`
/// — running the same doors as [`dispatch_send`]: the reservation (taken by the caller, moved in here),
/// skill resolution, the AE preflight with the request's own `_meta`, the mandate to the provider, the
/// evidence record. A refusal becomes a failed task rather than a silent drop, so the client learns
/// the same thing it would on a unary send.
pub(crate) async fn tasks_send_subscribe(
    state:    A2aState,
    caller:   Option<ResolvedPrincipal>,
    id:       Option<Value>,
    req:      SendRequest,
    reservation: TaskReservation,
) -> impl IntoResponse {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(8);

    // Emit "submitted" immediately.
    for e in stream_events(&id, &req, Step::Submitted) {
        let _ = tx.try_send(Ok(e));
    }

    tokio::spawn(async move {
        let send_all = |events: Vec<Event>| {
            let tx = tx.clone();
            async move {
                for e in events {
                    let _ = tx.send(Ok(e)).await;
                }
            }
        };
        let skill_id = req.skill_id.clone();
        let target = match resolve_skill(&state.task_ctx, &skill_id) {
            Ok(target) => target,
            Err(why) => {
                send_all(stream_events(&id, &req, Step::Failed { error: Some(why), detail: None })).await;
                return;
            }
        };

        // Emit "working" after a short delay while the RPC runs.
        //
        // The handle is kept and **aborted before every terminal event**, because this task
        // outlives the outcome it was announcing. It sleeps a fixed 100 ms, and an AE refusal
        // resolves in about one, so a refused subscriber used to receive
        // `submitted → failed → working`: a terminal state followed by a non-terminal one, which
        // reads as a task that recovered from failing. The same held for any dispatch that
        // completed inside 100 ms — `completed → working` — so this predates the AE seam and is
        // not confined to refusals. Found by `examples/a2a_skill_authority`'s first CI run.
        let tx2     = tx.clone();
        let working_events = stream_events(&id, &req, Step::Working);
        let working = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            for e in working_events {
                let _ = tx2.try_send(Ok(e));
            }
        });
        // Adopter-surface X3 (third PR): while the skill runs, a grant request the serving node's agent writes is
        // reported as `input-required` with the request's id, and `working` again once it is answered — the records
        // gossip, so the gateway sees them wherever the agent is. Aborted with `working` before every terminal event.
        let grants = {
            let (tx3, tc, id3, req3) = (tx.clone(), Arc::clone(&state.task_ctx), id.clone(), req.clone());
            let prefix = format!("grant/{}/", gateway_caller::node_principal(&target));
            tokio::spawn(async move {
                let mut rx = mycelium_core::kv_handle::KvHandle::from_core(Arc::clone(&tc.core)).subscribe_prefix(prefix.clone());
                // The requests standing on the serving node: unanswered, unconsumed and unexpired — the same reading
                // as the supervisor's list. The record carries no task id, so this is the node's open requests, not
                // this task's: stated on the feature page (#679's review, finding 4).
                let open = |tc: &TaskCtx| -> std::collections::BTreeSet<String> {
                    let now_ms = tc.hlc.decision_now_ms();
                    let entries = crate::store::scan_kv_prefix(&tc.kv_state, &prefix);
                    let answered: std::collections::HashSet<String> = entries.iter().filter_map(|(k, _)| k.strip_suffix("/issued").or_else(|| k.strip_suffix("/refused")).or_else(|| k.strip_suffix("/consumed")).map(str::to_string)).collect();
                    entries.iter()
                        .filter(|(k, v)| k.matches('/').count() == 2 && !answered.contains(k.as_ref())
                            && serde_json::from_slice::<Value>(v).ok().and_then(|r| r["expires_at_ms"].as_u64()).is_some_and(|e| e > now_ms))
                        .filter_map(|(k, _)| k.rsplit('/').next().map(str::to_string))
                        .collect()
                };
                let before = open(&tc);
                // Every new request is reported once; `working` returns when none this task reported still stands.
                let mut reported: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
                let mut waiting = false;
                loop {
                    if rx.changed().await.is_err() { return; }
                    let now = open(&tc);
                    for r in now.difference(&before).filter(|r| !reported.contains(*r)).cloned().collect::<Vec<_>>() {
                        for e in stream_events(&id3, &req3, Step::InputRequired(&r)) { let _ = tx3.send(Ok(e)).await; }
                        reported.insert(r);
                        waiting = true;
                    }
                    if waiting && reported.iter().all(|r| !now.contains(r)) {
                        for e in stream_events(&id3, &req3, Step::Working) { let _ = tx3.send(Ok(e)).await; }
                        waiting = false;
                    }
                }
            })
        };

        #[cfg(all(feature = "gateway", feature = "tls"))]
        let preflight = super::http::ae_preflight(
            &state.task_ctx,
            caller.as_ref(),
            "skill.invoke",
            &format!("skill:{skill_id}@{target}"),
            &json!({ "text": &req.text }),
            // The request's own `_meta`, so a presented mandate is assessed here exactly as on a
            // unary send. This passed `Null` until 2026-09-25 (closure plan C2), so a stream call's
            // mandate was never read and a mandate-requiring rule refused every stream.
            &req.meta,
            super::http::ENFORCEMENT_POINT_A2A,
        )
        .await;
        #[cfg(all(feature = "gateway", feature = "tls"))]
        if let super::http::Preflight::Refuse(refusal) = &preflight {
            working.abort();
            grants.abort();
            send_all(stream_events(&id, &req, Step::Failed { error: Some(refusal.to_string()), detail: None })).await;
            return;
        }

        // The unary path's bound (its comment says why 30 s was too short for a multi-step skill); a streamed task
        // whose agent is waiting for a grant needs it too — a supervisor who answers after the dispatch times out
        // answers a task already reported failed (#679's review, finding 3). Stated on the feature page: the grant
        // window an A2A task can wait is bounded by this, not by `grant_window`.
        let timeout = Duration::from_secs(120);
        // Closure plan C2: the presented mandate travels to the provider, which verifies it itself.
        let mandate = gateway_caller::presented_mandate(&req.meta);
        let resource_claim = format!("skill:{skill_id}@{target}");
        let dispatched = gateway_caller::gateway_rpc_call_with_mandate(
            &state.task_ctx, caller.as_ref(), target,
            "skill.invoke".into(), Bytes::from(req.text.clone().into_bytes()), timeout, mandate,
            Some(&resource_claim),
        ).await;

        #[cfg(all(feature = "gateway", feature = "tls"))]
        {
            let observed = super::http::observed_execution(&dispatched);
            super::http::ae_record_execution(&state.task_ctx, &preflight, observed).await;
        }

        // Whatever the outcome, it is terminal: no "working" or "input-required" may follow it.
        working.abort();
        grants.abort();

        match dispatched {
            Ok(reply) => {
                let task = completed_task(req.task_id.clone(), reply);
                reservation.complete(task.clone());
                send_all(stream_events(&id, &req, Step::Completed(&task))).await;
            }
            Err(GatewayDispatchError::Rpc(e)) => {
                warn!("A2A streamed send rpc_call failed for skill {}: {:?}", skill_id, e);
                send_all(stream_events(&id, &req, Step::Failed { error: None, detail: None })).await;
            }
            Err(e) => {
                warn!("A2A streamed send refused for skill {}: {e}", skill_id);
                send_all(stream_events(&id, &req, Step::Failed {
                    error: Some(e.reason().to_string()),
                    detail: Some(e.to_string()),
                })).await;
            }
        }
    });

    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    Sse::new(stream).keep_alive(KeepAlive::default())
}

// ── Full JSON-RPC handler with SSE dispatch ───────────────────────────────────

pub(crate) async fn a2a_jsonrpc_full(
    State(state): State<A2aState>,
    caller:       Option<Extension<ResolvedPrincipal>>,
    #[cfg(feature = "tls")]
    federated:    Option<Extension<super::federation_http::FederatedIdentity>>,
    // `Bytes`, not `Json`, because a federated credential may bind these exact bytes and the
    // extractor would have thrown them away. Parsing is done here instead, which also lets a
    // malformed request be answered as JSON-RPC `-32700` rather than as a bare 400 — a JSON-RPC
    // endpoint that replies in a different protocol when it dislikes the input is its own small
    // dishonesty.
    headers:      axum::http::HeaderMap,
    raw:          axum::body::Bytes,
) -> axum::response::Response {
    // X4: a causal field carried as a header is never read — counted against this door.
    super::http::note_misplaced_causal_headers(&state.task_ctx, &headers, "a2a");
    let body: Value = match serde_json::from_slice(&raw) {
        Ok(v) => v,
        Err(e) => {
            return Json(jsonrpc_error(None, -32700, &format!("parse error: {e}"))).into_response()
        }
    };
    // Item 7: inserted by the gateway's `/a2a` optional-auth layer (a bearer's principal, or
    // `anonymous`); a handler reached without it (a bare router in a unit test) dispatches with
    // no context and is refused by the secure profile.
    let caller = caller.map(|Extension(c)| c);
    #[cfg(feature = "tls")]
    let federated = federated.map(|Extension(f)| f);
    let id     = body.get("id").cloned();
    let method = body.get("method").and_then(|m| m.as_str()).unwrap_or("").to_string();
    let params = body.get("params").cloned().unwrap_or(Value::Null);

    // The method's generation and verb (plan row N). `tasks/get` and `tasks/cancel` are pre-0.2's and
    // 0.3's both: the task answers in the generation that created it.
    let (verb, generation) = match method.as_str() {
        "tasks/send" => (Verb::Send, Some(Generation::Legacy)),
        "message/send" => (Verb::Send, Some(Generation::V03)),
        "SendMessage" => (Verb::Send, Some(Generation::V10)),
        "tasks/sendSubscribe" => (Verb::Stream, Some(Generation::Legacy)),
        "message/stream" => (Verb::Stream, Some(Generation::V03)),
        "SendStreamingMessage" => (Verb::Stream, Some(Generation::V10)),
        "tasks/get" => (Verb::Get, None),
        "GetTask" => (Verb::Get, Some(Generation::V10)),
        "tasks/cancel" => (Verb::Cancel, None),
        "CancelTask" => (Verb::Cancel, Some(Generation::V10)),
        _ => return Json(jsonrpc_error(id, -32601, "method not found")).into_response(),
    };

    // Item 2 PR 8: federated calls are unary (§5's "authenticated unary calls"). Streaming would
    // need the credential re-checked per event; that is a revision of D5, not a silent extension.
    #[cfg(feature = "tls")]
    if federated.is_some() && verb == Verb::Stream {
        return Json(jsonrpc_error(id, -32003, "federated calls are unary: use SendMessage, message/send or tasks/send"))
            .into_response();
    }

    // A credential binds **one export**, and only a send authorises against it
    // (`FederationEdge::authorize`). A get or a cancel carries no export and takes no
    // caller, so a partner granted one export could read any task's completed artifact — including a
    // *native* caller's — or cancel it, by naming its id. Task ids are caller-supplied on
    // `tasks/send`, so they are enumerable. Refused here for the same reason streaming is: binding
    // them to an export is a revision of D5, not a silent extension.
    // Found by the Phase-C adversarial audit (items 1+2+7). That closed the federated half only;
    // the native half — any bearer, or none, reading any task — is closed by `A2aTask::owner`.
    #[cfg(feature = "tls")]
    if federated.is_some() && matches!(verb, Verb::Get | Verb::Cancel) {
        return Json(jsonrpc_error(
            id,
            -32004,
            "a federated credential binds one export: a task is not read or cancelled across the federation edge",
        ))
        .into_response();
    }

    let send_request = |generation: Generation| match generation {
        Generation::Legacy => Ok(SendRequest::legacy(&state, &params)),
        Generation::V03 | Generation::V10 => SendRequest::current(&state, &id, &params, generation),
    };

    let result: Value = match verb {
        Verb::Stream => {
            let req = match send_request(generation.unwrap_or(Generation::Legacy)) {
                Ok(req) => req,
                Err(error) => return Json(error).into_response(),
            };
            // Claimed before the stream's task is spawned (#587 F5), and moved into it: a failed or
            // refused stream drops it, removing the pending entry.
            let reservation =
                match reserve_task(&state, &req.task_id, &req.context_id, req.generation, &task_owner(caller.as_ref())) {
                    Ok(reservation) => reservation,
                    Err(why) => return Json(why.refusal(id)).into_response(),
                };
            return tasks_send_subscribe(state, caller, id, req, reservation).await.into_response();
        }
        Verb::Send => match send_request(generation.unwrap_or(Generation::Legacy)) {
            Ok(req) => handle_send(
                &state,
                caller.as_ref(),
                #[cfg(feature = "tls")]
                federated.as_ref(),
                id.clone(),
                &req,
                &raw,
            ).await,
            Err(error) => error,
        },
        Verb::Get    => handle_tasks_get(&state, caller.as_ref(), id.clone(), &params, generation),
        Verb::Cancel => handle_tasks_cancel(&state, caller.as_ref(), id.clone(), &params, generation),
    };
    Json(result).into_response()
}

/// What a JSON-RPC method does, whichever generation names it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Verb { Send, Stream, Get, Cancel }

// ── Router (updated to use full handler with SSE) ─────────────────────────────

/// Returns the A2A router. Use this variant when `tasks/sendSubscribe` SSE support is needed.
pub(crate) fn a2a_router_full(
    task_ctx: Arc<TaskCtx>,
    tasks:    Arc<papaya::HashMap<String, A2aTask>>,
) -> Router {
    let state = A2aState { task_ctx, tasks };
    Router::new()
        .route("/.well-known/agent-card.json", get(agent_card_handler))
        // The pre-0.3 path, kept for one release (`deprecations.md` §46).
        .route("/.well-known/agent.json",      get(agent_card_handler))
        .route("/a2a",                         post(a2a_jsonrpc_full))
        .with_state(state)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    /// The 1.0 state names: an uppercased hyphen is not an enum value an `a2a-sdk` client accepts (#679's review,
    /// finding 1; seen failing first as `TASK_STATE_INPUT-REQUIRED`).
    #[test]
    fn a_hyphenated_state_is_an_underscored_1_0_name() {
        assert_eq!(super::state_name("input-required", super::Generation::V10), "TASK_STATE_INPUT_REQUIRED");
        assert_eq!(super::state_name("working", super::Generation::V10), "TASK_STATE_WORKING");
        assert_eq!(super::state_name("input-required", super::Generation::V03), "input-required");
        assert_eq!(super::state_name("input-required", super::Generation::Legacy), "input-required");
    }

    use super::*;
    use crate::{GossipAgent, GossipConfig, NodeId};

    /// **The card named the gossip port.** Its `url` was built from the node's gossip address, so an A2A
    /// client that follows the card (the current `a2a-sdk`, which Strands Agents uses) posted to the gossip
    /// listener without `/a2a` and failed with a network error (plan row N; the 2026-10-12 probe,
    /// `docs/plans/external/2026-10-12-a2a-sdk-probe/`). The card now names the gateway: its scheme
    /// (`https` under `gateway_tls`), the advertised host, its port and `/a2a`.
    #[tokio::test]
    async fn the_agent_card_names_the_gateways_a2a_endpoint_not_the_gossip_port() {
        let card_url = |cfg: GossipConfig, node: NodeId| async move {
            let agent = GossipAgent::new(node, cfg);
            let state = A2aState { task_ctx: Arc::clone(&agent.task_ctx), tasks: Arc::new(papaya::HashMap::new()) };
            let resp = agent_card_handler(State(state)).await.into_response();
            let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.expect("body");
            let v: Value = serde_json::from_slice(&body).expect("card json");
            v["url"].as_str().expect("url").to_string()
        };
        let mut cfg = GossipConfig::default();
        cfg.http_port = Some(9092);
        cfg.http_addr = "127.0.0.1".into();
        assert_eq!(card_url(cfg, NodeId::new("127.0.0.1", 8091).unwrap()).await, "http://127.0.0.1:9092/a2a",
            "the card must name the gateway's /a2a, not the gossip port");

        // A gateway bound to every interface is reached at the node's own address.
        let mut cfg = GossipConfig::default();
        cfg.http_port = Some(9092);
        cfg.http_addr = "0.0.0.0".into();
        assert_eq!(card_url(cfg, NodeId::new("10.0.0.7", 8091).unwrap()).await, "http://10.0.0.7:9092/a2a");
    }

    /// **Plan row N: the card where a current client reads it.** The 2026-10-12 probe found the
    /// current `a2a-sdk` (1.2.2, the library Strands Agents uses) reading `/.well-known/agent-card.json`
    /// (404 here) and, given the card, choosing its transport from `supportedInterfaces`, which the card
    /// did not have. The card now names `/a2a` once per protocol version it serves, 1.0 first, and says
    /// how a message reaches a skill. (That the path is mounted and public is
    /// `every_listed_public_path_is_mounted_and_public_and_gated_paths_are_not`'s, from `PUBLIC_PATHS`.)
    #[tokio::test]
    async fn the_card_names_a2a_once_per_protocol_version_and_how_a_message_reaches_a_skill() {
        let card = |agent: GossipAgent| async move {
            let state = A2aState { task_ctx: Arc::clone(&agent.task_ctx), tasks: Arc::new(papaya::HashMap::new()) };
            let resp = agent_card_handler(State(state)).await.into_response();
            let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.expect("body");
            serde_json::from_slice::<Value>(&body).expect("card json")
        };
        let mut cfg = GossipConfig::default();
        cfg.http_port = Some(9092);
        cfg.http_addr = "127.0.0.1".into();
        let v = card(GossipAgent::new(NodeId::new("127.0.0.1", 8091).unwrap(), cfg.clone())).await;
        let interfaces = v["supportedInterfaces"].as_array().expect("supportedInterfaces");
        let versions: Vec<&str> = interfaces.iter().map(|i| i["protocolVersion"].as_str().unwrap()).collect();
        assert_eq!(versions, ["1.0", "0.3"], "{v}");
        for i in interfaces {
            assert_eq!(i["url"], "http://127.0.0.1:9092/a2a", "{v}");
            assert_eq!(i["protocolBinding"], "JSONRPC", "{v}");
        }
        assert_eq!(v["url"], "http://127.0.0.1:9092/a2a", "the pre-0.3 field is kept: {v}");
        assert!(v["description"].as_str().unwrap().contains("is refused"), "no default, and the card says so: {v}");

        let agent = GossipAgent::new(NodeId::new("127.0.0.1", 8091).unwrap(), cfg).with_a2a_default_skill("depot", "dispatch");
        let v = card(agent).await;
        assert!(v["description"].as_str().unwrap().contains("goes to depot/dispatch"), "the default is stated: {v}");
    }

    /// **Plan row N: a standard client never sends `skillId`.** A 0.3 / 1.0 send names its skill in
    /// `metadata.skillId` — the message's or the request's — or reaches the declared default skill, or
    /// is refused `-32602` naming `metadata.skillId`; two different names are refused; a `taskId` is
    /// refused `-32004` (each message is its own task); the first part is read as text, or its `data`
    /// as JSON, and anything else is `-32005`; the request's `metadata._meta` is what the AE preflight
    /// and the mandate reader see, as `params._meta` is on `tasks/send`.
    #[test]
    fn a_current_send_names_its_skill_in_metadata_or_reaches_the_default() {
        let state = |default: Option<(&str, &str)>| {
            let mut agent = GossipAgent::new(NodeId::new("127.0.0.1", 0).unwrap(), GossipConfig::default());
            if let Some((ns, name)) = default {
                agent = agent.with_a2a_default_skill(ns, name);
            }
            A2aState { task_ctx: Arc::clone(&agent.task_ctx), tasks: Arc::new(papaya::HashMap::new()) }
        };
        let id = Some(json!(1));
        let msg = |extra: Value| {
            let mut m = json!({ "messageId": "m", "role": "ROLE_USER", "parts": [{ "text": "go" }] });
            if let (Some(m), Some(e)) = (m.as_object_mut(), extra.as_object()) {
                m.extend(e.clone());
            }
            m
        };
        let bare = state(None);
        let current = |st: &A2aState, params: Value| SendRequest::current(st, &id, &params, Generation::V10);

        // No skill named, no default: refused, and the refusal names where a skill is named.
        let e = current(&bare, json!({ "message": msg(json!({})) })).err().expect("refused");
        assert_eq!(e["error"]["code"], -32602, "{e}");
        assert!(e["error"]["message"].as_str().unwrap().contains("metadata.skillId"), "{e}");
        // The message's metadata, or the request's.
        let r = current(&bare, json!({ "message": msg(json!({ "metadata": { "skillId": "a/b" } })) })).unwrap();
        assert_eq!(r.skill_id, "a/b");
        let r = current(&bare, json!({ "message": msg(json!({})), "metadata": { "skillId": "c/d" } })).unwrap();
        assert_eq!(r.skill_id, "c/d");
        assert_eq!(r.text, "go");
        // Both, naming different skills: refused rather than one silently preferred.
        let e = current(&bare, json!({ "message": msg(json!({ "metadata": { "skillId": "a/b" } })),
                                       "metadata": { "skillId": "c/d" } })).err().expect("refused");
        assert_eq!(e["error"]["code"], -32602, "{e}");
        let e = current(&bare, json!({ "message": msg(json!({ "metadata": { "skillId": 7 } })) })).err().expect("refused");
        assert_eq!(e["error"]["code"], -32602, "{e}");

        // A declared default: reached when nothing is named, overridden when something is.
        let defaulted = state(Some(("depot", "dispatch")));
        assert_eq!(current(&defaulted, json!({ "message": msg(json!({})) })).unwrap().skill_id, "depot/dispatch");
        assert_eq!(
            current(&defaulted, json!({ "message": msg(json!({ "metadata": { "skillId": "a/b" } })) })).unwrap().skill_id,
            "a/b"
        );
        // A default that is not a skill name is not set.
        assert!(state(Some(("depot", "a/b"))).task_ctx.a2a_default_skill.get().is_none());

        // Each message is its own task; the task id is never the caller's.
        let e = current(&defaulted, json!({ "message": msg(json!({ "taskId": "someone-elses" })) })).err().expect("refused");
        assert_eq!(e["error"]["code"], -32004, "{e}");
        let r = current(&defaulted, json!({ "message": msg(json!({ "contextId": "ctx-1" })) })).unwrap();
        assert_eq!(r.context_id, "ctx-1");
        assert_ne!(r.task_id, "ctx-1");

        // Parts: text, data as JSON, nothing else; at least one.
        let parts = |p: Value| json!({ "message": { "messageId": "m", "role": "ROLE_USER", "parts": p } });
        assert_eq!(current(&defaulted, parts(json!([{ "data": { "n": 1 } }]))).unwrap().text, r#"{"n":1}"#);
        assert_eq!(current(&defaulted, parts(json!([{ "kind": "text", "text": "v03" }]))).unwrap().text, "v03");
        assert_eq!(current(&defaulted, parts(json!([{ "url": "https://x/y" }]))).err().unwrap()["error"]["code"], -32005);
        assert_eq!(current(&defaulted, parts(json!([]))).err().unwrap()["error"]["code"], -32602);
        assert_eq!(current(&defaulted, json!({})).err().unwrap()["error"]["code"], -32602);

        // The mandate and operation id ride in the request's `metadata._meta`.
        let r = current(&defaulted, json!({ "message": msg(json!({})),
                                            "metadata": { "_meta": { "mandate": { "m": 1 }, "operation_id": "op" } } })).unwrap();
        assert_eq!(gateway_caller::presented_mandate(&r.meta), Some(&json!({ "m": 1 })));
        assert_eq!(r.meta["_meta"]["operation_id"], "op");
    }

    /// **Plan row N: the current methods, end to end.** `SendMessage` (1.0) and `message/send` (0.3)
    /// reach the skill and answer in their generation's shapes; `GetTask` and `CancelTask` keep the
    /// owner rule; `tasks/get` answers a task in the generation that created it, since pre-0.2 and 0.3
    /// share the name; the streamed sends emit submitted, the artifact and a terminal completed status
    /// as JSON-RPC responses; a `taskId` is refused before any dispatch. Seen failing first: every new
    /// method answered `-32601` (method not found).
    #[cfg(feature = "compliance")]
    #[tokio::test]
    async fn the_current_a2a_methods_reach_the_skill_and_answer_in_their_generations_shapes() {
        use crate::capability::Capability;
        use crate::test_util::alloc_port;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let gossip_port = alloc_port();
        let http_port = alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.gateway_scoped_tokens = vec![
            crate::GatewayToken { token: "alice-tok".into(), scopes: vec!["kv:read".into()] },
            crate::GatewayToken { token: "bob-tok".into(), scopes: vec!["kv:read".into()] },
        ];
        let agent = Arc::new(
            GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg)
                .with_a2a()
                .with_a2a_default_skill("depot", "dispatch"),
        );
        agent.start().await.unwrap();
        let _reg = agent.capabilities().advertise_capability(Capability::new("depot", "dispatch"), Duration::from_secs(30));
        let ran = Arc::new(AtomicUsize::new(0));
        {
            let (agent, ran) = (Arc::clone(&agent), Arc::clone(&ran));
            let mut rx = agent.service().rpc_rx("skill.invoke");
            tokio::spawn(async move {
                while let Some(req) = rx.recv().await {
                    ran.fetch_add(1, Ordering::SeqCst);
                    agent.service().rpc_respond(&req, b"dispatched".to_vec());
                }
            });
        }
        until_advertised(&agent, "depot", "dispatch").await;

        let http = reqwest::Client::new();
        let base = format!("http://127.0.0.1:{http_port}");
        let raw = |bearer: Option<&str>, method: &str, params: Value| {
            let (http, url, method) = (http.clone(), format!("{base}/a2a"), method.to_string());
            let bearer = bearer.map(str::to_owned);
            async move {
                let mut req = http.post(&url).json(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}));
                if let Some(b) = bearer {
                    req = req.bearer_auth(b);
                }
                let r = req.send().await.unwrap();
                assert_eq!(r.status(), 200, "answered as JSON-RPC");
                r.text().await.unwrap()
            }
        };
        let call = |bearer: Option<&'static str>, method: &'static str, params: Value| {
            let fut = raw(bearer, method, params);
            async move { serde_json::from_str::<Value>(&fut.await).unwrap() }
        };
        // A message as a 1.0 client sends it (no skill: the default), and as a 0.3 client does.
        let v10 = json!({ "message": { "messageId": "m1", "role": "ROLE_USER", "parts": [{ "text": "go" }] } });
        let v03 = json!({ "message": { "kind": "message", "messageId": "m2", "role": "user",
                                       "parts": [{ "kind": "text", "text": "go" }] } });

        // The card, at the path the SDK reads.
        let card: Value = http.get(format!("{base}/.well-known/agent-card.json")).send().await.unwrap().json().await.unwrap();
        assert_eq!(card["supportedInterfaces"][0]["url"], format!("{base}/a2a"), "{card}");

        // 1.0: `{ task: … }`, `TASK_STATE_*`, bare text parts with an artifact id.
        let v = call(Some("alice-tok"), "SendMessage", v10.clone()).await;
        let task = &v["result"]["task"];
        assert_eq!(task["status"]["state"], "TASK_STATE_COMPLETED", "{v}");
        assert_eq!(task["artifacts"][0]["parts"][0], json!({ "text": "dispatched" }), "{v}");
        assert!(task["artifacts"][0]["artifactId"].is_string() && task["contextId"].is_string(), "{v}");
        let t10 = task["id"].as_str().unwrap().to_string();
        assert_eq!(ran.load(Ordering::SeqCst), 1);

        // 0.3: the task itself, `kind`-tagged, lower-case states.
        let v = call(Some("alice-tok"), "message/send", v03.clone()).await;
        assert_eq!(v["result"]["kind"], "task", "{v}");
        assert_eq!(v["result"]["status"]["state"], "completed", "{v}");
        assert_eq!(v["result"]["artifacts"][0]["parts"][0], json!({ "kind": "text", "text": "dispatched" }), "{v}");
        let t03 = v["result"]["id"].as_str().unwrap().to_string();
        assert_eq!(ran.load(Ordering::SeqCst), 2);

        // `GetTask` and `CancelTask` keep the owner rule; the creator reads its task in 1.0's shape.
        let v = call(Some("alice-tok"), "GetTask", json!({ "id": t10 })).await;
        assert_eq!(v["result"]["status"]["state"], "TASK_STATE_COMPLETED", "{v}");
        for who in [Some("bob-tok"), None] {
            for method in ["GetTask", "CancelTask"] {
                let v = call(who, method, json!({ "id": t10 })).await;
                assert_eq!(v["error"]["code"], -32004, "{who:?} {method}: {v}");
            }
        }
        let v = call(Some("alice-tok"), "CancelTask", json!({ "id": t10 })).await;
        assert_eq!(v["error"]["code"], -32002, "a completed task is not cancelable: {v}");
        // `tasks/get` answers in the generation that created the task: 0.3 here, pre-0.2 for `tasks/send`'s.
        let v = call(Some("alice-tok"), "tasks/get", json!({ "id": t03 })).await;
        assert_eq!(v["result"]["kind"], "task", "{v}");
        let v = call(Some("alice-tok"), "tasks/send", json!({ "id": "t-legacy", "skillId": "depot/dispatch",
            "message": { "role": "user", "parts": [{ "type": "text", "text": "go" }] } })).await;
        assert_eq!(v["result"]["artifacts"][0]["parts"][0], json!({ "type": "text", "text": "dispatched" }), "{v}");
        let v = call(Some("alice-tok"), "tasks/get", json!({ "id": "t-legacy" })).await;
        assert!(v["result"].get("kind").is_none() && v["result"]["status"]["state"] == "completed", "{v}");
        assert_eq!(ran.load(Ordering::SeqCst), 3);

        // A pre-0.2 re-send naming a task `SendMessage` created is refused, not run again over it
        // (#633 review, finding 2; seen failing first: the skill ran and the task was replaced).
        let v = call(Some("alice-tok"), "tasks/send", json!({ "id": t10, "skillId": "depot/dispatch",
            "message": { "role": "user", "parts": [{ "type": "text", "text": "over" }] } })).await;
        assert_eq!(v["error"]["code"], -32004, "{v}");
        assert_eq!(ran.load(Ordering::SeqCst), 3, "nothing dispatched for the refused re-send");
        let v = call(Some("alice-tok"), "GetTask", json!({ "id": t10 })).await;
        assert_eq!(v["result"]["artifacts"][0]["parts"][0]["text"], "dispatched", "the task is unchanged: {v}");

        // A `taskId` is refused before anything is dispatched.
        let mut cont = v10.clone();
        cont["message"]["taskId"] = json!(t10);
        assert_eq!(call(Some("alice-tok"), "SendMessage", cont).await["error"]["code"], -32004);
        assert_eq!(ran.load(Ordering::SeqCst), 3, "nothing dispatched for the refused send");

        // The streams: every event a JSON-RPC response to the request; submitted first, the artifact,
        // and the terminal completed status last.
        let events = |body: String| -> Vec<Value> {
            body.lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .map(|d| serde_json::from_str::<Value>(d.trim()).expect("event data is JSON"))
                .collect()
        };
        let e = events(raw(Some("alice-tok"), "SendStreamingMessage", v10.clone()).await);
        assert!(e.iter().all(|e| e["jsonrpc"] == "2.0" && e["id"] == 1), "{e:?}");
        assert_eq!(e[0]["result"]["task"]["status"]["state"], "TASK_STATE_SUBMITTED", "{e:?}");
        assert!(e.iter().any(|e| e["result"]["artifactUpdate"]["artifact"]["parts"][0]["text"] == "dispatched"), "{e:?}");
        assert_eq!(e.last().unwrap()["result"]["statusUpdate"]["status"]["state"], "TASK_STATE_COMPLETED", "{e:?}");
        let e = events(raw(Some("alice-tok"), "message/stream", v03.clone()).await);
        assert_eq!(e[0]["result"]["kind"], "task", "{e:?}");
        assert!(e.iter().any(|e| e["result"]["kind"] == "artifact-update"), "{e:?}");
        let last = &e.last().unwrap()["result"];
        assert_eq!((last["kind"].as_str(), last["status"]["state"].as_str(), last["final"].as_bool()),
                   (Some("status-update"), Some("completed"), Some(true)), "{e:?}");
        assert_eq!(ran.load(Ordering::SeqCst), 5);

        agent.shutdown().await;
    }

    /// Waits until the node's own `cap/` entry for `ns/name` is in its store. Advertising is spawned,
    /// so a fixed sleep before the first send lost the race under a loaded test run (`skill not
    /// found`, seen on the full suite 2026-10-17).
    #[cfg(feature = "compliance")]
    async fn until_advertised(agent: &GossipAgent, ns: &str, name: &str) {
        let key = format!("cap/{}/{ns}/{name}", agent.node_id());
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while agent.kv().get(&key).is_none() {
            assert!(std::time::Instant::now() < deadline, "{key} was never written");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn make_ctx() -> Arc<TaskCtx> {
        let agent = GossipAgent::new(
            NodeId::new("127.0.0.1", 0).unwrap(),
            GossipConfig::default(),
        );
        Arc::clone(&agent.task_ctx)
    }

    /// **`/a2a` was unguarded.** The evaluator ran on `tools/call` only, so a remit enforced at the
    /// MCP edge could be walked around by choosing the other door — and the evidence would have gone
    /// on saying `coverage.complete: false`, truthfully, while nothing was enforced there at all.
    ///
    /// This exercises the shared preflight with the A2A route's own parameters and asserts the
    /// evidence names that route. The SSE call site above is compile-checked; standing up the
    /// streaming stack to re-test the same function would buy little.
    #[cfg(feature = "compliance")]
    #[tokio::test]
    async fn the_a2a_edge_refuses_and_records_under_its_own_enforcement_point() {
        use crate::test_util::alloc_port;
        use crate::{AeEvidence, AeReference, Execution, ReferenceEvaluator, Rule};
        use super::super::evidence_journal::{self, EvidenceJournal, EvidenceProfile};

        let port = alloc_port();
        let cert_dir = std::env::temp_dir().join(format!("ae-a2a-{port}"));
        let journal_path =
            std::env::temp_dir().join(format!("ae-a2a-journal-{port}")).join("evidence.log");
        let _ = std::fs::remove_dir_all(&cert_dir);
        let _ = std::fs::remove_dir_all(journal_path.parent().unwrap());
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.tls = Some(crate::TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
        agent.with_evidence_journal(
            EvidenceJournal::open(&journal_path, EvidenceProfile::Strict).unwrap(),
        );

        let resource = "skill:danger@127.0.0.1:1".to_string();
        agent.with_action_evaluator(Arc::new(
            ReferenceEvaluator::new("rev-a2a")
                .with_catalogue("cat-test", "1")
                .map_action("skill.invoke", resource.clone())
                .prohibit(Rule::new("*", "skill.invoke", resource.clone())),
        ));
        agent.start().await.unwrap();

        let refusal = super::super::http::ae_preflight(
            &agent.task_ctx,
            None,
            "skill.invoke",
            &resource,
            &json!({ "text": "please do the forbidden thing" }),
            &Value::Null,
            super::super::http::ENFORCEMENT_POINT_A2A,
        )
        .await;
        let super::super::http::Preflight::Refuse(refusal) = refusal else {
            panic!("a prohibited skill must be refused at the A2A edge")
        };
        assert_eq!(refusal.json_rpc_code(), -32030);
        assert_eq!(refusal.reason(), "action_denied");

        // The evidence is in the node-local journal, in full.
        let journalled: Vec<AeEvidence> = evidence_journal::read_evidence_journal(&journal_path)
            .unwrap()
            .iter()
            .map(|b| serde_json::from_slice(b).expect("an AE evidence record"))
            .collect();
        assert_eq!(journalled.len(), 1, "the A2A decision is recorded like any other");
        let e = &journalled[0];
        assert_eq!(e.enforcement_point, super::super::http::ENFORCEMENT_POINT_A2A);
        assert_eq!(e.operation, "skill.invoke");
        assert_eq!(e.resource, resource);
        assert_eq!(e.execution, Execution::None, "nothing ran, and this point can say so");

        // The chain carries a reference — and not the skill that was asked for.
        let chain = agent.audit_stream(agent.node_id());
        let refs: Vec<AeReference> = chain
            .iter()
            .filter_map(|r| AeReference::from_detail(r.record.detail.as_deref()))
            .collect();
        assert_eq!(refs.len(), 1);
        assert!(refs[0].journal_sha256.is_some());
        let gossiped: String = chain
            .iter()
            .map(|r| format!("{}|{}", r.record.target, r.record.detail.clone().unwrap_or_default()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !gossiped.contains("skill:danger"),
            "the A2A resource reached the gossiped chain; §5 forbids it.\n{gossiped}"
        );

        agent.shutdown().await;
        let _ = std::fs::remove_dir_all(&cert_dir);
        let _ = std::fs::remove_dir_all(journal_path.parent().unwrap());
    }

    #[test]
    fn agent_card_from_empty_caps() {
        let ctx   = make_ctx();
        let kv    = &ctx.kv_state;
        let pairs = scan_kv_prefix(kv, "cap/");
        assert!(pairs.is_empty(), "fresh agent has no caps");
    }

    /// The card lists what an agent can be asked to do, not the fleet's plumbing: a provisioning tier, a
    /// shed mark, model metadata, the librarian and the blob cache are capabilities nobody invokes as a skill.
    #[test]
    fn agent_card_leaves_out_infrastructure_capabilities() {
        use crate::capability::Capability;
        use crate::framing::make_gossip_update;
        use crate::store::apply_and_notify;
        let ctx = make_ctx();
        for (ns, name) in [("demo", "echo"), ("prov-shed", "demo:echo:00ff"), ("llm", "loading"), ("demo", "installable"),
                           ("llm-meta", "qwen"), ("artifact", "librarian"), ("reason", "blob-cache"),
                           // A companion's election and role marks (the review of #550).
                           ("tuple", "orders.primary"), ("blackboard", "plan.secondary"), ("tuple", "orders.candidate"),
                           ("wiki", "council.curator"),
                           // A prompt skill answers llm.invoke, which `/a2a` does not send.
                           ("llm", "qwen3")] {
            let key = format!("cap/127.0.0.1:9001/{ns}/{name}");
            let upd = make_gossip_update(&ctx.node_id, 5, std::sync::Arc::from(key.as_str()),
                Capability::new(ns, name).encode(), false, &ctx.hlc);
            apply_and_notify(&ctx.kv_state, &upd);
        }
        apply_and_notify(&ctx.kv_state, &make_gossip_update(&ctx.node_id, 5, std::sync::Arc::from("prompts/llm/qwen3"),
            bytes::Bytes::from_static(b"{}"), false, &ctx.hlc));
        let ids = card_skill_ids(&ctx.kv_state);
        assert_eq!(ids, std::collections::HashSet::from(["demo/echo".to_string()]), "{ids:?}");
        // tasks/send agrees with the card: what the card leaves out does not resolve.
        assert!(resolve_skill(&ctx, "demo/echo").is_ok());
        for id in ["prov-shed/demo:echo:00ff", "artifact/librarian", "tuple/orders.primary", "llm/qwen3"] {
            assert!(resolve_skill(&ctx, id).is_err(), "{id} resolved");
        }
        assert!(resolve_skill(&ctx, "llm/qwen3").unwrap_err().contains("/gateway/llm/call"));
    }

    #[test]
    fn agent_card_deduplicates_skills() {
        use crate::capability::Capability;
        use crate::framing::make_gossip_update;
        use crate::store::apply_and_notify;

        let ctx  = make_ctx();
        let node = ctx.node_id.clone();
        let cap  = Capability::new("compute", "gpu");
        let bytes = cap.encode();

        // Two cap/ entries for the same (ns, name) from different nodes.
        for suffix in ["cap/127.0.0.1:9001/compute/gpu", "cap/127.0.0.1:9002/compute/gpu"] {
            let upd = make_gossip_update(
                &node, 5, std::sync::Arc::from(suffix),
                bytes.clone(), false, &ctx.hlc,
            );
            apply_and_notify(&ctx.kv_state, &upd);
        }

        let skill_ids = card_skill_ids(&ctx.kv_state);
        assert_eq!(skill_ids.len(), 1, "two nodes with same cap must produce one skill");
    }

    #[test]
    fn tasks_get_unknown_returns_error() {
        let tasks = Arc::new(papaya::HashMap::<String, A2aTask>::new());
        let state = A2aState { task_ctx: make_ctx(), tasks };
        let result = handle_tasks_get(&state, None, None, &json!({ "id": "no-such-id" }), None);
        assert_eq!(result["error"]["code"], -32001);
    }

    /// The agent card must surface each skill's gossiped input schema as a
    /// description — tool-calling frameworks decide how to invoke from it,
    /// and an empty description left agents passing plain text to
    /// JSON-expecting skills.
    #[test]
    fn agent_card_description_renders_input_schema() {
        let kv = crate::store::KvState::new(0);
        let schema = serde_json::json!({
            "type": "object",
            "required": ["topic"],
            "properties": {
                "topic":      { "type": "string", "description": "Topic to write about" },
                "max_points": { "type": "integer" },
            },
        });
        kv.store.pin().insert(
            Arc::from("skills/llm/orchestrator/127.0.0.1:7950/input"),
            crate::store::StoreEntry {
                data:      Some(bytes::Bytes::from(serde_json::to_vec(&schema).unwrap())),
                timestamp: 1,
            },
        );
        let (desc, schema) = skill_input_description(&kv, "llm/orchestrator");
        assert!(schema.is_some(), "raw schema is exposed on the card as inputSchema");
        assert!(desc.contains("JSON object"), "tells callers the payload shape: {desc}");
        assert!(desc.contains("topic (string, required): Topic to write about"), "{desc}");
        assert!(desc.contains("max_points (integer, optional)"), "{desc}");
        // No schema on the mesh → empty description, never a panic.
        let (empty_desc, no_schema) = skill_input_description(&kv, "llm/unknown");
        assert_eq!(empty_desc, "");
        assert!(no_schema.is_none());
    }

    /// M2 Run-18 race-family sweep regression: the cleanup sweep removes only
    /// entries that are still stale at removal time — a task re-inserted with
    /// a fresh `created_at` (status update) survives the sweep.
    #[test]
    fn cleanup_sweep_evicts_stale_keeps_fresh() {
        let mk = |id: &str| Task {
            id: id.into(),
            status: TaskStatus { state: "working".into() },
            artifacts: vec![],
        };
        let tasks = Arc::new(papaya::HashMap::<String, A2aTask>::new());
        let now = Instant::now();
        let old = now.checked_sub(Duration::from_secs(600)).expect("clock supports -600s");
        tasks.pin().insert("stale".into(), A2aTask { task: mk("stale"), context_id: String::new(), generation: Generation::Legacy, created_at: old, owner: None, pending: None });
        tasks.pin().insert("fresh".into(), A2aTask { task: mk("fresh"), context_id: String::new(), generation: Generation::Legacy, created_at: now, owner: None, pending: None });
        evict_stale_tasks(&tasks, now);
        assert!(tasks.pin().get("stale").is_none(), "stale task evicted");
        assert!(tasks.pin().get("fresh").is_some(), "fresh task survives the sweep");
    }

    #[test]
    fn tasks_cancel_completed_returns_error() {
        let tasks = Arc::new(papaya::HashMap::<String, A2aTask>::new());
        let task  = Task {
            id:        "t1".into(),
            status:    TaskStatus { state: "completed".into() },
            artifacts: vec![],
        };
        tasks.pin().insert("t1".into(), A2aTask { task, context_id: String::new(), generation: Generation::Legacy, created_at: Instant::now(), owner: Some("token:gw/a".into()), pending: None });
        let state = A2aState { task_ctx: make_ctx(), tasks };
        let creator = ResolvedPrincipal { principal: "token:gw/a".into(), scopes: Vec::new() };
        let result = handle_tasks_cancel(&state, Some(&creator), None, &json!({ "id": "t1" }), None);
        assert_eq!(result["error"]["code"], -32002);
        // The owner rule comes before the state rule: another identity, or no identity, is -32004.
        let other = ResolvedPrincipal { principal: "token:gw/b".into(), scopes: Vec::new() };
        assert_eq!(handle_tasks_cancel(&state, Some(&other), None, &json!({ "id": "t1" }), None)["error"]["code"], -32004);
        assert_eq!(handle_tasks_cancel(&state, None, None, &json!({ "id": "t1" }), None)["error"]["code"], -32004);
        assert_eq!(handle_tasks_get(&state, None, None, &json!({ "id": "t1" }), None)["error"]["code"], -32004);
        assert_eq!(handle_tasks_get(&state, Some(&creator), None, &json!({ "id": "t1" }), None)["result"]["id"], "t1");
    }

    /// **Public `/a2a` `tasks/get` and `tasks/cancel` had no owner check.** Task ids are caller-chosen
    /// on `tasks/send`, so they are enumerable, and both methods looked an id up with no caller — any
    /// client could read another caller's completed artifact or cancel its task by naming the id. The
    /// federated half was closed in the Phase-C audit (2026-09-20); the native half was not. Now a task
    /// records the principal that created it, and `tasks/get` / `tasks/cancel` answer **only that
    /// identity**: another principal, or an anonymous caller, is refused `-32004` naming the rule, and
    /// a `tasks/send` naming an id another principal owns is refused before any dispatch. Seen failing
    /// first: the second principal read the first's task (`result.id == "t-alice"`).
    #[cfg(feature = "compliance")]
    #[tokio::test]
    async fn a_task_is_readable_and_cancellable_only_by_the_identity_that_created_it() {
        use crate::capability::Capability;
        use crate::test_util::alloc_port;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let gossip_port = alloc_port();
        let http_port = alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.gateway_scoped_tokens = vec![
            crate::GatewayToken { token: "alice-tok".into(), scopes: vec!["kv:read".into()] },
            crate::GatewayToken { token: "bob-tok".into(), scopes: vec!["kv:read".into()] },
        ];
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg).with_a2a());
        agent.start().await.unwrap();
        let _reg = agent.capabilities().advertise_capability(Capability::new("depot", "dispatch"), Duration::from_secs(30));
        let ran = Arc::new(AtomicUsize::new(0));
        {
            let (agent, ran) = (Arc::clone(&agent), Arc::clone(&ran));
            let mut rx = agent.service().rpc_rx("skill.invoke");
            tokio::spawn(async move {
                while let Some(req) = rx.recv().await {
                    ran.fetch_add(1, Ordering::SeqCst);
                    agent.service().rpc_respond(&req, b"dispatched".to_vec());
                }
            });
        }
        until_advertised(&agent, "depot", "dispatch").await;

        let http = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{http_port}/a2a");
        let call = |bearer: Option<&str>, method: &str, params: serde_json::Value| {
            let (http, url, method) = (http.clone(), url.clone(), method.to_string());
            let bearer = bearer.map(str::to_owned);
            async move {
                let mut req = http.post(&url).json(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}));
                if let Some(b) = bearer {
                    req = req.bearer_auth(b);
                }
                let r = req.send().await.unwrap();
                assert_eq!(r.status(), 200, "answered as JSON-RPC");
                r.json::<serde_json::Value>().await.unwrap()
            }
        };
        let send = |id: &str| json!({
            "id": id, "skillId": "depot/dispatch",
            "message": {"role": "user", "parts": [{"type": "text", "text": "go"}]},
        });

        let v = call(Some("alice-tok"), "tasks/send", send("t-alice")).await;
        assert_eq!(v["result"]["status"]["state"], "completed", "{v}");
        assert_eq!(ran.load(Ordering::SeqCst), 1);

        for (who, method) in [(Some("bob-tok"), "tasks/get"), (Some("bob-tok"), "tasks/cancel"), (None, "tasks/get"), (None, "tasks/cancel")] {
            let v = call(who, method, json!({"id": "t-alice"})).await;
            assert_eq!(v["error"]["code"], -32004, "{who:?} {method}: not the identity that created the task: {v}");
            assert!(v["error"]["message"].as_str().unwrap().contains("created it"), "names the rule: {v}");
        }
        // The creator reads its own task.
        let v = call(Some("alice-tok"), "tasks/get", json!({"id": "t-alice"})).await;
        assert_eq!(v["result"]["id"], "t-alice", "{v}");
        // A caller-chosen id cannot be taken over by another principal's `tasks/send`: refused before dispatch.
        let v = call(Some("bob-tok"), "tasks/send", send("t-alice")).await;
        assert_eq!(v["error"]["code"], -32004, "{v}");
        assert_eq!(ran.load(Ordering::SeqCst), 1, "nothing was dispatched for the refused send");
        let v = call(Some("alice-tok"), "tasks/get", json!({"id": "t-alice"})).await;
        assert_eq!(v["result"]["id"], "t-alice", "alice's task is still hers: {v}");

        agent.shutdown().await;
    }

    /// **#587 F5: the owner check and the insert straddled the dispatch.** Two principals sending
    /// under one new id both passed the check (nothing was stored until the reply), both dispatched,
    /// and the later reply replaced the earlier caller's task. The id is now **reserved** before the
    /// dispatch (`try_insert` of a pending task owned by the caller), so exactly one send runs; the
    /// other is refused `-32004` before anything is dispatched. Seen failing first: the skill ran
    /// twice.
    #[cfg(feature = "compliance")]
    #[tokio::test]
    async fn two_principals_racing_one_task_id_dispatch_once() {
        use crate::capability::Capability;
        use crate::test_util::alloc_port;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let gossip_port = alloc_port();
        let http_port = alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.gateway_scoped_tokens = vec![
            crate::GatewayToken { token: "alice-tok".into(), scopes: vec!["kv:read".into()] },
            crate::GatewayToken { token: "bob-tok".into(), scopes: vec!["kv:read".into()] },
        ];
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg).with_a2a());
        agent.start().await.unwrap();
        let _reg = agent.capabilities().advertise_capability(Capability::new("depot", "dispatch"), Duration::from_secs(30));
        let ran = Arc::new(AtomicUsize::new(0));
        {
            let (agent, ran) = (Arc::clone(&agent), Arc::clone(&ran));
            let mut rx = agent.service().rpc_rx("skill.invoke");
            tokio::spawn(async move {
                while let Some(req) = rx.recv().await {
                    ran.fetch_add(1, Ordering::SeqCst);
                    let agent = Arc::clone(&agent);
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        agent.service().rpc_respond(&req, b"dispatched".to_vec());
                    });
                }
            });
        }
        until_advertised(&agent, "depot", "dispatch").await;

        let url = format!("http://127.0.0.1:{http_port}/a2a");
        let send = |bearer: &'static str| {
            let url = url.clone();
            tokio::spawn(async move {
                reqwest::Client::new().post(&url).bearer_auth(bearer)
                    .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tasks/send", "params": {
                        "id": "t-race", "skillId": "depot/dispatch",
                        "message": {"role": "user", "parts": [{"type": "text", "text": "go"}]}}}))
                    .send().await.unwrap().json::<serde_json::Value>().await.unwrap()
            })
        };
        let (a, b) = (send("alice-tok"), send("bob-tok"));
        let (a, b) = (a.await.unwrap(), b.await.unwrap());
        let completed = [&a, &b].iter().filter(|v| v["result"]["status"]["state"] == "completed").count();
        let refused = [&a, &b].iter().filter(|v| v["error"]["code"] == -32004).count();
        assert_eq!((completed, refused), (1, 1), "exactly one send runs, the other is refused: {a} / {b}");
        assert_eq!(ran.load(Ordering::SeqCst), 1, "the skill ran once");
        agent.shutdown().await;
    }
}
