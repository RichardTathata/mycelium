//! Agent state machine: typed execution states, policy guards, and mesh-visible
//! state propagation via the KV + signal substrate.
//!
//! Obtain an [`AgentStateMachine`] via
//! [`GossipAgent::agent_state_machine`](crate::GossipAgent::agent_state_machine).

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Weak,
    },
    time::Duration,
};
use bytes::Bytes;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use tokio::{sync::watch, task::JoinHandle};

use crate::CapFilter;
use crate::NodeId;
use crate::framing::{dispatch_gossip_try_send, ForwardHint, make_gossip_update};
use crate::signal::{SignalScope, signal_kind};
use crate::store::apply_and_notify;
use super::capability_ops::{resolve_filter_against_kv, subscribe_prefix_on_kv};
use super::helpers::emit_signal;
use super::TaskCtx;

// ── ExecutionState ────────────────────────────────────────────────────────────────

/// Execution state of an LLM agent node.
///
/// Propagated to the gossip mesh via KV key `agent/{node}/state` on every
/// committed [`AgentStateMachine::transition`] call. Remote observers can read
/// live states from that key prefix or use [`AgentStateMachine::watch_mesh_states`].
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ExecutionState {
    /// Agent is idle — no active task.
    Idle,
    /// Calling the LLM to plan the next action.
    Planning,
    /// Executing a tool call.
    Invoking { tool: String },
    /// Processing a tool result before the next planning step.
    Reflecting,
    /// Task checkpointed; waiting to be resumed.
    Suspended { task_id: String },
    /// Task completed successfully.
    Done,
    /// Task failed with a reason string.
    Failed { reason: String },
    /// Application-defined state.
    Custom(String),
}

impl ExecutionState {
    /// Serialize to the compact KV wire format used in `agent/{node}/state`.
    pub fn to_kv_str(&self) -> String {
        match self {
            ExecutionState::Idle               => "Idle".to_string(),
            ExecutionState::Planning           => "Planning".to_string(),
            ExecutionState::Reflecting         => "Reflecting".to_string(),
            ExecutionState::Done               => "Done".to_string(),
            ExecutionState::Invoking  { tool }    => format!("Invoking:{tool}"),
            ExecutionState::Failed    { reason }  => format!("Failed:{reason}"),
            ExecutionState::Suspended { task_id } => format!("Suspended:{task_id}"),
            ExecutionState::Custom(s)             => format!("Custom:{s}"),
        }
    }

    /// Parse from the KV wire format produced by [`to_kv_str`](Self::to_kv_str).
    pub fn from_kv_str(s: &str) -> Self {
        if s == "Idle"       { return ExecutionState::Idle; }
        if s == "Planning"   { return ExecutionState::Planning; }
        if s == "Reflecting" { return ExecutionState::Reflecting; }
        if s == "Done"       { return ExecutionState::Done; }
        if let Some(t)  = s.strip_prefix("Invoking:")   { return ExecutionState::Invoking   { tool:    t.to_string() }; }
        if let Some(r)  = s.strip_prefix("Failed:")     { return ExecutionState::Failed     { reason:  r.to_string() }; }
        if let Some(id) = s.strip_prefix("Suspended:")  { return ExecutionState::Suspended  { task_id: id.to_string() }; }
        if let Some(c)  = s.strip_prefix("Custom:")     { return ExecutionState::Custom(c.to_string()); }
        ExecutionState::Custom(s.to_string())
    }
}

impl std::fmt::Display for ExecutionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_kv_str())
    }
}

// ── AgentPolicy ───────────────────────────────────────────────────────────────

/// Policy rules enforced before any state transition is committed.
///
/// All fields are optional — the zero value permits all transitions.
///
/// The active policy is written to `agent/{node}/policy` in the gossip KV store
/// whenever it changes (construction or [`AgentStateMachine::set_policy`]), so
/// supervisors and monitors can read the current rules from any mesh node.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AgentPolicy {
    /// Maximum number of LLM turns (Planning→…→Reflecting cycles) per task.
    /// Blocks any `→ Invoking` transition once exceeded.
    pub max_turns: Option<usize>,

    /// Maximum total tool calls per task.
    /// Blocks any `→ Invoking` transition once exceeded.
    pub tool_budget: Option<usize>,

    /// If non-empty, only the listed tool names may be invoked.
    /// Empty = all tools allowed.
    pub allowed_tools: Vec<String>,

    /// Tool names that are always blocked, regardless of `allowed_tools`.
    pub denied_tools: Vec<String>,

    /// Auto-transition to `Failed` after spending longer than the specified
    /// duration in the given state. The timer is armed on each committed
    /// transition and cancelled when the agent moves to a new state.
    pub state_timeouts: HashMap<ExecutionState, Duration>,

    /// Tools that require a supervisor approval signal before `Invoking` is
    /// committed. Emits [`AGENT_APPROVE`](signal_kind::AGENT_APPROVE); blocks
    /// up to 30 s for [`AGENT_VETO`](signal_kind::AGENT_VETO).
    /// Silence within the window = approved; veto = [`PolicyViolation::ApprovalVetoed`].
    ///
    /// This is **veto, not grant**: an absent supervisor approves. For approval that is default-deny, see
    /// [`require_grant_for`](Self::require_grant_for).
    pub require_approval_for: Vec<String>,
    /// Adopter-surface X3 (third PR): the data class of the arguments a grant request renders for a human — the
    /// class's name in the operator's vocabulary (`public`, `confidential`, …), as row R's `DataClass` names it.
    /// `None` is unclassified.
    #[serde(default)]
    pub grant_arguments_class: Option<String>,
    /// The classes whose arguments **may be rendered** into the gossiped request record. Arguments of a class not
    /// listed are withheld — the record says so and carries the class and the size — while the grant still binds
    /// their digest. Unclassified arguments are rendered. **Default: empty**, so a classified argument set is
    /// withheld unless its class is listed here.
    #[serde(default)]
    pub grant_renderable_classes: Vec<String>,

    /// Tools that require a **grant** before `Invoking` is committed (adopter-surface X3) — approval that is
    /// default-deny. A grant is a scoped mandate a supervisor issues for one call, bound to the tool, the
    /// arguments' digest, this agent's identity and the policy revision, with an expiry
    /// ([`crate::mandate::grant::GRANT_SCOPE_PREFIX`]). The agent asks through
    /// [`AgentStateMachine::transition_invoking`], which writes the request under `grant/{agent}/{request}` and
    /// waits [`grant_window`](Self::grant_window) for `…/issued`; silence or a refusal is
    /// [`PolicyViolation::GrantNotPresented`] / [`PolicyViolation::GrantRefused`], never a proceed. A plain
    /// [`transition`](AgentStateMachine::transition) to `Invoking` on a tool listed here is refused: it carries no
    /// arguments to bind. The grant is verified here, again by the gateway's preflight, and again by a provider with
    /// `with_provider_enforcement()`, so a client's button is never the boundary.
    #[serde(default)]
    pub require_grant_for: Vec<String>,

    /// How long [`transition_invoking`](AgentStateMachine::transition_invoking) waits for a grant before refusing.
    /// Default 30 s. Read on the wall clock (`Hlc::decision_now_ms`), never on the HLC alone.
    #[serde(default)]
    pub grant_window: Option<Duration>,

    /// Blocks `Idle → Planning` until every filter resolves to ≥ 1 capability
    /// provider. Checked synchronously inside `transition(Planning)` via a
    /// live KV scan — fails fast if required dependencies are not yet advertised.
    pub required_capabilities: Vec<CapFilter>,
}

// ── PolicyViolation ───────────────────────────────────────────────────────────

/// A policy guard rejected a transition. The requested transition was **not** committed.
#[derive(Debug, Clone)]
pub enum PolicyViolation {
    RequiredCapabilityMissing(String),
    TurnBudgetExceeded,
    ToolBudgetExceeded,
    ToolNotAllowed(String),
    ToolDenied(String),
    /// Never produced: `await_approval` returns `Ok` on silence — the veto mode approves an absent supervisor.
    /// Kept so a `match` on the 2.x line compiles; the grant mode's refusal on silence is
    /// [`GrantNotPresented`](Self::GrantNotPresented) (`deprecations.md` §59).
    #[deprecated(since = "2.38.0", note = "never produced: the veto mode approves on silence; the grant mode refuses with GrantNotPresented")]
    ApprovalTimedOut(String),
    ApprovalVetoed(String),
    /// A tool in `require_grant_for` was invoked and no grant was presented within the window — or `transition`
    /// was called with no arguments to bind (adopter-surface X3). Default-deny: silence refuses.
    GrantNotPresented(String),
    /// A supervisor refused the request, or the grant issued did not verify here (the reason is in the string).
    GrantRefused(String),
}

impl std::fmt::Display for PolicyViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PolicyViolation::RequiredCapabilityMissing(s) => write!(f, "required capability missing: {s}"),
            PolicyViolation::TurnBudgetExceeded           => write!(f, "turn budget exceeded"),
            PolicyViolation::ToolBudgetExceeded           => write!(f, "tool budget exceeded"),
            PolicyViolation::ToolNotAllowed(t)            => write!(f, "tool not in allow-list: {t}"),
            PolicyViolation::ToolDenied(t)                => write!(f, "tool is denied: {t}"),
            #[allow(deprecated)]
            PolicyViolation::ApprovalTimedOut(t)          => write!(f, "approval timed out for: {t}"),
            PolicyViolation::ApprovalVetoed(t)            => write!(f, "approval vetoed for: {t}"),
            PolicyViolation::GrantNotPresented(t)         => write!(f, "no grant presented for: {t}"),
            PolicyViolation::GrantRefused(why)            => write!(f, "grant refused: {why}"),
        }
    }
}

impl std::error::Error for PolicyViolation {}

// ── AgentStateMachine ─────────────────────────────────────────────────────────

/// Drives an agent through typed execution states with policy guards and
/// mesh-visible state propagation.
///
/// See [`AgentPolicy`] for available guards. See the module docs for the full
/// state transition diagram.
///
/// Obtain via [`GossipAgent::agent_state_machine`](crate::GossipAgent::agent_state_machine).
pub struct AgentStateMachine {
    ctx:            Arc<TaskCtx>,
    policy:         RwLock<AgentPolicy>,
    current:        Mutex<ExecutionState>,
    /// Incremented on every `→ Planning` transition; reset on Done/Failed/Idle.
    turn_count:     AtomicUsize,
    /// Incremented on every `→ Invoking` transition; reset on Done/Failed/Idle.
    call_count:     AtomicUsize,
    /// Task identifier used to namespace `agent/{node}/task/{id}/turn|calls` KV keys.
    task_id:        Mutex<Arc<str>>,
    /// Back-reference used by the state-timeout executor task to call `transition`.
    weak_self:      Weak<Self>,
    /// Handle for the active state-timeout task; replaced on every committed transition.
    timeout_handle: Mutex<Option<JoinHandle<()>>>,
}

impl AgentStateMachine {
    pub(super) fn new(ctx: Arc<TaskCtx>, policy: AgentPolicy) -> Arc<Self> {
        let sm = Arc::new_cyclic(|weak| Self {
            ctx,
            policy:         RwLock::new(policy),
            current:        Mutex::new(ExecutionState::Idle),
            turn_count:     AtomicUsize::new(0),
            call_count:     AtomicUsize::new(0),
            task_id:        Mutex::new(Arc::from("current")),
            weak_self:      Weak::clone(weak),
            timeout_handle: Mutex::new(None),
        });
        sm.write_policy_kv();
        sm
    }

    /// Replace the active policy atomically.
    ///
    /// The new policy takes effect on the next call to [`transition`](Self::transition).
    /// In-flight transitions that have already passed a guard complete under the old policy.
    /// Writes `agent/{node}/policy` to the gossip KV store so supervisors observe the change.
    pub fn set_policy(&self, policy: AgentPolicy) {
        *self.policy.write() = policy;
        self.write_policy_kv();
    }

    /// Read a snapshot of the current policy.
    pub fn policy(&self) -> AgentPolicy {
        self.policy.read().clone()
    }

    /// Set the task identifier used in the gossip KV counter keys
    /// (`agent/{node}/task/{id}/turn` and `agent/{node}/task/{id}/calls`).
    ///
    /// Call before transitioning to `Planning` to namespace a new task's counters.
    /// Defaults to `"current"` if never set.
    #[deprecated(since = "2.33.0", note = "no caller, test, SDK or document in the repository references it (post-360 hardening row J, 2026-10-12); kept on the 2.x line — open an issue if you use it and it will be undeprecated")]
    pub fn set_task_id(&self, id: impl Into<Arc<str>>) {
        *self.task_id.lock() = id.into();
    }

    /// Current state — O(1) in-memory read.
    ///
    /// The KV key `agent/{node}/state` is the durable, mesh-visible copy;
    /// this field is always in sync with it after a successful `transition`.
    pub fn state(&self) -> ExecutionState {
        self.current.lock().clone()
    }

    /// Attempt a state transition.
    ///
    /// Runs all policy guards. On success: updates in-memory state, writes
    /// `agent/{node}/state` to the gossip KV store, and emits an `agent.state`
    /// signal so the whole mesh observes the change. On failure: returns
    /// [`PolicyViolation`] and leaves state unchanged.
    ///
    /// **Commit discipline (M2 Run-28 fix):** guards are fast-failed against a
    /// snapshot, but the *authoritative* check happens at commit time inside
    /// [`try_commit`](Self::try_commit) — a validate-and-swap under the state
    /// lock, retried if the state moved (e.g. a timeout fired during the
    /// approval `await`). Budget counters are checked *and reserved* while the
    /// lock is held, so two racing transitions can never both pass the same
    /// last budget slot, and a state committed concurrently is never silently
    /// overwritten — the transition is re-validated against it instead.
    pub async fn transition(&self, to: ExecutionState) -> Result<(), PolicyViolation> {
        // Fast-fail guards against a snapshot (cheap early exit before the
        // approval round-trip; the commit re-validates authoritatively).
        let from_snapshot = self.current.lock().clone();
        self.check_sync_guards(&from_snapshot, &to)?;

        // Adopter-surface X3: a tool that requires a grant cannot be invoked without the arguments to bind it to;
        // the door for it is `transition_invoking`. Default-deny.
        if let ExecutionState::Invoking { tool } = &to
            && self.policy.read().require_grant_for.contains(tool)
        {
            return Err(PolicyViolation::GrantNotPresented(format!("{tool}: a grant-required tool is invoked through transition_invoking, with its arguments")));
        }
        // Async guard: supervisor approval. The approval concerns the *tool*,
        // not the state, so it is requested once — not per commit retry.
        let needs_approval = if let ExecutionState::Invoking { tool } = &to {
            self.policy.read().require_approval_for.contains(tool)
        } else {
            false
        };
        if needs_approval
            && let ExecutionState::Invoking { tool } = &to {
                self.await_approval(tool).await?;
            }

        // Snapshot the budget limits before the commit loop: `try_commit` must
        // not take `policy` while holding `current` (single-lock discipline).
        let (max_turns, tool_budget) = {
            let p = self.policy.read();
            (p.max_turns, p.tool_budget)
        };

        // Validate-and-swap commit loop: re-read the state, re-run the guards
        // against it, and commit only if it is still the state we validated.
        let from = loop {
            let expected = self.current.lock().clone();
            self.check_sync_guards(&expected, &to)?;
            match self.try_commit(&expected, &to, max_turns, tool_budget)? {
                Some(from) => break from,
                None       => continue, // state moved between validate and lock — re-validate
            }
        };

        // Post-commit effects: mesh-visible counter KV writes (counters were
        // already reserved atomically inside try_commit).
        match &to {
            ExecutionState::Planning => {
                self.write_counter_kv("turn", self.turn_count.load(Ordering::Relaxed));
            }
            ExecutionState::Invoking { .. } => {
                self.write_counter_kv("calls", self.call_count.load(Ordering::Relaxed));
            }
            ExecutionState::Idle | ExecutionState::Done | ExecutionState::Failed { .. } => {
                self.write_counter_kv("turn", 0);
                self.write_counter_kv("calls", 0);
            }
            _ => {}
        }

        self.write_kv_and_signal(&from, &to);
        self.arm_state_timeout(&to);
        Ok(())
    }

    /// Cancel any prior timeout; arm a new one if policy specifies one for this state.
    fn arm_state_timeout(&self, to: &ExecutionState) {
        if let Some(h) = self.timeout_handle.lock().take() { h.abort(); }
        if let Some(dur) = self.policy.read().state_timeouts.get(to).copied() {
            let weak = Weak::clone(&self.weak_self);
            let state_snap = to.clone();
            let h = tokio::spawn(async move {
                tokio::time::sleep(dur).await;
                // After sleeping, call the sync commit path — never await on `transition()`
                // from a spawned task (the returned future is !Send due to internal guards).
                if let Some(sm) = weak.upgrade() {
                    sm.force_failed_transition(&state_snap, dur);
                }
            });
            *self.timeout_handle.lock() = Some(h);
        }
    }

    /// Returns a `watch::Receiver` whose value is the current snapshot of all
    /// agent states visible in the mesh (read from `agent/*/state` KV entries).
    ///
    /// The receiver is updated whenever any `agent/` KV key changes. Callers
    /// typically `await changed()` then read the new value.
    pub fn watch_mesh_states(&self) -> watch::Receiver<Vec<(NodeId, ExecutionState)>> {
        let kv = Arc::clone(&self.ctx.kv_state);
        let initial = scan_agent_states(&kv);
        let (tx, rx) = watch::channel(initial);
        let mut change_rx = subscribe_prefix_on_kv(&kv, Arc::from("agent/"));
        tokio::spawn(async move {
            loop {
                if change_rx.changed().await.is_err() { break; }
                let states = scan_agent_states(&kv);
                if tx.send(states).is_err() { break; }
            }
        });
        rx
    }

    // ─── private ──────────────────────────────────────────────────────────────

    /// Validate-and-swap commit: under the state lock, verifies the state is still
    /// `expected`, re-checks the budget guards against the *current* counters, commits
    /// `to`, and reserves the counters — one atomic step. Returns `Ok(Some(from))` on
    /// commit, `Ok(None)` when the state changed since `expected` was read (the caller
    /// re-validates and retries), `Err` when a budget is exhausted.
    ///
    /// Budget counters only move while this lock is held (here and in
    /// [`force_failed_transition`](Self::force_failed_transition)), which is what makes
    /// the check-then-reserve atomic — the M2 Run-28 race (two approval-gated
    /// transitions both passing `tool_budget = 1`) is closed by this.
    /// `max_turns` / `tool_budget` are passed in pre-read: never take `policy` while
    /// holding `current` (single-lock discipline, CLAUDE.md lock-order table).
    fn try_commit(
        &self,
        expected:    &ExecutionState,
        to:          &ExecutionState,
        max_turns:   Option<usize>,
        tool_budget: Option<usize>,
    ) -> Result<Option<ExecutionState>, PolicyViolation> {
        let mut guard = self.current.lock();
        if *guard != *expected {
            return Ok(None);
        }
        if matches!(to, ExecutionState::Invoking { .. }) {
            if let Some(max) = max_turns
                && self.turn_count.load(Ordering::Relaxed) >= max {
                    return Err(PolicyViolation::TurnBudgetExceeded);
                }
            if let Some(max) = tool_budget
                && self.call_count.load(Ordering::Relaxed) >= max {
                    return Err(PolicyViolation::ToolBudgetExceeded);
                }
        }
        let from = std::mem::replace(&mut *guard, to.clone());
        match to {
            ExecutionState::Planning        => { self.turn_count.fetch_add(1, Ordering::Relaxed); }
            ExecutionState::Invoking { .. } => { self.call_count.fetch_add(1, Ordering::Relaxed); }
            ExecutionState::Idle | ExecutionState::Done | ExecutionState::Failed { .. } => {
                self.turn_count.store(0, Ordering::Relaxed);
                self.call_count.store(0, Ordering::Relaxed);
            }
            _ => {}
        }
        Ok(Some(from))
    }

    fn check_sync_guards(&self, from: &ExecutionState, to: &ExecutionState) -> Result<(), PolicyViolation> {
        let policy = self.policy.read();

        // required_capabilities: block Idle → Planning until all filters resolve
        if *from == ExecutionState::Idle
            && let ExecutionState::Planning = to {
                for filter in &policy.required_capabilities {
                    if resolve_filter_against_kv(&self.ctx.kv_state, filter).is_empty() {
                        return Err(PolicyViolation::RequiredCapabilityMissing(
                            format!("{}/{}", filter.namespace, filter.name),
                        ));
                    }
                }
            }

        if let ExecutionState::Invoking { tool } = &to {
            if let Some(max) = policy.max_turns
                && self.turn_count.load(Ordering::Relaxed) >= max {
                    return Err(PolicyViolation::TurnBudgetExceeded);
                }
            if let Some(max) = policy.tool_budget
                && self.call_count.load(Ordering::Relaxed) >= max {
                    return Err(PolicyViolation::ToolBudgetExceeded);
                }
            if !policy.allowed_tools.is_empty() && !policy.allowed_tools.contains(tool) {
                return Err(PolicyViolation::ToolNotAllowed(tool.clone()));
            }
            if policy.denied_tools.contains(tool) {
                return Err(PolicyViolation::ToolDenied(tool.clone()));
            }
        }

        Ok(())
    }

    /// `→ Invoking { tool }` with the call's arguments (adopter-surface X3). For a tool in
    /// [`AgentPolicy::require_grant_for`]: writes the grant request under `grant/{agent}/{request}` — the agent's
    /// principal (`node:{self}`), the tool, the arguments' digest and a bounded rendering of them, the deployed policy
    /// revision when known, and the expiry — then waits [`AgentPolicy::grant_window`] for `…/issued`. A grant that
    /// arrives is verified by this node's execution authority (holder, signature, binding, window, revocation) with
    /// a possession proof signed by this node's identity key; it is returned so the caller can present it on the
    /// call (`_meta.mandate`), where the gateway and the provider verify it again. Silence refuses
    /// `GrantNotPresented`; a supervisor's refusal or a grant that does not verify refuses `GrantRefused`. A pending
    /// request for the same tool and digest, not yet expired, is waited on rather than asked again (a restart does
    /// not ask twice). For any other tool this is [`transition`](Self::transition), and `Ok(None)`.
    ///
    /// The window is read on the wall clock (`Hlc::decision_now_ms`), so a quiet node's grant expires; the wait
    /// itself sleeps on the timer seam and takes no lock.
    pub async fn transition_invoking(
        &self,
        tool: &str,
        arguments: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, PolicyViolation> {
        if !self.policy.read().require_grant_for.contains(&tool.to_string()) {
            self.transition(ExecutionState::Invoking { tool: tool.to_string() }).await?;
            return Ok(None);
        }
        // The agent's own guards first — a tool that is denied, over budget or out of turn asks no one (#673's review,
        // finding 5); the commit re-validates them against the state as it then is.
        let to = ExecutionState::Invoking { tool: tool.to_string() };
        let snapshot = self.current.lock().clone();
        self.check_sync_guards(&snapshot, &to)?;
        let (presented, request) = self.await_grant(tool, arguments).await?;
        // A tool in both modes passes both: the grant, then the veto window (#673's review).
        if self.policy.read().require_approval_for.contains(&tool.to_string()) {
            self.await_approval(tool).await?;
        }
        // The guards that do not concern the grant run as for any invocation; the grant gate above is passed by
        // taking the grant-required path, so the plain transition's refusal is bypassed here deliberately.
        self.commit_invoking_granted(tool).await?;
        // One grant, one commit at this agent: the request is marked consumed, so the same tool and arguments ask
        // again rather than re-reading this grant (#673's review, finding 3). At the doors a grant is single-use
        // too, claimed by each door's preflight (the second PR).
        self.consume_grant(&request);
        Ok(Some(presented))
    }

    /// [`transition`](Self::transition) to `Invoking { tool }` for a tool whose grant was just verified: the same
    /// guards and commit, minus the grant gate.
    async fn commit_invoking_granted(&self, tool: &str) -> Result<(), PolicyViolation> {
        let to = ExecutionState::Invoking { tool: tool.to_string() };
        let from_snapshot = self.current.lock().clone();
        self.check_sync_guards(&from_snapshot, &to)?;
        let (max_turns, tool_budget) = {
            let p = self.policy.read();
            (p.max_turns, p.tool_budget)
        };
        let from = loop {
            let expected = self.current.lock().clone();
            self.check_sync_guards(&expected, &to)?;
            match self.try_commit(&expected, &to, max_turns, tool_budget)? {
                Some(from) => break from,
                None => continue,
            }
        };
        self.write_counter_kv("calls", self.call_count.load(Ordering::Relaxed));
        self.write_kv_and_signal(&from, &to);
        self.arm_state_timeout(&to);
        Ok(())
    }

    #[cfg(all(feature = "gateway", feature = "tls"))]
    /// The grant request's key under the route-owned `grant/` namespace: `grant/{agent principal}/{request}`.
    fn grant_key(&self, request: &str) -> String {
        format!("grant/{}/{request}", super::gateway_caller::node_principal(&self.ctx.node_id))
    }

    /// Without the gateway and TLS features there is no authority to verify a grant and no identity to prove
    /// possession: the grant mode stays default-deny and refuses at once, by name.
    #[cfg(not(all(feature = "gateway", feature = "tls")))]
    async fn await_grant(&self, tool: &str, _arguments: &serde_json::Value) -> Result<(serde_json::Value, String), PolicyViolation> {
        Err(PolicyViolation::GrantNotPresented(format!("{tool}: a grant needs the `gateway` and `tls` features (an execution authority and this node's identity); this build has neither")))
    }

    /// Write, wait, verify (see [`transition_invoking`](Self::transition_invoking)). Returns the presented grant as
    /// the JSON `_meta.mandate` carries.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    async fn await_grant(&self, tool: &str, arguments: &serde_json::Value) -> Result<(serde_json::Value, String), PolicyViolation> {
        use super::gateway_authority::{arguments_digest, possession_request, PresentedMandate};
        use crate::mandate::grant::{possession_message, SignedMandateGrant};
        let window_ms = self.policy.read().grant_window.unwrap_or(Duration::from_secs(30)).as_millis() as u64;
        let agent = super::gateway_caller::node_principal(&self.ctx.node_id);
        let digest = arguments_digest(arguments);
        let digest_hex = super::action_evaluator::hex32(&digest);
        let now_ms = self.ctx.hlc.decision_now_ms();
        let prefix = format!("grant/{agent}/");
        // A pending request for the same tool and digest is waited on, not asked again (a restart does not ask twice)
        // — unless its grant was already consumed here, in which case this is a new call and a new request.
        let entries = crate::store::scan_kv_prefix(&self.ctx.kv_state, &prefix);
        let consumed: std::collections::HashSet<String> = entries.iter().filter_map(|(k, _)| k.strip_suffix("/consumed").and_then(|k| k.strip_prefix(prefix.as_str())).map(str::to_string)).collect();
        let existing = entries.iter().find_map(|(key, bytes)| {
            let rest = key.strip_prefix(prefix.as_str())?;
            if rest.contains('/') || consumed.contains(rest) { return None; }
            let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
            (v["tool"] == tool && v["arguments_digest"] == digest_hex.as_str() && v["expires_at_ms"].as_u64()? > now_ms).then(|| rest.to_string())
        });
        let (request, expires_at_ms) = match existing {
            Some(r) => {
                let v: serde_json::Value = serde_json::from_slice(&mycelium_core::ops::kv_get(&self.ctx, &self.grant_key(&r)).unwrap_or_default()).unwrap_or_default();
                (r, v["expires_at_ms"].as_u64().unwrap_or(now_ms + window_ms))
            }
            None => {
                let request = format!("{:016x}", self.ctx.hlc.tick());
                let expires_at_ms = now_ms + window_ms;
                let (class, renderable) = {
                    let p = self.policy.read();
                    (p.grant_arguments_class.clone(), p.grant_renderable_classes.clone())
                };
                let rendering = render_arguments_under(arguments, class.as_deref(), &renderable);
                let record = serde_json::json!({
                    "schema": "mycelium.grant-request/1",
                    "request": request,
                    "agent": agent,
                    "node": self.ctx.node_id.to_string(),
                    "tool": tool,
                    "operation": "tools/call",
                    "resource": format!("tool:{tool}@{}", self.ctx.node_id),
                    "arguments_digest": digest_hex,
                    "arguments": rendering,
                    "arguments_class": class,
                    "policy_revision": self.ctx.deployed_policy_revision.load_full().map(|r| (*r).clone()),
                    "requested_at_ms": now_ms,
                    "expires_at_ms": expires_at_ms,
                });
                self.write_kv(&self.grant_key(&request), record.to_string().into_bytes());
                #[cfg(feature = "metrics")]
                {
                    metrics::counter!("mycelium_grants_requested_total").increment(1);
                    metrics::gauge!("mycelium_grants_pending").set(pending_grants(&self.ctx, now_ms) as f64);
                }
                (request, expires_at_ms)
            }
        };
        let issued_key: Arc<str> = format!("{}/issued", self.grant_key(&request)).into();
        let refused_key: Arc<str> = format!("{}/refused", self.grant_key(&request)).into();
        let mut rx = mycelium_core::kv_handle::KvHandle::from_core(Arc::clone(&self.ctx.core)).subscribe_prefix(format!("{}/", self.grant_key(&request)));
        // An unknown standing is waited out, not refused: the issuer's checkpoint may land after the grant (#678's
        // review, finding 3); the revoked standing is final; the expiry names the last reason seen.
        let mut last_unknown: Option<String> = None;
        loop {
            if let Some(bytes) = mycelium_core::ops::kv_get(&self.ctx, &refused_key) {
                let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
                return Err(PolicyViolation::GrantRefused(format!("{tool}: refused by {} — {}", v["issuer"].as_str().unwrap_or("?"), v["reason"].as_str().unwrap_or("no reason given"))));
            }
            if let Some(bytes) = mycelium_core::ops::kv_get(&self.ctx, &issued_key) {
                let grant: SignedMandateGrant = serde_json::from_slice(&bytes)
                    .map_err(|e| PolicyViolation::GrantRefused(format!("{tool}: the issued grant does not parse: {e}")))?;
                // Possession: this node signs the request it is about to make with its identity key.
                let Some(tls) = self.ctx.tls.get() else {
                    return Err(PolicyViolation::GrantRefused(format!("{tool}: a grant needs this node's identity ([tls]) to prove possession")));
                };
                let resource = format!("tool:{tool}@{}", self.ctx.node_id);
                let request_bytes = possession_request("tools/call", &resource, &digest);
                let proof = mycelium_core::tls::sign_bytes(&tls.signing_key(), &possession_message(&grant.mandate, &request_bytes));
                let presented = PresentedMandate { grant, possession: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, proof) };
                // Verified here, by this node's own authority, before the transition commits.
                let Some(authority) = self.ctx.execution_authority.get() else {
                    return Err(PolicyViolation::GrantRefused(format!("{tool}: no execution authority is attached to verify the grant")));
                };
                let now = self.ctx.hlc.decision_now_ms();
                // The grant's checkpoint, written beside it by the issuing node — this one or another — so the
                // standing is known here before the grant is assessed (adopter-surface X3, second PR).
                super::gateway_authority::feed_grant_checkpoint(&self.ctx, authority, &presented.grant.mandate, now);
                let assessment = authority.assess(&agent, "tools/call", &resource, &digest, Some(&presented), now, &super::gateway_member_keys(&self.ctx));
                match assessment.binding.map(|b| b.state) {
                    Some(super::action_evaluator::MandateState::Established) => {
                        #[cfg(feature = "metrics")]
                        metrics::counter!("mycelium_grants_verified_total").increment(1);
                        return serde_json::to_value(&presented)
                            .map(|v| (v, request.clone()))
                            .map_err(|e| PolicyViolation::GrantRefused(format!("{tool}: the grant does not serialise: {e}")));
                    }
                    Some(super::action_evaluator::MandateState::Refused(r)) => return Err(PolicyViolation::GrantRefused(format!("{tool}: {r}"))),
                    Some(super::action_evaluator::MandateState::Unknown(why)) if why == super::gateway_authority::GRANT_REVOKED => {
                        return Err(PolicyViolation::GrantRefused(format!("{tool}: not established — {why}")));
                    }
                    // Not established *yet*: the issuer's checkpoint may land after the grant did. Wait on, to the expiry.
                    Some(super::action_evaluator::MandateState::Unknown(why)) => last_unknown = Some(why),
                    None => last_unknown = Some("the grant was not established".into()),
                }
            }
            let now = self.ctx.hlc.decision_now_ms();
            if now >= expires_at_ms {
                #[cfg(feature = "metrics")]
                metrics::counter!("mycelium_grants_expired_total").increment(1);
                return Err(PolicyViolation::GrantNotPresented(format!(
                    "{tool}: no established grant within {window_ms} ms (request {request}){}",
                    last_unknown.map(|w| format!("; the grant seen was not established — {w}")).unwrap_or_default()
                )));
            }
            // Wake on a write under the request, or on the timer seam at the expiry — whichever first.
            let remaining = expires_at_ms - now;
            tokio::select! {
                changed = rx.changed() => { if changed.is_err() { return Err(PolicyViolation::GrantNotPresented(format!("{tool}: the request's watch closed"))); } }
                _ = mycelium_core::sim_seam::sleep_ms("agent/grant-wait", remaining.min(1_000)) => {}
            }
        }
    }

    /// The grant's request is consumed by the commit it admitted (X3): a marker beside it, so a later call with the
    /// same tool and arguments asks again.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    fn consume_grant(&self, request: &str) {
        self.write_kv(&format!("{}/consumed", self.grant_key(request)), b"1".to_vec());
    }
    #[cfg(not(all(feature = "gateway", feature = "tls")))]
    fn consume_grant(&self, _request: &str) {}

    /// One KV write from the state machine: applied, **appended to the WAL** when the node persists (the request must
    /// survive this node's restart — #673's review, finding 4), then gossiped.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    fn write_kv(&self, key: &str, value: Vec<u8>) {
        write_owned_kv(&self.ctx, key, value);
    }

    /// Emits `AGENT_APPROVE` and waits up to 30 s for a matching `AGENT_VETO`.
    /// Silence = approved (no violation). Explicit veto = `ApprovalVetoed`.
    async fn await_approval(&self, tool: &str) -> Result<(), PolicyViolation> {
        let mut veto_rx = self.ctx.signal_handlers.register(signal_kind::AGENT_VETO.into());
        let payload = serde_json::json!({
            "tool": tool,
            "node": self.ctx.node_id.to_string(),
        });
        emit_signal(
            &self.ctx,
            signal_kind::AGENT_APPROVE.into(),
            SignalScope::Cluster,
            Bytes::from(payload.to_string()),
        );
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while let Ok(Some(sig)) = tokio::time::timeout_at(deadline, veto_rx.recv()).await {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&sig.payload)
                && v.get("tool").and_then(|t| t.as_str()) == Some(tool) {
                    return Err(PolicyViolation::ApprovalVetoed(tool.to_string()));
                }
            // Veto for a different tool — keep waiting
        }
        Ok(())
    }

    fn write_kv_and_signal(&self, from: &ExecutionState, to: &ExecutionState) {
        let state_str = to.to_kv_str();
        let key: Arc<str> = format!("agent/{}/state", self.ctx.node_id).into();
        let update = make_gossip_update(
            &self.ctx.node_id,
            self.ctx.default_ttl,
            key,
            Bytes::copy_from_slice(state_str.as_bytes()),
            false,
            &self.ctx.hlc,
        );
        apply_and_notify(&self.ctx.kv_state, &update);
        dispatch_gossip_try_send(
            &self.ctx.gossip_txs,
            crate::framing::local_kv_wire_msg(&self.ctx, update),
            self.ctx.node_id.id_hash(),
            ForwardHint::All,
            &self.ctx.kv_state.dropped_frames,
        );
        let payload = serde_json::json!({
            "node": self.ctx.node_id.to_string(),
            "from": from.to_kv_str(),
            "to":   &state_str,
        });
        emit_signal(
            &self.ctx,
            signal_kind::AGENT_STATE.into(),
            SignalScope::Cluster,
            Bytes::from(payload.to_string()),
        );
    }

    /// Bypass-policy commit used exclusively by the state-timeout executor.
    ///
    /// Only commits if `self.state() == expected` to guard against races where
    /// the agent already transitioned away before the timer fired.
    fn force_failed_transition(&self, expected: &ExecutionState, dur: Duration) {
        let to = ExecutionState::Failed {
            reason: format!("timeout after {dur:?} in state {}", expected.to_kv_str()),
        };
        let from = {
            let mut guard = self.current.lock();
            if *guard != *expected { return; }
            let from = std::mem::replace(&mut *guard, to.clone());
            // Counters only move under the state lock (see try_commit).
            self.turn_count.store(0, Ordering::Relaxed);
            self.call_count.store(0, Ordering::Relaxed);
            from
        };
        self.write_counter_kv("turn", 0);
        self.write_counter_kv("calls", 0);
        self.write_kv_and_signal(&from, &to);
        if let Some(h) = self.timeout_handle.lock().take() { h.abort(); }
    }

    fn write_counter_kv(&self, suffix: &str, value: usize) {
        let task_id = self.task_id.lock().clone();
        let key: Arc<str> = format!("agent/{}/task/{}/{}", self.ctx.node_id, task_id, suffix).into();
        let update = make_gossip_update(
            &self.ctx.node_id,
            self.ctx.default_ttl,
            key,
            Bytes::from(value.to_string()),
            false,
            &self.ctx.hlc,
        );
        apply_and_notify(&self.ctx.kv_state, &update);
        dispatch_gossip_try_send(
            &self.ctx.gossip_txs,
            crate::framing::local_kv_wire_msg(&self.ctx, update),
            self.ctx.node_id.id_hash(),
            ForwardHint::All,
            &self.ctx.kv_state.dropped_frames,
        );
    }

    fn write_policy_kv(&self) {
        let encoded = match serde_json::to_vec(&*self.policy.read()) {
            Ok(v) => Bytes::from(v),
            Err(_) => return,
        };
        let key: Arc<str> = format!("agent/{}/policy", self.ctx.node_id).into();
        let update = make_gossip_update(
            &self.ctx.node_id,
            self.ctx.default_ttl,
            key,
            encoded,
            false,
            &self.ctx.hlc,
        );
        apply_and_notify(&self.ctx.kv_state, &update);
        dispatch_gossip_try_send(
            &self.ctx.gossip_txs,
            crate::framing::local_kv_wire_msg(&self.ctx, update),
            self.ctx.node_id.id_hash(),
            ForwardHint::All,
            &self.ctx.kv_state.dropped_frames,
        );
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

impl Drop for AgentStateMachine {
    fn drop(&mut self) {
        if let Some(h) = self.timeout_handle.get_mut().take() {
            h.abort();
        }
    }
}

#[cfg(all(feature = "gateway", feature = "tls"))]
/// Open grant requests at this node — the gauge `mycelium_grants_pending` (X3).
#[cfg(all(feature = "gateway", feature = "tls", feature = "metrics"))]
pub(crate) fn pending_grants(ctx: &TaskCtx, now_ms: u64) -> usize {
    let entries = crate::store::scan_kv_prefix(&ctx.kv_state, "grant/");
    let answered: std::collections::HashSet<String> = entries.iter().filter_map(|(k, _)| {
        k.strip_suffix("/issued").or_else(|| k.strip_suffix("/refused")).map(str::to_string)
    }).collect();
    entries.iter().filter(|(k, v)| {
        k.matches('/').count() == 2 && !answered.contains(k.as_ref())
            && serde_json::from_slice::<serde_json::Value>(v).ok().and_then(|r| r["expires_at_ms"].as_u64()).is_some_and(|e| e > now_ms)
    }).count()
}

#[cfg(all(feature = "gateway", feature = "tls"))]
/// A human reads arguments, not a digest: the request record carries the arguments as JSON when they fit the cap,
/// else a summary naming the size (the digest is beside it either way). Bounded well under the KV write cap. The
/// data-class policy is applied by [`render_arguments_under`], which calls this for a class it renders.
pub(crate) fn render_arguments(arguments: &serde_json::Value) -> serde_json::Value {
    const CAP: usize = 16 * 1024;
    let text = arguments.to_string();
    if text.len() <= CAP {
        arguments.clone()
    } else {
        serde_json::json!({ "withheld": "over the rendering cap", "bytes": text.len(), "cap": CAP })
    }
}

/// [`render_arguments`] under the data-class policy (adopter-surface X3, third PR): arguments of a class the policy
/// does not list as renderable are **withheld** — shown as withheld with the class and the size, never as empty —
/// and the request record carries the digest beside it, which is what the grant binds. Unclassified arguments are
/// rendered under the cap.
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) fn render_arguments_under(arguments: &serde_json::Value, class: Option<&str>, renderable: &[String]) -> serde_json::Value {
    match class {
        Some(c) if !renderable.iter().any(|r| r == c) => serde_json::json!({
            "withheld": format!("data class {c}: not rendered into the gossiped request (the policy renders {})", if renderable.is_empty() { "no class".to_string() } else { renderable.join(", ") }),
            "class": c,
            "bytes": arguments.to_string().len(),
        }),
        _ => render_arguments(arguments),
    }
}

/// One KV write under a route-owned namespace, made synchronously: applied, **appended to the WAL** when the node
/// persists (the record must survive this node's restart — #673's review, finding 4), then gossiped. The state
/// machine's own writes and the doors' single-use markers (adopter-surface X3, second PR) go through it.
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) fn write_owned_kv(ctx: &Arc<super::TaskCtx>, key: &str, value: Vec<u8>) {
    let update = make_gossip_update(&ctx.node_id, ctx.default_ttl, Arc::from(key), Bytes::from(value), false, &ctx.hlc);
    apply_and_notify(&ctx.kv_state, &update); // apply, then persist (persistence.rs invariant 1)
    if let Some(wal) = ctx.wal.get() {
        wal.append_try(crate::framing::sync_entry_from(&update));
    }
    dispatch_gossip_try_send(
        &ctx.gossip_txs,
        crate::framing::local_kv_wire_msg(ctx, update),
        ctx.node_id.id_hash(),
        ForwardHint::All,
        &ctx.kv_state.dropped_frames,
    );
}

/// Snapshot all `agent/{node}/state` entries from the local KV store.
fn scan_agent_states(kv: &crate::store::KvState) -> Vec<(NodeId, ExecutionState)> {
    let guard = kv.store.pin();
    guard
        .iter()
        .filter_map(|(key, entry)| {
            if !key.starts_with("agent/") || !key.ends_with("/state") {
                return None;
            }
            let data = entry.data.as_ref()?;
            // Strip "agent/" prefix and "/state" suffix to get the node_id string.
            let inner = key.get("agent/".len()..key.len() - "/state".len())?;
            let node_id: NodeId = inner.parse().ok()?;
            let state = ExecutionState::from_kv_str(std::str::from_utf8(data).unwrap_or(""));
            Some((node_id, state))
        })
        .collect()
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kv_roundtrip() {
        let cases = [
            ExecutionState::Idle,
            ExecutionState::Planning,
            ExecutionState::Reflecting,
            ExecutionState::Done,
            ExecutionState::Invoking   { tool:    "weather".into() },
            ExecutionState::Failed     { reason:  "budget exceeded".into() },
            ExecutionState::Suspended  { task_id: "task-42".into() },
            ExecutionState::Custom("my-state".into()),
        ];
        for s in &cases {
            assert_eq!(*s, ExecutionState::from_kv_str(&s.to_kv_str()), "roundtrip failed for {s:?}");
        }
    }

    /// X3: a grant-required tool is default-deny — a plain transition carries no arguments to bind, so it is refused
    /// at once, and the state does not move. (The full loop — the request written, a grant issued and verified, the
    /// call bound — is `http::grant_tests`.)
    #[tokio::test]
    async fn a_plain_transition_to_a_grant_required_tool_is_refused() {
        let agent = crate::lib_tests::make_agent_for_sm_tests();
        let policy = AgentPolicy { require_grant_for: vec!["deploy".into()], ..Default::default() };
        let sm = agent.agent_state_machine(policy);
        sm.transition(ExecutionState::Planning).await.unwrap();
        let err = sm.transition(ExecutionState::Invoking { tool: "deploy".into() }).await.unwrap_err();
        assert!(matches!(err, PolicyViolation::GrantNotPresented(_)), "{err}");
        assert_eq!(sm.state(), ExecutionState::Planning);
        // Another tool is unaffected, and the grant-free path of `transition_invoking` is the plain transition.
        assert!(sm.transition_invoking("other", &serde_json::json!({})).await.unwrap().is_none());
        assert_eq!(sm.state(), ExecutionState::Invoking { tool: "other".into() });
    }

    #[tokio::test]
    async fn policy_tool_budget() {
        let agent = crate::lib_tests::make_agent_for_sm_tests();
        let policy = AgentPolicy { tool_budget: Some(2), ..Default::default() };
        let sm = agent.agent_state_machine(policy);
        sm.transition(ExecutionState::Planning).await.unwrap();
        sm.transition(ExecutionState::Invoking { tool: "a".into() }).await.unwrap();
        sm.transition(ExecutionState::Reflecting).await.unwrap();
        sm.transition(ExecutionState::Invoking { tool: "b".into() }).await.unwrap();
        sm.transition(ExecutionState::Reflecting).await.unwrap();
        // Third call exceeds budget
        let err = sm.transition(ExecutionState::Invoking { tool: "c".into() }).await;
        assert!(matches!(err, Err(PolicyViolation::ToolBudgetExceeded)), "expected ToolBudgetExceeded");
    }

    #[tokio::test]
    async fn policy_tool_denied() {
        let agent = crate::lib_tests::make_agent_for_sm_tests();
        let policy = AgentPolicy {
            denied_tools: vec!["dangerous".into()],
            ..Default::default()
        };
        let sm = agent.agent_state_machine(policy);
        sm.transition(ExecutionState::Planning).await.unwrap();
        let err = sm.transition(ExecutionState::Invoking { tool: "dangerous".into() }).await;
        assert!(matches!(err, Err(PolicyViolation::ToolDenied(_))), "expected ToolDenied");
        let ok = sm.transition(ExecutionState::Invoking { tool: "safe".into() }).await;
        assert!(ok.is_ok());
    }

    #[tokio::test]
    async fn kv_written_on_transition() {
        let agent = crate::lib_tests::make_agent_for_sm_tests();
        let sm = agent.agent_state_machine(AgentPolicy::default());
        sm.transition(ExecutionState::Planning).await.unwrap();
        let key = format!("agent/{}/state", agent.node_id());
        let val = agent.kv().get(&key);
        assert_eq!(val.as_deref().map(|b| std::str::from_utf8(b).unwrap_or("")), Some("Planning"));
    }

    #[tokio::test]
    async fn counter_kv_written() {
        let agent = crate::lib_tests::make_agent_for_sm_tests();
        let sm = agent.agent_state_machine(AgentPolicy::default());
        let node = agent.node_id();

        sm.transition(ExecutionState::Planning).await.unwrap();
        sm.transition(ExecutionState::Invoking { tool: "a".into() }).await.unwrap();
        sm.transition(ExecutionState::Reflecting).await.unwrap();
        sm.transition(ExecutionState::Planning).await.unwrap();
        sm.transition(ExecutionState::Invoking { tool: "b".into() }).await.unwrap();
        sm.transition(ExecutionState::Done).await.unwrap();

        let kv_turn  = agent.kv().get(&format!("agent/{node}/task/current/turn"));
        let kv_calls = agent.kv().get(&format!("agent/{node}/task/current/calls"));
        let read = |v: Option<bytes::Bytes>| -> usize {
            std::str::from_utf8(&v.unwrap()).unwrap().parse().unwrap()
        };
        // Done resets both to 0
        assert_eq!(read(kv_turn),  0);
        assert_eq!(read(kv_calls), 0);

        // Mid-task values: verify by inspecting after second Planning (turn=2)
        let sm2 = agent.agent_state_machine(AgentPolicy::default());
        sm2.transition(ExecutionState::Planning).await.unwrap();
        sm2.transition(ExecutionState::Planning).await.unwrap();
        let kv_turn2 = agent.kv().get(&format!("agent/{node}/task/current/turn"));
        assert_eq!(read(kv_turn2), 2);
    }

    #[tokio::test]
    async fn state_timeout_fires() {
        let agent = crate::lib_tests::make_agent_for_sm_tests();
        let mut policy = AgentPolicy::default();
        policy.state_timeouts.insert(ExecutionState::Planning, Duration::from_millis(50));
        let sm = agent.agent_state_machine(policy);

        sm.transition(ExecutionState::Planning).await.unwrap();
        // Sleep well past the 50 ms timeout so the spawned task has time to run.
        tokio::time::sleep(Duration::from_millis(150)).await;

        assert!(
            matches!(sm.state(), ExecutionState::Failed { .. }),
            "expected Failed after timeout, got {:?}", sm.state()
        );
    }

    #[tokio::test]
    async fn state_timeout_cancelled_on_transition() {
        let agent = crate::lib_tests::make_agent_for_sm_tests();
        let mut policy = AgentPolicy::default();
        policy.state_timeouts.insert(ExecutionState::Planning, Duration::from_millis(100));
        let sm = agent.agent_state_machine(policy);

        sm.transition(ExecutionState::Planning).await.unwrap();
        // Transition away well before the 100 ms timeout fires.
        tokio::time::sleep(Duration::from_millis(20)).await;
        sm.transition(ExecutionState::Invoking { tool: "x".into() }).await.unwrap();
        // Sleep well past the original deadline — the cancelled task must not fire.
        tokio::time::sleep(Duration::from_millis(250)).await;

        assert!(
            !matches!(sm.state(), ExecutionState::Failed { .. }),
            "timeout fired after transition away; state is {:?}", sm.state()
        );
    }

    #[tokio::test]
    async fn set_policy_takes_effect_and_writes_kv() {
        let agent = crate::lib_tests::make_agent_for_sm_tests();
        let sm = agent.agent_state_machine(AgentPolicy::default());

        // Initial policy is written to KV at construction
        let policy_key = format!("agent/{}/policy", agent.node_id());
        assert!(agent.kv().get(&policy_key).is_some(), "policy KV not written at construction");

        // Tighten: deny "dangerous"
        sm.set_policy(AgentPolicy {
            denied_tools: vec!["dangerous".into()],
            ..Default::default()
        });

        // KV is updated
        let kv_bytes = agent.kv().get(&policy_key).expect("policy KV missing after set_policy");
        let kv_policy: AgentPolicy = serde_json::from_slice(&kv_bytes).expect("policy KV not valid JSON");
        assert_eq!(kv_policy.denied_tools, vec!["dangerous".to_string()]);

        // New policy is enforced on next transition
        sm.transition(ExecutionState::Planning).await.unwrap();
        let err = sm.transition(ExecutionState::Invoking { tool: "dangerous".into() }).await;
        assert!(matches!(err, Err(PolicyViolation::ToolDenied(_))));
    }

    /// M2 Run-28 probe (dim 9 — concurrency), **flipped to a regression gate**
    /// same-day: `transition()` was check-then-act (budget counters read before
    /// the approval `await`, incremented only after commit; commit never
    /// re-read `current`), so two approval-gated `Invoking` transitions racing
    /// through the await both passed `tool_budget = 1`. Fixed by `try_commit`:
    /// a validate-and-swap under the state lock with budget check + reserve as
    /// one atomic step. Exactly one racer may now win the last budget slot; the
    /// loser gets `ToolBudgetExceeded`.
    #[tokio::test(start_paused = true)]
    async fn tool_budget_enforced_under_concurrent_approval_gated_transitions() {
        let agent = crate::lib_tests::make_agent_for_sm_tests();
        let policy = AgentPolicy {
            tool_budget: Some(1),
            require_approval_for: vec!["a".into(), "b".into()],
            ..Default::default()
        };
        let sm = agent.agent_state_machine(policy);
        sm.transition(ExecutionState::Planning).await.unwrap();

        // Both futures pass the fast-fail check (call_count = 0 < 1) and park on
        // the 30 s approval window (paused time auto-advances) — the commit loop
        // must serialise them.
        let (r1, r2) = tokio::join!(
            sm.transition(ExecutionState::Invoking { tool: "a".into() }),
            sm.transition(ExecutionState::Invoking { tool: "b".into() }),
        );
        let ok = u32::from(r1.is_ok()) + u32::from(r2.is_ok());
        assert_eq!(
            ok, 1,
            "tool_budget=1 must admit exactly one of two racing Invoking transitions, got {ok} \
             (r1={r1:?}, r2={r2:?})"
        );
        let (winner_tool, loser) = if r1.is_ok() { ("a", r2) } else { ("b", r1) };
        assert!(
            matches!(loser, Err(PolicyViolation::ToolBudgetExceeded)),
            "the losing transition must fail with ToolBudgetExceeded, got {loser:?}"
        );
        assert_eq!(sm.state(), ExecutionState::Invoking { tool: winner_tool.into() });
    }
}
