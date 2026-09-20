//! # mycelium — gossip substrate for adaptive AI agent systems
//!
//! An embedded, broker-less library that provides two primitives:
//!
//! - **Layer 1 — KV store**: epidemic last-write-wins state propagation over TCP.
//!   Every agent holds a eventually-consistent view of the full cluster's key-value state.
//! - **Layer 2 — Signal mesh**: ephemeral scoped events that flood the cluster epidemically.
//!   Each agent holds a local [`Boundary`](signal::Boundary) (its receptor set) that decides
//!   whether it *acts* on an incoming signal — forwarding is always unconditional.
//!
//! Higher layers build Actor/Event systems, async RPC, and MCP AI tool routing on top.
//! Each agent chooses its own payload serialisation; the substrate routes by signal `kind`
//! string and carries opaque [`bytes::Bytes`].
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use mycelium::{GossipAgent, GossipConfig, NodeId, SignalScope, signal_kind};
//! use bytes::Bytes;
//! use std::{sync::Arc, time::Duration};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let node_id = NodeId::new("127.0.0.1", 7946)?;
//!     let mut config = GossipConfig::default();
//!     config.bind_port = 7946;   // the agent listens on `bind_port`, not the NodeId's port — set both
//!     config.bootstrap_peers = vec![NodeId::new("127.0.0.1", 7947)?];
//!
//!     let agent = Arc::new(GossipAgent::new(node_id, config));
//!     agent.start().await?;
//!
//!     // Layer 1 — KV state
//!     let _ = agent.kv().set("load/self", Bytes::from_static(b"queue=0"));
//!     let val = agent.kv().get("load/self");
//!
//!     // Layer 2 — signals
//!     agent.mesh().join_group("nlp");
//!     agent.mesh().emit(signal_kind::INVOKE, SignalScope::Group("nlp".into()), Bytes::new());
//!
//!     agent.shutdown().await;
//!     Ok(())
//! }
//! ```
//!
//! See [`GossipAgent`] for the full API. See [`GossipConfig`] for all tunable parameters.
//! See [ROADMAP.md](https://github.com/RichardEko/mycelium/blob/main/ROADMAP.md) for the
//! layer-by-layer architecture and higher-layer design.
//!
//! **Building a use case on top of Mycelium?** Start with
//! [Building on Mycelium](https://github.com/RichardEko/mycelium/blob/main/docs/guide/building-on-mycelium.md)
//! — the integrator contract (dependency, public-API-only rule, reserved KV prefixes, the
//! invariants to respect, and a copyable `CLAUDE.md` snippet), then the
//! [FAQ](https://github.com/RichardEko/mycelium/blob/main/docs/guide/faq.md).
//!
//! ## Crate layout — `mycelium` vs `mycelium-core`
//!
//! This crate is the **full runtime** (Layers I + II + III: gossip KV, signal mesh,
//! consensus, capabilities, services, the HTTP/MCP/A2A gateway, and TLS/RBAC/audit) and is
//! what most users want. The Layers I + II substrate alone — gossip KV + signal/boundary mesh,
//! with no Axum/gateway and roughly a third of the dependency tree — lives in the separate
//! [`mycelium-core`](https://crates.io/crates/mycelium-core) crate, which `mycelium`
//! re-exports and depends on. Depend on `mycelium-core` directly only for a minimal embed that
//! needs last-write-wins KV propagation and the scoped event mesh but not RPC, consensus, the
//! capability system, or the gateway. The crate boundary makes the inverted-dependency
//! invariant a compile-time guarantee (the substrate cannot reference the layers above it).
//! You can also trim *this* crate toward the core with `default-features = false` (drops the
//! gateway) and `--features gateway` without `consensus` (drops the agreement layer); a
//! consensus-disabled node still forwards PROPOSE/VOTE/COMMIT, it just never acts. The split
//! landed in v2.0 M1 — see [ROADMAP.md](https://github.com/RichardEko/mycelium/blob/main/ROADMAP.md)
//! §v2.0 Milestones for the rationale.
//!
//! ## KV namespace ownership
//!
//! The KV store is the single substrate; higher layers own dedicated key
//! prefixes. Higher layers write directly to their prefix via
//! `make_gossip_update` + `apply_and_notify`. This is intentional and not a
//! layer violation: ownership is documented, encoding is shared, and no
//! foreign writer ever touches another layer's prefix.
//!
//! | Prefix                              | Owner / purpose                                              |
//! |-------------------------------------|--------------------------------------------------------------|
//! | `grp/{group}/{node}`                | Signal Mesh — group membership                               |
//! | `sys/load/{node}/{kind}`            | Signal Mesh — opacity (load + auto-opacity composition)      |
//! | `sys/load/{node}/req/{ns}/{name}`   | Phase 3 requirement opacity (composes via `is_self_opaque`)  |
//! | `sys/load/{node}/group-req/{g}/{i}` | Group-requirement opacity; written by the emergent-group membership task when a `CapabilityGroupDef::requires` filter is unsatisfied |
//! | `sys/quorum/{kind}/{sender}`        | Persistent quorum evidence                                   |
//! | `sys/topology-override/{group}`     | Consensus — operator escape hatch (value: `b"true"`)         |
//! | `sys/health/{node}`                 | Emergent detectors (Phase 2) — periodic store self-report feeding cross-node store-convergence |
//! | `sys/rate/{observer}/{sender}`      | Distributed rate observation (WS-C M7) — per-observer rate evidence, fps as ASCII u64, short-TTL |
//! | `sys/role/{node}`                   | RBAC — signature-verified role claim (checked against `sys/identity/{node}` at read) |
//! | `sys/audit/{node}/{seq}`            | WS2 audit — per-node hash-chained, Ed25519-signed tamper-evident record (`compliance`) |
//! | `audit/{ts_unix_nanos}/{node}`      | SkillRunner's **plain** audit trail *without* `compliance` (`src/bin/skillrunner/audit.rs`) — unsigned, time-keyed JSON; with `compliance` the same records go to `sys/audit/…` instead |
//! | `sys/audit-checkpoint/{node}/{seq}` | WS-D audit — signed mid-chain boundary enabling export-then-prune retention (`compliance`) |
//! | `sys/revocation/{node}/{key-hex}`   | WS-D — signed key revocation; excluded on all verify paths incl. consensus (`compliance`) |
//! | `sys/capauthz/{ns}/{name}`          | Gossip-level capability-authz policy (`required_roles`; resolve-time enforcement) (`compliance`) |
//! | `consensus/committed/{slot}`        | Consensus — committed slot state                             |
//! | `consensus/ballot/{slot}`           | Consensus — ballot tracking                                  |
//! | `consensus/lease/{slot}`            | Consensus — epoch-lease window (u64 LE ms); written when `ConsensusConfig::committed_lease_secs` is set; expiry is evaluated read-side |
//! | `consensus/trust/{group}/{node}`    | Consensus — trust slices                                     |
//! | `cap/{node}/{ns}/{name}`            | Node-level capability advertisements                         |
//! | `cap/{node}/locality/self`          | Locality (also a capability — single namespace, single shape)|
//! | `req/{node}/{ns}/{name}`            | Node-level requirement declarations                          |
//! | `cap-group/{group}`                 | Emergent capability-group definitions                        |
//! | `gcap/{group}/{ns}/{name}/{contrib}`| Group-level capability projections                           |
//! | `mailbox/{target}/{kind}/{hlc_hex}` | Service Patterns — event mailbox entries (value: `sender_len(2LE) | sender_bytes | payload`) |
//! | `schemas/{schema_id}`              | Schema registry — authoritative JSON Schema bytes for a capability `schema_id`; written via `publish_schema`; gossip-propagated and WAL-persisted |
//! | `tools/{name}/{node}`              | Layer IV MCP tool registrations (value: JSON Schema bytes)   |
//! | `agent/{node}/state`               | Layer V agent state machine — current state string (gossips to mesh) |
//! | `agent/{node}/policy`              | Layer V serialised AgentPolicy (readable by monitors/supervisors) |
//! | `agent/{node}/task/{id}/turn`      | Layer V turn counter for `max_turns` enforcement              |
//! | `agent/{node}/task/{id}/calls`     | Layer V tool-call counter for `tool_budget` enforcement       |
//! | `agent/{node}/provision/{item}/error` | Last provisioning failure — written by the **application** provisioning handler, not the substrate |
//! | `sys/identity/{node}`              | mTLS — 32-byte Ed25519 verifying key history (current‖retained); written at startup by TLS-enabled nodes |
//! | `sys/identity-proof/{node}`        | identity-auth Phase 2 — `signer_key(32)‖sig(64)` authenticating the identity entry; peers accept a key only if the proof chains to a trusted key (`tls`) |
//! | `sys/caller-context/{node}`        | v3 item 7 — the node strips + verifies the `GatewayCaller` envelope on its RPC receive path (value `b"1"`, the envelope version); a secure-profile gateway dispatches only to nodes carrying it. Written at start by every node; self-owned (`sys/` tripwire) |
//! | `cap/{node}/llm/inference`         | LLM backend capability (model, context, backend, endpoint attrs) |
//! | `cap/{node}/llm/installable`       | LLM models that can be pulled (model, size_gb, est_mins attrs) |
//! | `cap/{node}/llm/loading`           | LLM model pull in progress; the shipped provisioner writes a `pct` (0–100) attr (the `llm_agent` example's *simulated* pull uses `progress`) |
//! | `cap/{node}/{ns}/installable`      | Any dynamically provisionable software capability             |
//! | `cap/{node}/{ns}/loading`          | Provisioning in progress; `pct` attr 0–100, written by `mycelium-wasm-host`'s `Provisioner` (read by `mycelium-reason`'s `ModelDependency::loading_progress`) |
//! | `svc/{kind}/{node}`                | Persistent capability advertisements (`advertise_persistent`; tombstoned on handle drop) |
//! | `log/{stream}/{hlc_hex}`           | Append-only KV log entries (`KvHandle::append`)              |
//! | `clog/{…}`                         | Consumer positions for group log subscription (`subscribe_log_group`) |
//! | `lock/{name}`                      | Distributed lock state (JSON holder/token/expiry; tombstoned on `LockGuard` drop) |
//! | `prompts/{ns}/{name}`              | LLM prompt templates (`llm` feature; configuration — the `cap/` entry is the presence heartbeat) |
//! | `skills/{ns}/{name}/{node}/input\|output` | SkillRunner skill registrations (signal-kind routing for `skill.invoke`) |
//! | `installable/{ns}/{name}/{hex}`    | `mycelium-wasm-host` — the artifact catalogue (encoded `InstallableEntry`: kind, content address, cost hints, signed resource requirements) |
//! | `comp/{node}/{ns}/…`               | `mycelium-wasm-host` — confined component KV (a WASM guest's scoped subtree; the host's enforcement point) |
//! | `wiki/{group}/proposal/{id}`       | `mycelium-wiki` companion — evaporating edit proposals (drained by the curator) |
//! | `tuple/inflight/{ns}/{id}`         | `mycelium-tuple-space` companion — advisory in-flight claim (JSON value; expiry is read-side, swept by the primary) |
//! | `sys/tuple/{node}/{ns}/…`          | `mycelium-tuple-space` companion — monitoring counters, role, and the backpressure pheromone (`…/pressure/{stage}`) |
//! | `cap/{node}/llm/{model}`           | `mycelium-reason` companion — a served model *is a prompt skill* (`serve_model` → `register_prompt_skill`); the presence cap for `llm/{model}` (template in `prompts/`) |
//! | `cap/{node}/llm-meta/{model}`      | `mycelium-reason` companion — the parallel **attributed** model-metadata ad (`ctx_window`, `family`, extras); separate cap so attribute updates don't LWW-churn the skill's own persist task |
//! | `cap/{node}/reason/blob-cache`     | `mycelium-reason` companion — this node serves content-addressed blobs (RPC `reason.blob.fetch`); `MeshBlobStore` discovers providers via this cap |
//! | `log/reason/{run_id}/{node}/{hlc}` | `mycelium-reason` companion — fleet-reasoning trace substreams (one per writer; a shared stream would collide same-ms HLC keys — `TraceRecorder`/`replay`) |
//! | `ckpt/{thread}/{ns}/{id}`          | `langgraph-checkpoint-mycelium` — LangGraph checkpoint **index** rows (metadata inline; payloads in the blob tier). Written by the Python saver via the gateway KV endpoint |
//! | `ckptw/{thread}/{ns}/{id}/{task}/{idx}` | `langgraph-checkpoint-mycelium` — LangGraph pending-write index rows (one blob per write) |
//! | `facts/{node}/{field}`             | `mycelium-agentfacts` companion — node-signed per-field AgentFacts CRDT publication (`publish_field`; LWW-assembled by readers) |
//! | `mandate/{scope}`                   | **Reserved** (v3 item 5, `docs/design/scoped-mandates.md`) — `(holder, authority epoch)`. An *announcement*; the enforcing check lives inside the protected resource's own atomic boundary |
//! | `log/wiki/{group}/proposals`        | **Reserved** (v3 item 5) — durable wiki proposals via the existing `append` verb; the evaporating KV queue stays a delivery hint |
//! | `knowledge/head/{issuer}/{stream}`  | **Reserved** (v3 item 3, `docs/design/knowledge-layer.md`) — bounded signed *discovery heads* only. Records and evidence live in an authorized store, so LWW moves a pointer and never erases a competing statement |
//! | `rights/head/{holder}`              | **Reserved** (v3 item 4, `docs/design/adaptive-stability.md`) — a holder's bounded signed *claim* of the allocated rights it holds. The ledger itself is node-local, fsynced and never gossiped: a right in the medium would evaporate with its holder's discovery entry and be issued twice |
//! | `cn/{requirement}` · `cn/{requirement}/award` · `log/cn/{requirement}/{offers,reports,assessments}` | `mycelium-commitment` companion (v3 §6.9, CN1) — the contract net as five records: the announcement head and the **one award per requirement** (a receipt-bearing write, refused rather than overwritten) in the medium; offers, reports and assessments as `append` streams. No component assigns another participant's obligation |
//! | `manifest/…`                       | Mesh manifest (`mesh_manifest::manifest_keys`) — `current` · `version` · `history/{ver}` · `control/system` · `control/group/{g}`; the namespace is defined here, written by operator/app code through the public KV API |
//!
//! Layer-III writes that read or write KV (consensus engine,
//! `sys/topology-override` reads) are documented at their call sites as
//! deliberate escape hatches, not layer violations — the consensus engine
//! owns the `consensus/` prefix and reads `sys/topology-override` as a
//! policy input, both of which are explicitly part of its namespace
//! contract.
//!
//! **Ownership is promise-strength, not mechanism-strength.** The substrate
//! does not enforce this table: any node can write any key, and LWW will
//! accept it. Higher layers' invariants (e.g. "committed slots are
//! commit-once") are therefore exactly as strong as every node's compliance
//! with this contract — by design, since teaching Layer I to enforce a
//! higher layer's law would invert the dependency that makes it the
//! foundation. Violations are made *legible* instead: the consensus
//! listener's commit-conflict tripwire ([`SystemStats::commit_conflicts`])
//! detects and refuses to endorse conflicting commits.
//!
//! ## Durability contract
//!
//! Gossip is the only replication mechanism; there is no quorum acknowledgement.
//! For a key to survive a full-cluster restart, **at least one node that holds
//! the key must have `PersistenceConfig` set** so the WAL survives process exit.
//! Nodes without persistence recover via anti-entropy from live peers; they
//! cannot contribute to full-cluster recovery.
//!
//! Capability and soft-state keys (`cap/`, `sys/load/`, `grp/`, `req/`) are
//! **not** WAL-persisted by design — they regenerate via `advertise_capability`
//! within seconds of reconnection. Hard-state application keys (`test/`,
//! `agent/`, `consensus/`, `tools/`) should be written via `set_async` on at
//! least one persistent node per write if restart durability is required.
//!
//! ## Speech act patterns (FIPA-ACL → Mycelium)
//!
//! The table below maps the seven FIPA-ACL performatives to idiomatic Mycelium
//! primitives. Use this as a vocabulary bridge when porting FIPA-based or A2A
//! interaction protocols to the gossip substrate.
//!
//! | FIPA-ACL performative      | Mycelium primitive                                                          | Notes                                                                                      |
//! |----------------------------|-----------------------------------------------------------------------------|--------------------------------------------------------------------------------------------|
//! | `INFORM`                   | [`emit`] / [`emit_ordered`]                                                 | Epidemic broadcast; no acknowledgement. `emit_ordered` adds causal HLC sequencing.        |
//! | `REQUEST`                  | [`rpc_call`] / [`rpc_respond`]                                              | Point-to-point call with correlation nonce; awaits reply or timeout.                       |
//! | `QUERY-IF` / `QUERY-REF`   | [`resolve`] / [`watch_capabilities`]                                        | Snapshot or live-streaming capability satisfaction check.                                  |
//! | `PROPOSE`                  | [`group_propose`] / [`system_propose`] / [`cross_group_propose`]            | Epidemic two-phase voting; `GroupQuorum` controls the acceptance fraction per group.       |
//! | `AGREE` / `REFUSE`         | [`rpc_respond`] or [`emit`] with `SignalScope::Individual`                  | Point-to-point reply to a specific RPC request or correlation nonce.                       |
//! | `SUBSCRIBE`                | [`signal_rx`] / [`signal_rx_from`] / [`watch_capabilities`]                 | Push channel; `signal_rx_from` restricts delivery to trusted senders.                     |
//! | `CFP` (call-for-proposals) | [`advertise_capability`] + [`declare_requirement`]                          | Providers advertise; consumers declare needs. Emergent groups form without a coordinator.  |
//!
//! [`emit`]: GossipAgent::emit
//! [`emit_ordered`]: GossipAgent::emit_ordered
//! [`rpc_call`]: GossipAgent::rpc_call
//! [`rpc_respond`]: GossipAgent::rpc_respond
//! [`resolve`]: GossipAgent::resolve
//! [`watch_capabilities`]: GossipAgent::watch_capabilities
//! [`group_propose`]: GossipAgent::group_propose
//! [`system_propose`]: GossipAgent::system_propose
//! [`cross_group_propose`]: GossipAgent::cross_group_propose
//! [`signal_rx`]: GossipAgent::signal_rx
//! [`signal_rx_from`]: GossipAgent::signal_rx_from
//! [`advertise_capability`]: GossipAgent::advertise_capability
//! [`declare_requirement`]: GossipAgent::declare_requirement

#![deny(unsafe_code)]
#![warn(clippy::clone_on_ref_ptr)]

pub mod capability;
pub mod capability_config;
pub mod mesh_manifest;
/// Federation identity and policy objects (v3 item 2 — `docs/design/federated-domains.md`).
/// The contract (types, canonical signing bytes) and, under `tls`, the transport's first arm (`edge`,
/// `client`, item 2 PR 8); no `federation/` KV prefix.
pub mod federation;
/// The scoped-mandate contract (v3 item 5 — `docs/design/scoped-mandates.md`).
/// The contract and the epoch check; the enforcement point inside a resource is a later PR.
pub mod mandate;
/// The adaptive-stability contract (v3 item 4 — `docs/design/adaptive-stability.md`).
/// Action classes, the confidence predicate, profiles, `ControlSpec`, spacing and settling —
/// pure decisions only; no governor is changed and the rights ledger is a later PR.
pub mod control;
/// Typed knowledge records (v3 item 3 — `docs/design/knowledge-layer.md`).
/// Records and links only: no store, no resolution, no gossip.
#[cfg(feature = "tls")]
pub mod knowledge;

// Layers I+II substrate live in the `mycelium-core` crate (ROADMAP §v2.0 M1, complete).
// Re-exported here so existing `crate::store::…`, `crate::signal::…`, `crate::config::…`,
// `crate::CoreCtx`, etc. keep resolving unchanged across the crate boundary, and the
// public `mycelium::{config, signal, error}` API surface is preserved.
// `hlc` is public because companion crates hand out packed HLC timestamps (`TraceEvent.hlc`,
// log keys) and need `hlc::physical_ms` to read them without copying the bit layout — it is
// already `pub mod hlc` in `mycelium-core`, so this commits to nothing that crate does not.
pub use mycelium_core::{config, error, hlc, signal};
/// The replay seams' install point, re-exported under `sim` so a **companion** on the public API
/// can record and replay its own runs under the kernel (item 6; first used by CN2).
#[cfg(feature = "sim")]
pub use mycelium_core::sim_seam;
pub(crate) use mycelium_core::{
    connection, framing, locality, node_id, persistence, seen, store, stream, tls, writer,
};

mod agent;
pub mod schema_evolution;
#[cfg(feature = "consensus")]
mod consensus;

pub use agent::{
    AgentPolicy, ExecutionState, AgentStateMachine, PolicyViolation,
    BulkError, BulkServeHandle,
    GossipAgent, MailboxHandle, McpError, McpToolHandle, McpHandle,
    MeshEvent, RpcError, RpcRequest, RpcRequestRx, ScatterError, ScatterResult, SystemStats,
    CallerAttestation, CallerError, GatewayCaller, RequestPrincipal,
    CALLER_CONTEXT_VERSION, PRINCIPAL_ANONYMOUS,
    federation_principal, legacy_token_principal, named_token_principal, node_principal, oidc_principal, positional_token_principal,
    AckResult, CapabilitiesHandle, LogEntry,
    KvHandle, KvQuorumExt, MeshHandle, QuorumError, ServiceHandle, ShardError,
    SchemaError, SchemaHandle, SchemaPublishResult,
};
// Layer III consensus + the consistency overlay built on it (v2 M2 feature gate).
// The AE evaluator seam (runtime authorisation at the gateway) — present where it is enforced.
#[cfg(all(feature = "gateway", feature = "tls"))]
pub use agent::{
    ActionEnvelope, ActionEvaluator, ActionMapping, AeEvidence, AeReference, Decision, DecisionKind,
    EvidenceState, Execution, MandateBinding, MandateState, MappingKind, MappingStatus,
    PreflightRefusal, RecordKind, ReferenceEvaluator, Rule, Verdict, AE_EVIDENCE_SCHEMA,
    AE_REFERENCE_SCHEMA,
};
#[cfg(all(feature = "gateway", feature = "tls"))]
pub use agent::evidence_journal::{
    read_evidence_journal, read_evidence_journal_from, Appended, EvidenceCursor, EvidenceJournal,
    EvidenceProfile, JournalEntry, JournalError, JournalPage,
};
#[cfg(feature = "consensus")]
pub use agent::{ConsensusHandle, ConsistencyError, LockGuard, LockService};
// WS-C M9: self-managing-metabolism config tuner + governance.
pub use agent::{accept_all, clamped, reject_all, ConfigPolicy, CONFIG_PREFIX};
pub use agent::{
    GovernIntent, GovernorSnapshot, HotParam, ParamDirective, ParamSnapshot, Ratchet,
    GOVERN_FLEET_KEY, GOVERN_INTENT_TTL_MS,
};
// Elastic group sizing (Track 2a).
pub use agent::{MembershipAction, MembershipIntent, MEMBERSHIP_INTENT_TTL_MS, MEMBERSHIP_PREFIX};
// Legible Emergence — fleet diagnostics as data (localize · explain · diagnose). `localize`
// (`fleet_snapshot`) and `diagnose` (`fleet_diagnosis`) are node-local reads exposed here;
// `explain` is intentionally gateway-only (`GET /gateway/explain`) — it is a cross-node `sys.explain`
// RPC fan-out, not a local read, so it has no in-process accessor by design. `GroupStatus`
// stays crate-internal (bare name is the mesh type); reach it via `FleetSnapshot.governed_groups`.
pub use agent::{
    FleetDiagnosis, FleetSnapshot, Finding, Severity, StoreConvergence, ThrottleEdge, ViewConfidence,
};
#[cfg(feature = "gateway")]
pub use agent::McpClientHandle;
#[cfg(feature = "llm")]
pub use agent::{PromptTemplate, PromptSkillError, PromptSkillHandle, LlmBackend, LlmResult, LlmError, OpenAiBackend, EchoBackend, LlmHandle};
#[cfg(feature = "compliance")]
pub use agent::{role_key, RoleClaim, SignedRoleClaim, ROLE_PREFIX};
#[cfg(feature = "compliance")]
pub use agent::{
    audit_key, audit_stream_prefix, verify_chain, verify_chain_keys, verify_stream_from_genesis,
    AuditAction, AuditOutcome, AuditRecord, AuditSink, AuditVerifyError, SignedAuditRecord,
    AUDIT_PREFIX,
};
#[cfg(feature = "compliance")]
pub use agent::{
    checkpoint_key, leaf_hash, merkle_root, verify_inclusion, AuditCheckpoint, ProofStep,
    RevocationEvent, SignedAuditCheckpoint, SignedRevocation, AUDIT_CHECKPOINT_PREFIX,
    REVOCATION_PREFIX,
};
#[cfg(feature = "compliance")]
pub use agent::OidcConfig;
pub use capability::{
    CallerContext, CapConstraint, CapEntry, CapFilter, CapRanking, CapValue, Capability, CapabilityEvent,
    CapabilityGroupDef, CapabilityGroupHandle, CapabilityReg,
    DemandStatus, RankingOrder, RequirementHandle, RequirementStatus,
    WiredEmitOutcome, WiringProvider, WiringStatus,
};
pub use capability_config::{
    CapabilityProbeEntry, NodeCapabilityConfig, ProbeEvent, ProbeState, TomlCapValue,
};
#[cfg(feature = "gateway")]
pub use capability_config::run_capability_probes;
pub use mesh_manifest::{
    GroupManifest, GroupStatus, MeshManifest, MeshMeta, MeshStatus,
    manifest_keys, semver_gt,
};
pub use config::{DomainProfile, EgressPolicy, GatewayCallerProfile, GatewayNamedToken, GatewayToken, GatewayTlsConfig, GossipConfig, GroupTopologyPolicy, PersistenceConfig, SyncMode, TlsConfig, TopologyEnforcement};
pub use persistence::DataAtRestCipher;
pub use locality::LocalityPreference;
#[cfg(feature = "consensus")]
pub use consensus::{ConsensusConfig, ConsensusListenerHandle, ConsensusResult, GroupQuorum, consensus_kind, consensus_ns};
pub use mycelium_core::error::GossipError;
// The contracts axis' receipt vocabulary (item 1 PR 2) — what an acknowledgement proves, by rung.
pub use mycelium_core::receipt::{
    content_hash, AttemptId, CommitError, CommitReceipt, DedupOutcome, DestinationCommit,
    LocalApplication, LocalDurability, OperationId, PreparedWrite, ReceiptError, ReplicaSync,
    WriteReceipt,
};
/// Crypto-shredding helper for GDPR right-to-erasure (WS-F) — see [`mycelium_core::erasure`].
#[cfg(feature = "tls")]
pub use mycelium_core::erasure::SubjectKeyRegistry;
pub use node_id::NodeId;
pub use signal::{
    AdvertiseHandle, OpacityHandle, OpacityHint, OpacityState,
    Signal, SignalScope, WatchHandle, signal_kind, kv_ns,
};

/// Re-exports for the cargo-fuzz harness under `fuzz/`. Gated by the
/// `fuzz-internals` cargo feature so normal builds do not widen the
/// public API. The functions here wrap internal `pub(crate)` decoders
/// (`WireMessage`, `Capability::decode`, …) into `&[u8] -> _` calls that
/// fuzz targets can hammer directly.
///
/// **Not stable.** Any item here can move or change shape between
/// patch releases; if you depend on these from outside `fuzz/`, expect
/// breakage.
#[cfg(feature = "fuzz-internals")]
pub mod fuzz_internals {
    use bytes::Bytes;

    /// Attempts to decode `data` as a `WireMessage` using the live hand-rolled
    /// codec (`mycelium_core::codec::decode_wire`). Returns whether decoding
    /// succeeded; the actual message is discarded.
    pub fn wire_message_decode(data: &[u8]) -> bool {
        mycelium_core::codec::decode_wire(data).is_ok()
    }

    pub fn capability_decode(bytes: &[u8]) -> bool {
        crate::Capability::decode(bytes).is_some()
    }
    pub fn cap_filter_decode(bytes: &[u8]) -> bool {
        crate::CapFilter::decode(bytes).is_some()
    }
    pub fn capability_group_def_decode(bytes: &[u8]) -> bool {
        crate::CapabilityGroupDef::decode(bytes).is_some()
    }
    pub fn locality_path_decode(bytes: &[u8]) -> bool {
        crate::locality::LocalityPath::decode(bytes).is_some()
    }
    pub fn load_state_decode(bytes: &[u8]) -> bool {
        let b = Bytes::copy_from_slice(bytes);
        crate::signal::decode_load_state(&b).is_some()
    }

    /// Decode→process: decode `bytes` as a `CapEntry` and, on success, drive the decoded value through
    /// `is_fresh` at extreme clock inputs. Reaches the value-processing arithmetic (`3 × interval`) the
    /// decode-only targets miss — the peer-supplied-arithmetic family (audit 2026-07-15).
    pub fn cap_entry_is_fresh(bytes: &[u8]) -> bool {
        match crate::CapEntry::decode(bytes) {
            Some(e) => e.is_fresh(0, u64::MAX) | e.is_fresh(u64::MAX, 0),
            None => false,
        }
    }

    /// Decode→process for a full wire frame: decode `bytes`, and for a `Data` frame drive the decoded
    /// update end-to-end through `hlc.observe → apply_and_notify → hlc.tick` with the drift bound
    /// disabled (the HLC-poison-prone config). Exercises the peer-supplied-arithmetic + LWW +
    /// secondary-index + live-count surfaces the decode-only targets never reach (audit 2026-07-15 sweep).
    pub fn wire_frame_apply(bytes: &[u8]) -> bool {
        match mycelium_core::codec::decode_wire(bytes) {
            Ok(mycelium_core::framing::WireMessage::Data(upd)) => {
                let kv  = mycelium_core::store::KvState::new(0);
                let hlc = mycelium_core::hlc::Hlc::with_max_drift(0);
                hlc.observe(upd.timestamp);
                mycelium_core::store::apply_and_notify(&kv, &upd);
                let _ = hlc.tick();
                true
            }
            _ => false,
        }
    }

    // ── Trust-edge parsers (§12.6) ────────────────────────────────────────────
    //
    // A parser sits on a **trust edge** when it reads bytes a partner or a client controls, before
    // anything about them has been verified. The axis added four such parsers, and the Phase-C
    // adversarial audit reached three of its four defects through exactly this kind of input — so
    // these join the input-fuzz gate rather than a list of things that ought to be fuzzed.
    //
    // Each entry point asserts the **invariant the parser is relied on for**, not merely that it
    // does not panic. A parser that returns a wrong-but-well-formed answer is the failure that
    // matters here; a crash is the easy case.

    /// The caller-context frame: the first thing read off an RPC payload, before any signature
    /// check, on bytes a gateway client may control.
    ///
    /// The invariant is **byte conservation**. Whatever the classification, every input byte is
    /// accounted for exactly once and none is invented: a client must not be able to shape a
    /// payload so that bytes it wrote as envelope reappear as application input, or the reverse.
    /// That is the smuggling property the raw-emission guard depends on.
    pub fn caller_frame_classify(data: &[u8]) -> bool {
        use crate::agent::gateway_caller::{split_frame, Frame, MAX_ENVELOPE_BYTES};
        let bytes = bytes::Bytes::copy_from_slice(data);
        match split_frame(&bytes) {
            Frame::Framed { envelope, app } => {
                // 5-byte magic + 2-byte length, then the envelope, then the rest.
                const HDR: usize = 7;
                assert!(envelope.len() <= MAX_ENVELOPE_BYTES, "envelope exceeded its own bound");
                assert_eq!(
                    HDR + envelope.len() + app.len(),
                    data.len(),
                    "framing lost or invented bytes"
                );
                true
            }
            Frame::Unframed(rest) => {
                assert_eq!(rest.as_ref(), data, "an unframed payload was altered in passing");
                false
            }
            Frame::Malformed(_) => false,
        }
    }

    /// The caller-context **envelope**, one layer past the frame: the JSON, the `via` node id and
    /// the two base64 credential fields, all read before the signature is checked.
    pub fn caller_envelope_decode(data: &[u8]) -> bool {
        crate::agent::gateway_caller::fuzz_envelope_decode(&bytes::Bytes::copy_from_slice(data))
    }

    /// A presented federation credential's wire form — one header value on `/a2a`, entirely
    /// partner-controlled and parsed *before* the credential is authenticated.
    ///
    /// The invariant is **parse stability**: the verifier rebuilds the canonical signing bytes from
    /// the parsed fields, so a value that parses one way and re-renders another would verify a
    /// different credential from the one presented.
    #[cfg(feature = "tls")]
    pub fn presented_call_parse(data: &[u8]) -> bool {
        use crate::federation::edge::PresentedCall;
        let Ok(text) = std::str::from_utf8(data) else { return false };
        match PresentedCall::from_header_value(Some(text)) {
            Ok(Some(call)) => {
                let again = PresentedCall::from_header_value(Some(&call.to_header_value()));
                assert_eq!(Ok(Some(call)), again, "a credential did not survive its own round trip");
                true
            }
            // `None` is unreachable with `Some(_)` in, and a malformed header is the expected
            // outcome for most inputs — what must not happen is it being read as *absent*.
            Ok(None) => unreachable!("a present header parsed as absent"),
            Err(_) => false,
        }
    }

    /// A signed catalogue reply, as a client parses it off the wire before verifying it. Same
    /// stability invariant, and it bites harder here: the signature covers bytes derived from the
    /// parsed fields.
    #[cfg(feature = "tls")]
    pub fn catalog_reply_parse(data: &[u8]) -> bool {
        use crate::federation::edge::CatalogReply;
        let Ok(text) = std::str::from_utf8(data) else { return false };
        match serde_json::from_str::<CatalogReply>(text) {
            Ok(reply) => {
                let rendered = serde_json::to_string(&reply).expect("a parsed reply must re-render");
                let again: CatalogReply =
                    serde_json::from_str(&rendered).expect("a rendered reply must re-parse");
                assert_eq!(reply, again, "a catalogue reply did not survive its own round trip");
                true
            }
            Err(_) => false,
        }
    }

    /// The hand-rolled fixed-int codec, on the two objects that reach it from outside the process:
    /// a `LedgerEvent` read back from the rights ledger on disk, and a `PublishedRightsHead` read
    /// from `rights/head/{holder}` in the gossip medium.
    ///
    /// §12.6's sweep flagged this as an unfuzzed decoder on a live path, and a 20k-case mutation
    /// sweep found **no panic** — `serde_fixint` checks its remaining length before every read, so
    /// the unchecked subtraction in `remaining()` is not reachable. This target exists because that
    /// sweep is far weaker than coverage-guided fuzzing, and because a hand-rolled binary decoder
    /// reading bytes off disk is precisely the shape behind the unbounded-allocation decode DoS
    /// that sat uncaught through M2 Run-20.
    ///
    /// The invariant is round-trip stability rather than byte equality: `from_slice` deliberately
    /// tolerates trailing bytes, so re-encoding need not reproduce the input.
    pub fn fixint_decode(data: &[u8]) -> bool {
        use crate::control::ledger::{LedgerEvent, PublishedRightsHead};
        use mycelium_core::serde_fixint;

        let mut decoded = false;
        if let Ok(event) = serde_fixint::from_slice::<LedgerEvent>(data) {
            let re = serde_fixint::to_vec(&event).expect("a decoded event must re-encode");
            let again = serde_fixint::from_slice::<LedgerEvent>(&re)
                .expect("a re-encoded event must decode");
            assert_eq!(event, again, "a ledger event did not survive its own round trip");
            decoded = true;
        }
        if let Ok(head) = serde_fixint::from_slice::<PublishedRightsHead>(data) {
            let re = serde_fixint::to_vec(&head).expect("a decoded head must re-encode");
            let again = serde_fixint::from_slice::<PublishedRightsHead>(&re)
                .expect("a re-encoded head must decode");
            assert_eq!(head, again, "a rights head did not survive its own round trip");
            decoded = true;
        }
        decoded
    }

    /// A trust bundle, as an operator's configuration is read into the process.
    ///
    /// §12.6 names trust bundles as a trust-edge parser and they are the weakest edge of the four:
    /// a bundle is **bilateral operator configuration**, with no registry and deliberately no way
    /// for a third party to add an entry. It is fuzzed anyway, because it is the object that decides
    /// which keys are acceptable at all — a misparse here is not a refused call, it is the wrong
    /// answer to *who do we trust*. Every partner id it carries must satisfy `DomainId::new`.
    pub fn trust_bundle_parse(data: &[u8]) -> bool {
        use crate::federation::TrustBundle;
        let Ok(text) = std::str::from_utf8(data) else { return false };
        match serde_json::from_str::<TrustBundle>(text) {
            Ok(bundle) => {
                for p in &bundle.partners {
                    assert!(
                        crate::federation::DomainId::new(p.domain.as_str()).is_ok(),
                        "a bundle parsed a partner id no constructor would make",
                    );
                }
                let rendered = serde_json::to_string(&bundle).expect("a parsed bundle must re-render");
                let again: TrustBundle =
                    serde_json::from_str(&rendered).expect("a rendered bundle must re-parse");
                assert_eq!(bundle, again, "a trust bundle did not survive its own round trip");
                true
            }
            Err(_) => false,
        }
    }

    /// The two signed objects an operator ingests from a partner: a domain descriptor and a domain
    /// policy. Both are parsed before their signatures are checked, and both carry a field a later
    /// decision is keyed on — `exports` and `revision`.
    pub fn federation_objects_parse(data: &[u8]) -> bool {
        use crate::federation::{DomainDescriptor, DomainPolicy};
        let Ok(text) = std::str::from_utf8(data) else { return false };
        // A parsed id must satisfy its own constructor. Deleting `DomainId`'s validating
        // `Deserialize` fails this within seconds of fuzzing, which is how the gap was found.
        fn well_formed(id: &crate::federation::DomainId) -> bool {
            crate::federation::DomainId::new(id.as_str()).is_ok()
        }

        let mut parsed = false;
        if let Ok(d) = serde_json::from_str::<DomainDescriptor>(text) {
            assert!(well_formed(&d.domain), "a descriptor parsed an id no constructor would make");
            let rendered = serde_json::to_string(&d).expect("a parsed descriptor must re-render");
            let again: DomainDescriptor =
                serde_json::from_str(&rendered).expect("a rendered descriptor must re-parse");
            assert_eq!(d, again, "a descriptor did not survive its own round trip");
            parsed = true;
        }
        if let Ok(p) = serde_json::from_str::<DomainPolicy>(text) {
            assert!(well_formed(&p.domain), "a policy parsed an id no constructor would make");
            for (partner, _) in &p.grants {
                assert!(well_formed(partner), "a grant parsed an id no constructor would make");
            }
            let rendered = serde_json::to_string(&p).expect("a parsed policy must re-render");
            let again: DomainPolicy =
                serde_json::from_str(&rendered).expect("a rendered policy must re-parse");
            assert_eq!(p, again, "a policy did not survive its own round trip");
            parsed = true;
        }
        parsed
    }
}

/// test-only: a bind-verified, process-unique loopback port allocator for companion
/// integration tests — retires the `free_port` TOCTOU flake class.
#[cfg(feature = "test-util")]
pub use test_util::alloc_port;

// ─── Tests ────────────────────────────────────────────────────────────────────


// Public under `test-util` (feature-gated, test-only) so companion integration tests can call
// `mycelium::test_util::alloc_port()`; a plain module for the core's own `#[cfg(test)]` build.
#[cfg(any(test, feature = "test-util"))]
pub mod test_util;

#[cfg(test)]
mod lib_tests;

#[cfg(test)]
mod swim_oracle_tests;
