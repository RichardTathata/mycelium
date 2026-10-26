# dev/operations — the diagnostic surface

↑ [dev/](dev.md) · operator runbooks: `docs/operations/` (deployment, observability, tuning, dynamic-scaling…). Before a go-live, the [`production-readiness.md`](../../operations/production-readiness.md) pre-flight ties the topic runbooks into one sweep; for a first customer engagement, [`customer-pilot.md`](../../operations/customer-pilot.md) scopes it as external validation.

## Node-level HTTP endpoints (`gateway` feature)

The four probes are **public by design** (M16 edge criterion — what a balancer or scraper needs with no credential); `/consensus/{*slot}` sits behind the gateway bearer when a token model is set (2026-09-05, with `/mcp` and `/signals/{kind}` — [security](security.md) §3). The whole public surface is the probes, `/bulk/{id}` (nonce-capability URL) and the A2A descriptor — `docs/operations/rbac.md`. **Gateway caller identity (item 7, 2026-09-13):** every gateway-originated dispatch carries an auth-layer `GatewayCaller` the provider verifies; `gateway_caller_profile` (`secure` default / `legacy` for a rolling-upgrade window), the `sys/caller-context/{node}` marker, and the refusal metric `mycelium_gateway_caller_refusals_total{reason}` — runbook `docs/operations/rbac.md` §7, mechanism [security](security.md) §WS1.5.

| Endpoint | Tells you |
|---|---|
| `GET /health` | process alive |
| `GET /ready` | startup complete → serving KV/signals/membership (capability discovery gossips independently and does **not** gate readiness — a node advertising no soft state is still ready; changed 2026-07-15) |
| `GET /stats` | `node_id`, optional `cluster_name` (`GOSSIP_CLUSTER_NAME` — also a `cluster` global label on `/metrics` and an AgentFacts field), `store_entries`, `dropped_frames`, `task_count`, the tripwire counters (`commit_conflicts`, `sys_namespace_violations`, `cap_authz_violations`, `schema_mismatch`, `rate_limited_senders`, `individual_flood_fallbacks` — the flood-fallback/topology-pressure signal, remedy `connect_peer` — and `identity_anchor_conflicts` — a `sys/identity` KV key differing from a directly-connected peer's CA-anchored mTLS key; identity-poisoning signal, WS-E Phase 1b), the liveness fields (`dead_shards`, `gc_alive`, `health_monitor_alive`), the emergent-detector gauges `governed_group_conflicts` (P1) and `capability_coverage_gaps` (P6), `membership_flaps` (P2), `opacity_oscillations` (P3 — (node,kind) pairs hunting in/out of shed), `role_concentration_pct` (P10 — one node's **percentage** share of the fleet's live single-writer roles; `0` also means *too few roles or holders to say*, incl. a partitioned view) (Legible-Emergence Phase 1; `0` unless `GOSSIP_EMERGENT_DETECTORS`), and — when detectors are enabled — `opaque_node_pct` (P4 fleet-opacity-storm gauge, 0–100; a raw gauge the operator thresholds) plus a `view_confidence` object (RT1/RT2: a per-node *estimate*, not fleet truth; `peers_heard`/`peers_known`, `max_staleness_ms`, `self_degraded`) |
| `GET /consensus/{*slot}` | committed value (lease-aware) + ballot + lease state; the slot is the **path tail** (hierarchical slots such as `lock/{name}` typed as-is, 2026-09-06) — **bearer-gated** when a token model is set (`consensus:read`, 2026-09-05) |
| `GET /metrics` | Prometheus (`metrics` feature). Includes the emergent-detector gauges when `GOSSIP_EMERGENT_DETECTORS` is on: `mycelium_emergent_governed_group_conflicts` (P1), `mycelium_emergent_capability_coverage_gaps` (P6), `mycelium_emergent_membership_flaps` (P2), `mycelium_emergent_opacity_oscillations` (P3), `mycelium_emergent_opaque_node_pct` (P4), `mycelium_emergent_role_concentration_pct` (P10), and the RT1/RT2 view-health gauges `mycelium_emergent_peers_heard` / `_peers_known` / `_max_staleness_ms` (alert-qualify a diagnostic by the observer's own view: `peers_heard` ≪ `peers_known` ⇒ partial view). Plus the **consensus/lock** family (needs `consensus`): `mycelium_consensus_timeouts_total{reason}` (event-emitted at every no-quorum exit — `no_voters`=nobody else answered (partition, or mid-upgrade all older) / `promise_short`=too few promised / `contended` / `blocked` / `quorum_short`=overload / `all_opaque` / `empty_groups`; the one series *not* gated on the detector loop) and `mycelium_consensus_commit_conflicts` / `mycelium_schema_mismatch` (gauges mirroring the `/stats` scalars, set on the detector tick). Locks are consensus slots → no per-lock gauge; inspect one via `GET /consensus/lock/{name}` |

Diagnostics surface (Legible Emergence, all `fleet:read`) — the three-verb operator spine, each
computed locally from this node's gossiped KV (coordinator-free: any node answers; at convergence
the *diagnosis* agrees across nodes, while each keeps its own `view_confidence`):

- **localize** — `GET /gateway/fleet`: the relational snapshot (governed-group status, coverage
  gaps, opacity, throttle graph, cross-node store-convergence, commit-conflict hot slots + the
  RT1/RT2 `view_confidence` header).
- **explain** — `GET /gateway/explain?since=`: the cross-node HLC-ordered event narrative,
  best-effort fan-out (**capped** at `EXPLAIN_MAX_FANOUT` peers so an operator query never becomes an
  O(N) RPC storm) that *names* both the peers that did not answer (`non_responders`) and the count
  skipped by the cap (`not_queried`) rather than silently dropping them (RT3).
- **diagnose** — `GET /gateway/diagnose`: the "why is the fleet in this state" rule engine — a
  most-severe-first list of findings, each naming a pathology in actionable terms, with an RT1/RT2
  `caveat` when the observer's own view is partial.
- **guarantees** — `GET /gateway/guarantees` (`fleet:read`): the node's **guarantee report** (plan
  `guarantees-and-rule-catalogue.md` I2, `src/agent/guarantee.rs`; its golden is `docs/reference/guarantee-catalogue.md` (descriptors + a state matrix, gated by `the_checked_in_guarantee_catalogue_is_current`); its sibling is the **rule catalogue**, I1 — `mycelium_core::rule`, entries in `src/rules.rs` and `mycelium-wasm-host/src/rules.rs`, generated to `docs/reference/rule-catalogue.md` and gated by `mycelium-wasm-host/tests/rule_catalogue.rs`) — every registered guarantee resolved
  against this node *as built and configured*: `enforced` · `not_configured` (the setting or attachment
  named) · `not_in_build` (the feature named) · `not_applicable` (the role fact named — a node with no
  `http_port` does not fail a gateway guarantee) · `not_verifiable_here` (external prerequisites: network
  confinement, clock sync — listed, never counted; the consensus profile was one until P2 made it the node-enforced
  `cons.safety_profile`). The same report is logged as
  one block at `start()`, after validation and before traffic (the G13 boundary); an attachment made
  after that is counted (`late_attachments`) and warned about. The strongest sentence it says is *node
  requirements satisfied*, never *deployment verified*. `ConfinementReport` is now a view over it.
  **Profiles (I3):** `GossipConfig::profile` / `GOSSIP_PROFILE` selects `dev` (nothing required, said
  loudly) or `secure-single-domain` (rev 5, post-360 row F2 — twenty ids pinned by `the_secure_profiles_required_set_is_pinned`; rev 5 added `mesh.signed_writes`, met wherever `mesh.tls` is; rev 4 (2.32.0) added `cons.safety_profile` (`consensus_require_electorate`, plan P2), rev 3 `gw.exposed_closed` (plan P1); rev 2 (v2.21.0) added `id.ca_key_off_node` and `persist.unreadable_refused`, announced in v2.20.0 per G12);
  under a profile `start()` refuses by name when a required guarantee is not `enforced` or
  `not_applicable` (`guarantee::check`), an unknown required id refuses, and an unknown name fails
  `validate()`. The report carries the profile, its revision and its required set.

All three are also programmatic (**diagnostics as data**): `agent.fleet_snapshot()` /
`fleet_diagnosis()`. Operator runbook (one entry per pathology + Prometheus alert recipes):
[operations/diagnostics.md](../../operations/diagnostics.md).

Governance surface: `POST /gateway/govern/{timing,tuning,membership,profile,topology-override,group}` (and `DELETE …/group`) +
`GET /gateway/govern` (deny-by-default scopes, `govern:read`/`govern:write`; each write is sealed into the audit
trail in a `compliance` build with `[tls]`, and one that is not is counted on `/stats` (`governance_unaudited`) —
`profile` recorded nothing and accepted a `target` until #555; `topology-override` sets or releases `sys/topology-override/{group}`,
which the raw KV routes refuse — 2.27.0, #549) — see
[management-as-intent](../domain/theory/management-as-intent.md) for the model.

**Identity / key revocation (SOC 2 WS-B, `compliance`):** `POST /gateway/identity/revoke` (scope
`identity:write`), body `{"revoked_key":"<64 hex>","reason":"…"}` — the operator compromise-
remediation trigger. Revocations are validated + excluded on all verify paths incl. consensus
(`src/agent/revocation.rs`); rotate then revoke, or `rotate_identity_on_compromise`. Runbook:
[cert-rotation](../../operations/cert-rotation.md).

**Native gateway TLS (WS-A):** the gateway is plaintext by default (the "no auth by design" edge
above is the *default*, not a ceiling — set `gateway_scoped_tokens`/OIDC for auth). Set
`GossipConfig::gateway_tls` (`GatewayTlsConfig`) for native server-side HTTPS so bearer tokens/JWTs
are never cleartext — reuse the node cert or supply a hostname cert.
[gateway-tls](../../operations/gateway-tls.md).

**Authenticated identity (WS-E):** every TLS node publishes a signed `sys/identity-proof/{self}`;
peers reject an identity overwrite whose proof doesn't chain to a trusted key (`identity_anchor_conflicts`
counts rejections, on `/stats`). `require_identity_proofs` (`GOSSIP_REQUIRE_IDENTITY_PROOFS`, a strict boolean, default **on** since
post-360 row S) also rejects entries it cannot authenticate: only the sealed record (one entry since
2.14.0) is merged, so the two-write window that kept it off is closed by construction. The default
newly refuses only a pre-2.3.0 node (no proof); `false` is the explicit opt-out ([security](security.md)
§identity-proof default, [cert-rotation](../../operations/cert-rotation.md), `deprecations.md` §41).

**Bridged capability adverts need a lease.** `POST /gateway/capability/advertise` spawns the
refresh loop *in the node*, so a bridge client's advert survives the client's crash and never
evaporates (the 2026-07-20 scraper-fleet "stale w15 advert" finding: provider liveness decoupled
from refresher liveness). Pass `lease_secs` and beat `POST /gateway/capability/{handle_id}/heartbeat`
within every window (beat at `lease_secs/3`); a missed window retracts exactly as DELETE would.
Omitting `lease_secs` keeps the old durable-until-DELETE semantics — correct only when the
advertised capability's lifetime really is the node's. Canon: `src/agent/http/caps.rs`
(`gw_cap_advertise`); both SDK bridges expose it (`leaseSecs` + `handle.heartbeat()` in
`mycelium-ts`, `lease_secs` + `handle.heartbeat()` in `mycelium-py`).

**An SDK agent's unit file goes through the same lease (Q2, 2026-10-01).** `POST /gateway/units/declare`
(`cap:write`) takes a unit file's *text*: the node parses it with the same `NodeCapabilityConfig` a
stem reads, refuses by name (422) what an SDK agent cannot hold — `[hosts]`, `[[presence]]`,
`[[activation]]`, `[[serve]]` — and holds the rest under one handle with the lease above
(`declare_from` in `mycelium-py`, `declareUnits` in `mycelium-ts`). **Publishing an artifact is a
separate door with its own scope:** `POST /gateway/artifacts/publish` (`artifact:publish`, served by
`mycelium-wasm-host`'s `artifact_router`, feature `gateway`, plan A3) adds a signed entry to the
library's manifest; the operator's walk is [artifacts](../../operations/artifacts.md) and the scope
row is in [rbac](../../operations/rbac.md).

## task_count reference (leak triage)

Steady state after `start()`: 7 core loops (GC, health, anti-entropy, WAL-flush, reorder
buffer, capability heartbeat, group-member sync) + 2 per gossip shard (default 4) + 1
gateway + N per-peer writers + N `bulk_serve` listeners. Typical 3-node baseline **17–20**.
Not tracked: `rpc_call` (no task), `scatter_gather` (local JoinSet), per-request bulk
handlers (semaphore-bounded, visible as `active_bulk_handlers`). Unbounded growth ⇒ suspect
a per-peer writer not exiting on disconnect.

## Stopping a node (`mycelium::shutdown`, 2.31.0)

The node binary, `mycelium-stem` and the long-running examples stop through one handler,
`ShutdownSignal` (`src/shutdown.rs` — its module doc is canon): **SIGINT or SIGTERM** (`docker stop`, a pod stop),
**installed when constructed** — before the bind, awaited after, because a handler registered only when the wait
begins leaves startup to the default action, which kills — and a **second signal exits at once** with `128 +
signal` (130 / 143). That forced exit is SIGKILL-equivalent: no destructors, unsynced WAL records lost, on-disk
state repaired at the next start. **Do not create one in an interactive program** — a handler nobody awaits
swallows Ctrl-C; the node binary skips it under `-i`. Gates: `tests/shutdown_signal.rs`,
`mycelium-wasm-host/tests/stem_shutdown.rs`.

## Feature gates

`cli` (default; tracing-subscriber for binaries) · `gateway` (default; disable for embedded: `default-features = false` — gossip, KV, signals,
consensus, typed handles all remain) · `consensus` (default; drop for minimal embeds — a
consensus-free node still *forwards* PROPOSE/VOTE/COMMIT) · `tls` · `metrics` · `a2a` ·
`llm` · `otel` (OTEL span export from `SkillRunner`) · `compliance` (= gateway+tls).
`mycelium-core` builds standalone (≈48 deps vs ≈140). (`fuzz-internals` / `test-util` are
test-only, and `sim` — the replay seams, `mycelium_core::sim_seam` routed through `mycelium-sim` — is for recording
and replay, never a shipped build; with `test-frame-filter` (the frame filter the lock-lifecycle tests drop frames through; CI's lib runs enable it), all four deliberately not listed above.)

## The published node (adopter-surface X0, 2026-10-23)

Every `v*` tag publishes the three binaries for Linux x86_64, Linux aarch64 and macOS aarch64 with `SHA256SUMS`, and
the multi-arch image `ghcr.io/richardtathata/mycelium` (`.github/workflows/release.yml`; the image is
`docker/Dockerfile.node` — `docker/Dockerfile` is the demo). The published node is the full feature set
(`cli,tls,compliance,a2a,llm,metrics`) because a configuration naming a table the build lacks refuses to start; the
stem is `stem,gateway,llm,object_store`. Linux binaries are bookworm-built (glibc 2.36, `libssl3`). The workflow refuses
a tag whose version differs from `Cargo.toml`'s, smokes each Linux tarball in a clean container (`mycelium --version`
equal to the tag, `wire-check` over the fixtures), and creates the Release from the annotated tag's message if
RELEASING § 8 has not. The sums are integrity, not provenance: nothing signs a binary (`what-is-proven.md`, third
table). Entry page: `docs/features/published-node.md`; log `.log/2026-10-23-published-node.md`.

## `mycelium declare` (adopter-surface X10, 2026-10-23)

`src/declare.rs` (feature `gateway`): one `tick` fingerprints `units/*.toml` and `skills/<ns>/<name>.md` (mtime, size),
runs `wire_check::check` over the units (a refusal keeps the previous declaration), then per unit `DELETE
/gateway/capability/{old handle}` and `POST /gateway/units/declare` (`interval_secs`, `lease_secs`; the response's
`not_enforced` collected), retracts units gone from the directory, and `PUT /gateway/prompts/{ns}/{name}` per skill
(a `PromptTemplate` with the file as `user_template`; a missing `{{input}}` is a warning). `heartbeat()` posts per
handle; `run` polls on `tokio::time::sleep`, beats every `lease/3`, retracts on Ctrl-C. A poll, not a seam: admitted in
the replay inventory. Entry page `docs/features/declare.md`; log `.log/2026-10-23-declare.md`.

## The `one-hour` job (adopter-surface § 1.1, 2026-10-23)

`.github/workflows/one-hour.yml`: the only place a number may be put on *one hour to a governed fleet*. Trigger:
`workflow_run` on the Release workflow's completion for a tag push (a Release created with `GITHUB_TOKEN` fires no
`release` event — #670's review), and dispatch. `python:3.12-slim-bookworm`, `shell: bash` (a job container's `sh` is
dash, whose `pipefail` depends on the Debian release), no toolchain, the published artefacts only. Steps 1/2/3/6 timed
from the first `curl`; apt on its own row outside the clock; the SDK's version checked against the tag's
`pyproject.toml`; a red run prints no number; 3 600 s is the bound. Steps 4 and 5 and the records half of 3 are
printed as not yet measured (X3, X4, X9). Log `.log/2026-10-23-one-hour.md`. *(2026-10-25)* Step 5 is in the job
(X9's `recovery` class on the published binary); it measures from v2.38.0, the first release that carries the class.

## `mycelium init` (adopter-surface X2, 2026-10-23)

A scaffold from a template the binary embeds (`src/init.rs`, `include_str!` over `templates/<name>/*`; a test holds the
directory and the list equal), pinned to the binary's release tag, into a new or empty directory only. `governed-call`
(Rust: two nodes sharing one `auto_cert_dir`, so one CA and per-node certificates; a member's direct `mcp.invoke` to
the provider under `with_provider_enforcement`, an `ActionEvaluator` of twenty lines, an `EvidenceJournal`, a
`DecisionSink` written to `data/decisions.jsonl`; a prompt skill on the member node, since an `llm.invoke` under
provider enforcement must name a resource) and `governed-call-py` (the published node with named tokens; `declare_from`,
`set`, a 403 `insufficient_scope`). CI's `Scaffold` job builds the Rust scaffold against the checkout by rewriting its
dependency to a path (a `[patch]` of a git source still fetches the pinned tag, which a version-bump commit lacks —
#666's review). Found: `ReferenceEvaluator` declares no
security-relevant arguments, so a `requires_values` rule cannot see its value through the seam. Entry page
`docs/features/init.md`; log `.log/2026-10-23-init.md`.

## `mycelium doctor` (adopter-surface X1, 2026-10-23)

The readiness report without a node (`src/doctor.rs`): `start()`'s refusal chain and profile check, extracted to
`GossipAgent::preflight()` and `profile_check()` in `lifecycle.rs` and called by `start()` itself, run over a node
built and never started, plus `guarantee_report()` and `confinement_report()`. The effective configuration is the
diff of three serialised layers (defaults, the file parsed without the environment, the environment applied), each
flattened key attributed `env` / `file` / `default`; secrets (`gateway_auth_token`, any `token`) rendered `set`/`unset`.
**Found by it:** `GossipConfig::validate()` probed `persistence.base_path` with `create_dir_all`, so a validator wrote to
disk; now read-only (`persistence_base_path_problem`, `deprecations.md` §57), `start()`'s R7 refusal unchanged. Limits:
an attachment made in code is not in its view; unreadable persisted state is `start()`'s. Entry page
`docs/features/doctor.md`; runbook `operations/diagnostics.md` § Before you start; log `.log/2026-10-23-doctor.md`.

## `mycelium test --faults` (adopter-surface X6, 2026-10-24)

The existing proofs against an adopter's declaration (`src/faults.rs`; the verifier probes that need agent internals
in `src/agent/fault_probes.rs`). `wire-check` first, then one fleet per class — `Fleet`: one node per unit, three at
least, each on a one-worker runtime of its own so a node is *cut* by dropping its runtime (no sweep, no departure,
sockets closed), the configuration rewritten to loopback, free ports, a per-node persistence directory (one owner per
directory, and the revocation journal sits at the base path), TLS material under the run's directory, no `http_port`,
a one-second health check, all-to-all bootstrap (a node back after a cut must find the survivors itself: they evicted
it and stopped dialling). `replay` records each unit's node alone on a current-thread runtime under one kernel, as
`main.rs`'s `record` does; a divergence is the seam's panic, caught and reported, the kernel taken down after. A
fleet recorded together on one kernel diverged at seq 14 — `writer/forward` to one peer recorded, to the other
replayed — because frame arrival between nodes is outside the seams (inventory §3.1.1); stated on the page. `revoke` lifts row Q's three
in-process verifiers from `lib_tests.rs` (the KV arm, the handshake, identity chaining) and adds the mandate half
through `ExecutionAuthority` (a revoked term reads `MandateState::Unknown("the appointment is revoked")`, not a
refusal). `policy-diff` decides the declaration's call set at the command's own enforcement point (`preflight` + an
`AeEvidence` per call into a journal under the run's directory) and re-decides each journal record under the
proposed revision; a `[[rule]]`'s resource is matched as written (no `@provider` suffix). **Found by it, fixed:** a governed member that went dark held
the floor — `grp/{group}/{node}` is written once at join and withdrawn only by the graceful sweep (#627), so after a
crash every survivor's `group_members` still counted it and no one rolled (the co-op fixture's `depots`: 0 live
members, 0 joins after 105 s); `converge` now counts a member only if it is this node or in `ctx.peers`
(`live_members`, `membership.governed` revision 2 — a convergence target, never a guarantee: a peer evicted here and
alive elsewhere is re-counted when heard; with an empty peer table the roster is read as written, #675's review).
The eleven other readers of `grp/` (`emergent.rs`'s `group_status`/`GroupConflict`/`membership_snapshot`, the fleet
snapshot, consensus, signal scoping, `/gateway/mesh/group`) still count the dark member until its entry is rewritten
— stated, not changed. **#675's review** also found: the `faults/` row unclassified for the raw KV routes; the port
allocator sharing `alloc_port`'s seed (a test build now uses `alloc_port` itself); the re-fill baseline sampled after
the evaporation wait; the secure profile refused by the command's own rewrite (not applied now, reported); `unknown`
in the report (a node built and never started resolves every id); the cut dropping handles before the runtime; and,
by `replay`, a persisted node diverging between its first WAL write and the jitter draw — recorded, not fixed. **Found and stated:** the eviction loop removes a dark peer in
`health × peer_eviction_intervals`, but its `cap/` advertisement leaves `resolve` only after `3 × ttl_secs`
(`CapEntry::is_fresh`) and nothing tombstones it — a scaffold's 30 s `ttl_secs` is a 90 s blind spot the report now
measures; the stale `grp/` entry stays in the KV until the node returns, so the fleet snapshot's group size still
counts it. Entry page `docs/features/test-faults.md`; runbook
`operations/diagnostics.md` § Before the fleet carries anything; log `.log/2026-10-24-x6-test-faults.md`.

## The adapter contract and a unit's `runtime` (adopter-surface X7 and X8, 2026-10-24)

`src/adapter_contract.rs`: `ADAPTERS` — eight rows (the native embedding first, then `mycelium-py`, `mycelium-ts`,
`langgraph-checkpoint-mycelium`, `strands-session-mycelium`, `mycelium-reason`, the A2A door, the MCP door), seven
`Cell`s each (`Carried { test, file, note }` · `NotCarried { why }` · `NotApplicable { why }`), the guarantee ids a
door resolves, the limits-section link — rendered to `docs/reference/adapter-contract.md` by
`the_checked_in_adapter_contract_is_current` (`UPDATE_ADAPTER_CONTRACT=1`). The gate `check-feature-pages.py` (j)
reads the page's rows against the Rust `name:` list and feeds every carried cell's test into the `--executed` mode,
which now matches `python <file>::<test>[param]` and `typescript <file>::<title>` keys beside `rust`.
`check-materials.py` requires `## What this does not establish` in every companion README (`companion_readme`:
`mycelium-*/README.md` and `*-mycelium/README.md`). X8: `NodeCapabilityConfig::runtime`, read by `wire-check` into
`UnitSummary::{runtime, adapter, carries, not_carried}` and the finding `unknown runtime`; the gateway ignores it.
**Cells worth knowing:** the checkpointer discards the receipt `/gateway/kv` returns (`raise_for_status()` only);
the Strands store's bearer has no principal test; neither SDK cancels or carries a grant; an A2A task is lost with
its node. Entry page `docs/features/adapter-contract.md`; log `.log/2026-10-24-x7-adapter-contract.md`.

## Grants, the fourth PR — the `grant` fault class (adopter-surface X3, 2026-10-25)

`src/faults/grant.rs`, `Class::Grant` (seven classes; `Class::ALL` is seven long). The class reads the declaration: the
first `[[rule]]` with `requires_grant = true` (`RuleDecl::requires_grant`, `to_rule` → `Rule::requiring_grant`; with
`requires_mandate` beside it the rule is refused) names the tool (`tool:<name>`, a provider suffix stripped) and the
agent's unit; the actor of a `grant.issue` rule on that resource or `*` is the issuer. Gated
`cfg(all(gateway, tls, compliance))` — `compliance` because `gateway_member_keys` is empty without it, so the grant's
checkpoint verifies against nothing and the standing stays `Unknown` (the door tests carry the same gate). Needs `[tls]`
and `[persistence]`, each `not_configured` by name; without the rules `not_applicable` naming which. The evaluator is
built as `policy-diff` builds one, plus a copy of every `tool:<name>` rule at `tool:<name>@<agent node>` — the request
record's and the doors' resource form; a declaration should not have to name the node. `start_agent_node` attaches the
evaluator, the journal (reopened at the same path across the restart) and an `ExecutionAuthority` entitled to the
node's own grants; the other nodes start plain. The class issues with `decide_grant` (`src/agent/http/grants.rs`), the
route's decision factored out of `gw_grant_decide` — `revoke_grant` likewise out of `gw_grant_revoke`;
`GrantRefusal::{Door, Policy}`; `write_record` returns `Result<Value, String>` now — re-exported `pub(crate)` from
`agent` under `gateway + tls`. The stranger (`test-faults-stranger`) is refused by the evaluator (`GrantRefusal::Policy`)
unless a `*` actor admits anyone — then the stranger does not issue at all and the report says so (#680's review,
finding 1: the stranger's grant was written and the issuer's refused `already_answered`; a test covers the `*` case).
The grant-required rule is a *selector*: the default-deny shown is the state machine's `require_grant_for`, derived from
the rule's resource; the rule is also admitted with the node's principal as actor (finding 4 — only a node holds a
grant). `RuleDecl::admits_grant_issue_for` is the one reading of "may issue" for `wire-check` and the class (`*` for
operation or resource, `@node` stripped; findings 2 and 3); a contradictory `requires_grant` + `requires_mandate` is
refused by `validate()`, so every reader agrees (finding 5); the journal is attached before the evaluator here and in
`policy-diff` (finding 6, the "decisions NOT recorded" warning). Found writing the `*` test: the reference evaluator's
catalogue mapping is per operation, so a `*` rule maps the literal `*` and `grant.issue` reads *Unmapped* — the class maps
the four operations it drives for a `*` rule. `policy-diff`'s `build` has the same shape (a wired edge's `tools/call` under
only a `*` rule reads Unmapped there) — stated, not changed in this PR. The restart: the waiting `transition_invoking` task is
aborted, the issuer answers, every handle on the node is dropped (the state machine holds the context and with it the
journal), `fleet.stop(i)`, `start_agent_node` again. The timer and clock reads are `sim_seam::mono_instant`, `until`
and `JoinHandle::is_finished` — no `tokio::time::timeout`, which the seam gate would flag. Scaffolds: both
`purchaser.toml`s declare `tool:purchase.approve` under the treasurer; the one-hour job's step 4 adds `[tls]` and
`[persistence]` to the scaffold's `node.toml` (TLS material is generated under the run's directory). `wire-check`'s
authority pass admits a grant-required rule at the edge (the grant is issued at run time) and warns `unissuable grant`
when no rule admits anyone to `grant.issue` on it. `RuleDecl` and `MandateDecl` are `deny_unknown_fields` now — the
other section tables are not (stated in §65).

## Grants, the third PR (adopter-surface X3, 2026-10-25)

The surfaces. `gw_grants_stream` (`src/agent/http/grants.rs`): one task per client, woken by `subscribe_prefix("grant/")`,
reporting `pending()` records not yet reported and answers (`…/issued|refused|revoked`) of requests it reported; it keeps
two sets per connection. A2A: `Step::InputRequired(&str)` in `src/agent/a2a.rs`, emitted by a watcher task spawned beside
`working` on `grant/{serving node}/` — new open request → `input-required` with the id; answered → `working`; aborted
with `working` before every terminal event. The watcher reads the serving node's prefix, not the task's: a request the
node's agent writes for another reason is reported too (stated); each new unexpired request is reported once, `working`
when none this task reported still stands. The streamed dispatch's bound is the unary path's 120 s — the longest wait
through this door, stated. The stream emits `grant_expired` on a timer (the soonest reported expiry) and selects on
`tx.closed()`, so a gone client ends its task at once — `gw_overlay_log_subscribe` has the older shape (parked until
the next write), not this PR's. The 1.0 state name replaces the hyphen (`TASK_STATE_INPUT_REQUIRED`), with a unit test.
Rendering: `render_arguments_under` in
`state_machine.rs`; `AgentPolicy` derives `Default`, so `grant_renderable_classes` defaults to empty — a classified
argument set is withheld unless listed, unclassified ones render. `SendRequest` is `Clone` now (the watcher holds one).

## Grants, the second PR (adopter-surface X3, 2026-10-25)

The enforcement remainder of X3. What to know: a grant's holder is the **agent's node** (`node:{id}`), so the door
a grant reaches is the provider's, on the node's own call (`rpc_call_with_mandate`); a bearer presenting a node's
grant at `/mcp` is refused as not the holder's before anything else. Single use is claimed in `ae_preflight` after the
evaluator permits — `ExecutionAuthority::claim_grant_use` (lock-order row 70) plus the marker
`grant/{agent}/{request}/used/{door}` read first (the WAL half) — so the gateway and the provider each spend their
own use of one call. A grant's checkpoints (`…/checkpoint`, seq 1 at issue, seq 2 at revocation) travel in the KV:
`feed_grant_checkpoint` offers one before every assessment, and a watcher on `grant/` (spawned in
`with_execution_authority`) offers every one that lands, which is what carries a revocation to another node's sweep.
**Found and fixed:** the sweep judged granted work by `ExecutionGate::check`, whose scope rule a grant's own scope
fails — granted work was cancelled at the first tick; it is judged by `grant_standing` and the window now. The
revoked refusal is *not established* (an error-outcome audit seal); the used refusal is a *denial* — the distinction
`prove_denials` sees. Cross-node issuing needs the agent node's `EntitlementTable::entitle_grants_from_fleet()`.
Tests: the four new ones in `src/agent/http/grant_tests.rs` (the restart one over `[persistence]`, dropping every
handle on the context before the restart — the state machine holds one, and with it the journal's lock).

## Selective recovery — the `recovery` class (adopter-surface X9, 2026-10-25)

`src/faults/recovery.rs`. The plan's decisive demonstration as a sixth class of `mycelium test --faults`: three
nodes with a reconciler (`mycelium::effects::Reconciler`), an evidence journal and the evaluator each, one mock
destination (`Lossy`, in process, losing an answer once per key: `lose:` applied-unanswered, `vanish:`
neither); a lost acknowledgement, dependents carrying the parent (`apply_caused`), independents, node 0 cut and
restarted over its WAL (ledger and journal re-opened from disk), node 2 dark; then ten checks. What to know:

- **The move.** The side-effect contract is `mycelium::effects` now (`src/effects/{mod,external,http,counting}.rs`),
  the companion re-exporting it unchanged — the published binary cannot depend on a companion that depends on it.
  The ledger is the node-local journal (`Ledger { journal, records: Mutex<HashMap> }`, lock-order row 68), folded at
  open, one owner at a time; `rusqlite` stays in the companion.
- **Phases.** A (unseamed): fleet, effects, crash, restart, cut. B (seamed under `sim`): probe before retry, the
  idempotent dependent's probe and two concurrent retries, the closure over two journals. C (unseamed): node 2 back.
  D (seamed): the closure complete, the compensatable probe, the compensation under the permit, the leave,
  independents, the destination's counts, the rendering. The kernel is installed around B and D only, so the
  recording holds the reconcilers' `Input` and clock choices and nothing of the fleet's schedule.
- **A hole hides what hangs below it.** With node 2's journal withheld, node 1's `notify-103` (parent: node 2's
  `points-102`) is outside the closure too; the reader names the hole. The check expects exactly that.
- **The compensation's record is under the evaluator's envelope**, so its operation is the class's declared one
  (`notify-retract`), not `effect.compensate`; the rendering check looks for the declared name.
- **The seamed form's checks on the destination's counts** read the recorded answers instead and say so
  (`replaying`); the test asserts `destination_reached_in_recovery == 0` separately.
- Corpus: `tests/replay-corpus/x9-selective-recovery/` (`MYCELIUM_RECORD_CORPUS=1 cargo test --features
  sim,test-util,tls --test faults_recovery_replay record_the_recovery_into_the_corpus` re-records).
- Pages: `docs/features/selective-recovery.md`, `what-is-proven.md`'s line, `deprecations.md` §62; the one-hour
  job's step 5 runs the class on the published binary from v2.38.0, the first release carrying it.

## Framing discipline

Mycelium is a **library, not a platform** — no daemon, no control plane; a cluster is
emergent from network reachability; fleet observability is the operator's stack aggregating
per-node `/metrics` ([deployment-framing](../domain/strategy/deployment-framing.md)).

## The documentation and examples pass (adopter-surface X11, 2026-10-25)

The closing row. `tests/first_hour_tutorial.rs` holds tutorial 7 to the `one-hour` workflow: it hand-parses the
YAML (`- name: "N · …"` then the `run: |` block, lines indented deeper than `run:`), strips the job's bookkeeping
(`set -euo pipefail`, the skip marker, `t=$(date +%s)`, the CSV `echo`, comments) and compares with the tutorial's
fences tagged `bash one-hour:N`; a step without a block or a block that drifts fails. Editing a step means editing
both, which is the point. The front doors: the router keeps its four routes (the gate's structure) and the
*Build a fleet* line gained the no-toolchain path's targets; the five steps are untouched — they are compared by
`check-positioning.sh` against `positioning.md` and are the toolchain path by design. The compass gained a row; every
compass row must link `docs/capabilities.md`. The glossary's multi-term rows carry one `<a id>` per term
(`check-feature-pages.py` resolves `concept:x` to `id="x"`), and the registry's `ids` make the owning page link each.
Guide 18 said the reconciler's ledger was SQLite — stale since X9 moved it onto the node-local journal; fixed here. `mycelium explain --closure` resolves its argument with `Corpus::resolve` (`src/closure.rs`) after the corpus loads: a full
hash as given, or a unique prefix of at least `MIN_HASH_PREFIX` (8) lowercase hex — the 12 characters a `--correlation`
chain line prints; an unknown or shared prefix is a usage error naming which (doc-coverage run 24, gap 1).
