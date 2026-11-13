# CLAUDE.md — Mycelium session on-ramp

Fast on-ramp for code-assistant sessions. This file is deliberately thin: it holds the
**workflow rules, the build/test gates, and the hottest invariants** — everything else
lives in the **LLM wiki** and the code canon it cites.

## What this is

**A fleet that grows the capabilities it lacks, under rules it can show it kept.** Mycelium is an embedded library for agent fleets with no coordinator. Generic nodes install signed capabilities when demand goes unmet and re-heal what they declared when a provider dies. Where you configure an enforcement point, it checks authority before work runs and records what it decided; a recorded run replays.
(the hero and the one description, `docs/positioning.md`, gated by `scripts/check-positioning.sh`). Concretely: an
embedded, broker-less Rust library — a three-layer substrate for AI agent
fleets and storage replication: **I** gossip KV (LWW + HLC, Merkle anti-entropy) ·
**II** signal mesh (scoped events, admission boundaries, opacity) · **III** epidemic
consensus. Layers I+II are the `mycelium-core` crate; `mycelium` adds III, capabilities,
services, gateway, tls. It is a **library, not a platform** — no daemon, no control plane;
a cluster is emergent from network reachability (peer-exchange + CA admission — **not**
`cluster_name`, which is a cosmetic label).

**Current release:** **v2.40.0 released 2026-11-12** (tag `v2.40.0` — a MINOR: **identity and lifecycle wave 1a** (`docs/plans/identity-and-lifecycle.md` §8, every row closed on evidence): **I0** `[authority]` — a node's evaluator, execution authority, provider enforcement and evidence journal from its declaration, so a `mycelium` binary with the `[authority]` table set enforces at its doors (without the table nothing changes, §83); **I1** client certificates for services (`mycelium tls issue --client`, `mycelium-client://` refused at the mesh, `sys/client-cert/`), a client's own `cs`/`cn` signature checked at the door and the provider, grants held by a client principal, the SDKs' `McpClient` and `ClientSigner`; **I3** the RFC 8785 arguments digest in Rust, Python and TypeScript (the older digest honoured for one release) and signed grant decisions relayed under `grant:relay`; **I5** an OIDC user's token exchanged at the gateway (RFC 8693, DPoP-bound) and verified at the provider, gated against Keycloak in CI — and **R5–R9 of the external code security review of 2026-10-31** (`docs/plans/external-review-2026-10-31.md`: the gateway closes a silent connection and caps how many it serves, a journal syncs its directory before its first acknowledged record, bulk under `gateway_tls` with a payload cap, a replica-sync answer `Failed` while a record is between apply and append, the audit failing on an unaccepted warning); the feature pages F3 and F4 (`docs/plans/feature-pages.md` — every registry feature has a page, audited); RUSTSEC-2026-0459 (SQLite, `mycelium-effects`) and -0486 (`rustix`), neither reachable from a shipped binary; the published stem built with `compliance`, and the node's `gateway_a2a` and `gateway_a2a_default_skill` serving A2A from configuration (the stem refuses `gateway_a2a`, §92); stale advertisements skipped by every reader; the Rust ecosystem watch. Companions `mycelium-py` 0.2.15, `mycelium-ts` 0.2.9, `mycelium-reason` 0.11.1, `mycelium-guardrails` 1.1.1. Wire **v12** unchanged. **Upgrade notes:** `deprecations.md` §78–§98 — the reference evaluator decides the `requires_values` rules it left `Indeterminate`, so check each (§98); `GossipConfig` gained `gateway_max_connections`, `max_bulk_payload_bytes`, `authority`, `gateway_a2a` and `gateway_a2a_default_skill`, `GatewayTlsConfig` `bulk_ca_pem_path`, and `OidcConfig` five `exchange_*` fields; Tokio 1.53.3 is every manifest's floor; `mycelium-effects` moves to `rusqlite` 0.41; `ExecutionAuthority::new` and its siblings take a shared issuer set; upgrade every node before issuing a client certificate, and verifiers before binders for the digest).

**Earlier releases are not repeated here.** Each release, back to v2.0 (complete 2026-06-21, all 16 milestones), is in
`CHANGELOG.md` and the delivery ledger [dev/history](docs/wiki/dev/history.md); each release's upgrade notes and
*Check:* lines are in `CHANGELOG.md`, and the deprecations and behaviour changes in `docs/guide/deprecations.md`. At a release, **replace** the paragraph above; do not append to it (`RELEASING.md` § 6).

Wire **v12** (`PREV = 11`). Scopes are **`Cluster · Group · Individual`** (all / subset / one),
shared by `SignalScope` and consensus (`cluster_propose` / `group_propose`). *Renamed 2026-07-10:*
`System` → `Cluster` (wire-compatible; `system_propose` kept as a `#[deprecated]` alias, gateway
still accepts `"system"`); `system_stats()` is unrelated — node-local runtime state, not a scope.

## The wiki workflow (non-negotiable)

1. **Query first.** Start any task by reading [`docs/wiki/wiki.md`](docs/wiki/wiki.md) →
   the relevant section/pages. Don't re-derive what the wiki already states.
2. **Ingest on completion.** When finished work produces durable knowledge (new invariant,
   root-caused bug family, shipped workstream, revised position): update the page(s),
   refresh folder-notes, add one dated file to the section's `.log/`.
3. **Schema:** [`docs/wiki/AGENTS.md`](docs/wiki/AGENTS.md). **Code is canon** — the wiki
   cites `src/` rather than paraphrasing it; on conflict trust the code, then fix the page.
4. **Lint periodically** via the `/wiki-lint` skill — doc-vs-code verification first (the
   check that catches drifted claims like the Run-28 lock-table finding).

Private memory (`~/.claude/.../memory/`) holds user preferences and session state only —
promote durable project knowledge to the wiki.

## Verification policy (non-negotiable, adopted 2026-10-06; rule 5 2026-11-07)

**Verify a change against the surface it touches, not against its own description.** Checks derived from
a change's description only confirm what its author already thought of; the defects that survived green CI
and repeated analysis runs sat at the edges — sibling functions, other doors, plan promises not in the PR,
files missing from an allow-list ([verification-policy](docs/wiki/dev/testing/verification-policy.md) has
the evidence).

1. **Enumerate before fixing.** For any fix to an invariant, first list every entry point that reads or
   writes the same thing — and the grep that found them — and test each one.
2. **Plan rows close on evidence.** Closing a row means quoting each of its promises next to the code or
   test that delivers it; an undelivered promise is written *not built*. Someone who did not write the PR
   checks it.
3. **CI collects tests by discovery, not a list.** Point runners at directories; live suites skip themselves
   when no node is configured, except in the step that starts the node, which sets their `*_LIVE_REQUIRED`
   guard. The CI job `test-coverage` (`scripts/ci-test-coverage.py`) fails on any test it knows of — from the
   logs and the listed universe — that did not execute in the run; `scripts/check-test-inventory.py` is its fast
   pre-push approximation and covers what the universe cannot list.
4. **Independent adversarial review after each PR.** Before merge, an agent other than the author enumerates
   the surfaces the change touches and tries to break them (`/adversarial-review`); its findings are fixed or
   answered in the PR.
5. **A feature's page changes with the feature.** A PR that adds or changes a feature adds or updates its page
   under `docs/features/` and its entry in `docs/features/registry.toml`, and the independent reviewer checks both.
   Exceptions, stated in the PR: a change that leaves the page true (the PR says why); an entry at `page = "todo"`
   that predates F3's close (`docs/plans/feature-pages.md`) needs only its entry — after F3, a new feature lands with
   its page. `scripts/check-feature-pages.py` (in `make check`) sees only its seven id sources (glossary, guarantee
   and rule catalogues, unit-file keys, Cargo features, companion guarantees, crates); for a configuration field, a
   route or an SDK verb the reviewer is the only check.

## Where to read what

| For | Read |
|---|---|
| Public API + KV-namespace ownership | `src/lib.rs` crate doc |
| Wire format + version policy | `mycelium-core/src/framing.rs` (top) |
| HLC design + limits | `mycelium-core/src/hlc.rs` module doc |
| Capability model | `src/capability.rs` |
| Purpose / roadmap | `docs/philosophy.md` · `ROADMAP.md` |
| The agent guide the binary serves; other agents' entry file | `mycelium skills get fleet` (source `docs/reference/agent-skills/fleet.md`) · root [`AGENTS.md`](AGENTS.md) for Codex, Cursor and Orca's workers — Claude Code reads it only where there is no `CLAUDE.md`, so this row is how a session here learns of both (gated: `src/agent_guide.rs`) |
| Docs map (guide, operations, design, plans, publications, analysis) | `docs/README.md` |
| Architecture, concurrency, testing lore, security, companions, history | the wiki: [`docs/wiki/`](docs/wiki/wiki.md) |

## Build & test gates (run before pushing)

**`make check`** is the one-command pre-push gate — clippy across the feature matrix CI enforces
(feature-matrix + `--no-default-features` + core), ~3 min, no wasmtime. The `--no-default-features`
clippy is the catcher for the *feature-gated dead-code trap* (an item live only under
`gateway`/`metrics` is dead in a minimal build — CI's Gateway-free + WASM-host jobs). `make
check-full` adds the test suites + wasm-host clippy. The underlying set:

```bash
cargo test --lib --features tls,metrics,a2a,llm,test-frame-filter   # CI's line; test-frame-filter for the lock_lifecycle* tests
cargo test --lib --features test-frame-filter                       # CI's default-feature lib run
cargo clippy --lib --tests -- -D warnings                 # default features — catches tls-only dead code
cargo clippy --lib --tests --features tls,metrics,a2a,llm -- -D warnings
cargo test --lib --features compliance,a2a                # CI runs it strict (CI_RETEST_STRICT=1)
cargo test --lib --no-default-features --features gateway
cargo clippy -p mycelium-core --lib --tests -- -D warnings
cargo clippy --lib --bins --tests --no-default-features -- -D warnings   # CI's line; test-only items (not gateway-free: dev-deps re-enable it)
cargo clippy --lib --no-default-features -- -D warnings   # catches the feature-gated dead-code trap
```

Companion crates: `cargo test -p mycelium-tuple-space --features gateway`, same for
`mycelium-blackboard` (+ clippy `--all-targets`); `cargo test -p mycelium-effects --features tuple-space`
(+ clippy with and without the feature; `rusqlite` bundled, quarantined to that crate); `cargo test -p mycelium-commitment`
(+ clippy `--all-targets` + `cargo run -p mycelium-commitment --example redistribution_cn`, the CN1 gate). The acceptor's model (critique plan CR10): `cargo test -p mycelium-model --release` (+ clippy `--all-targets`) — read the wiki's [acceptor-model page](docs/wiki/dev/testing/acceptor-model.md) before changing `prepare_slot`, the accept rule, the phase-1 rule or `read_slot`: the model calls them, and a pinned state count moves when they do. CI also gates `tsc --noEmit` and the TypeScript SDK's `jest` suites — node-free, and since 2026-10-05 the live suite against a
`mycelium` node — the AFN +
coop smokes, fuzz (non-PR), and `cargo audit`. **Never trust a memorised test count** — run
the suite. Scale suites: `make test-scale` (100 nodes), `test-scale-resilience`,
`test-scale-entries` — read the wiki's
[scale-tests page](docs/wiki/dev/testing/scale-tests.md) before interpreting failures
(Docker-bridge iptables ceiling, VM fatigue).

## Hot invariants (the ones that ship regressions when forgotten)

- **One lock per function**, flat acquisitions only — the [lock-order
  table](docs/wiki/dev/concurrency/lock-order.md) claims completeness **over the library crates
  (`src/`, `mycelium-core/`, `mycelium-*/`), not `examples/`**: adding any
  `Mutex`/`RwLock` field there means adding a row.
- **papaya:** `compute` closures retry-safe; never act on a stale read — the whole
  recurring race family, rules + reference impls in
  [lock-free-and-atomics](docs/wiki/dev/concurrency/lock-free-and-atomics.md).
- **Individual-scope forwarding is unconditional** (flood fallback); only *admission* is
  scoped. Do not "optimize" it away. Two carve-outs, neither precedent: a frame addressed
  to *this* node terminates here (routing at the terminal, #162), and a removed member's frames
  are dropped at the connection (origin-keyed membership admission, closure plan C5) —
  [runtime-invariants](docs/wiki/dev/architecture/runtime-invariants.md).
- **Detection, not prevention:** never teach Layer I a higher-layer law (no prefix write
  guards in `apply_and_notify`) — tripwires + counters instead.
- **A security-bearing record in the gossip KV is signed by its owner and verified at read** — any
  admitted member can write or tombstone any key, so a record that decides authority, admission,
  cancellation, retention or audit is never trusted for where it sits: signed under the key its path
  names (or the authority the reader is configured to trust), body checked against the path and for
  currency, a failed record read as absent and counted, **absence failing closed**, a forged-record
  test. Availability and bounded, detected soft state are outside it — the rule and the inventory of where it
  does not hold yet: [owner-signed-records](docs/wiki/dev/owner-signed-records.md).
- **Consensus listeners register synchronously**; multi-node consensus tests need a
  listener on every node + a structural peer-ready poll (never fixed sleeps) —
  [testing](docs/wiki/dev/testing/testing.md).
- **KV writes are size-gated** (`framing::MAX_KV_WRITE_BYTES`); anti-entropy is chunked;
  a `FrameTooLarge` frame is dropped without tearing down the connection.
- **Apply to the store, then hand the record to the WAL** — never the reverse — and a WAL ack is
  a durability claim (`Err` when the writer is gone; `append_sync` fsyncs in every `SyncMode`).
  The snapshot merges the WAL tail before truncating; replay is LWW, not a watermark —
  [runtime-invariants](docs/wiki/dev/architecture/runtime-invariants.md) §Persistence.
- **An ack is a receipt that names its rung and nothing above it** — local application · local sync
  · replica sync · destination commit; a timeout is `DeliveryUnknown`, never a negative; same
  `operation_id` + different content is a `Conflict`. Today's `bool`s and `persisted` are pinned by the
  regression floor (`floor_*` tests + the golden on-disk fixtures under `tests/fixtures/persistence/`,
  replayed in CI); a PR changes an ack's meaning by changing a pin, in the open —
  [contracts-receipts ADR](docs/design/contracts-receipts.md).
- **No direct clock, RNG or filesystem access in production logic** — in the three crates the gate
  scans (`src/`, `mycelium-core/src/`, `mycelium-wasm-host/src/`) — every one goes through
  `mycelium_core::sim_seam` (`wall_now_ms`, `mono_instant`, the timer seam the node's periodic loops tick
  through; the wasm host's librarian sync and `[[serve]]` loops and `run_capability_probes`' health
  loop sleep on tokio directly and sit in the baseline), or a recording cannot replay deterministically. `scripts/check-sim-seams.sh` (in `make check`)
  holds a baseline of the permitted call sites in `scripts/sim-seams-baseline.txt`; a
  new one fails the gate until it is routed or the baseline is updated in the open. Off in every
  shipped build (feature `sim`) — [`mycelium-core/src/sim_seam.rs`](mycelium-core/src/sim_seam.rs).
- Ports via `test_util::alloc_port`; env-var tests hold `config::tests::env_test_lock()`.

## Active work

**The index of plans is [`docs/plans/README.md`](docs/plans/README.md); what shipped, and when, is the delivery
ledger [dev/history](docs/wiki/dev/history.md).** Delivered plans are not summarised here: read their rows' closures
in the plan files. Every gap that is claimed but not gated, dated, is on `docs/operations/what-is-proven.md`.

**In progress: identity and lifecycle** (`docs/plans/identity-and-lifecycle.md`, agreed 2026-11-07; its status line
is the state of play).
- **Wave 1a, released in v2.40.0:** I0, I1, I3, I5, each closed on evidence in the plan's §8.
- **Wave 1b (one MINOR), next:** I4 (cancellation under a `call:` mandate), I2 (CA succession), I7 (the developer's
  identity), each written from its spike note in `docs/plans/spikes/`; and with them, from the critique plan, CR2's
  first PR (the grant use marker), CR13's counter and CR12 (`#[non_exhaustive]` on `GossipConfig`). A row is built only once its header (or, for a critique-plan row, its State line) says
  *buildable*.
- **Wave 2 (one MINOR), the developer surface:** N1 and N3–N7 (N7 the Orca plugin). N3 (`--format json`) is delivered,
  unreleased (#767); N5 needs nothing from wave 1; N4's identity half and N7 wait on I7. The examples and records
  E1–E6.
- **Open items the plan tracks (§9):** O2, a reply to a restarted peer is lost (#746); O3, O5 and O6, counters that
  become refusals one release after they ship. Beside them, issue #753: a bounded retry for `unknown_signer`
  (`docs/operations/what-is-proven.md`).

**Open elsewhere:**
- **Adopter surface:** Y9 is paused until a cloud credential is held; each row's stated *not built* items are in the
  plan's §8.
- **Post-360 hardening:** the *not built* items and the known flakes, `docs/plans/post-360-hardening.md` §3.6.
- **The third-party critique of 2026-11-12:** agreed 2026-11-12, all sixteen decisions answered (`docs/plans/external-critique-2026-11-12.md` §5): CR1 now (#774), CR2's first PR, CR13's counter and CR12 inside wave 1b, CR3–CR11 and the rest after it; CR10 (the acceptor's model, `mycelium-model`, CI job `model`) is delivered, unreleased.
- **Contracts axis, not code gates** (each needs a machine, a counterparty or a credential, not a commit): V1, the nightly scale runner whose self-hosted box is offline, so its criterion has never been
  met; AE4's four joint AWS/GCP runs, which need the counterparty; S4, real object-store buckets, which need
  credentials.
- **Design-time tooling:** Q4, and Q5's cross-language golden vectors (`docs/plans/README.md`); the artifact
  library's crate-naming question (`docs/design/artifact-library.md`).
- **The Rust ecosystem watch:** `docs/plans/rust-ecosystem-watch.md`, reviewed monthly; W1 (build provenance) is
  ready to become a plan row.
- **Research track:** the three-arm work-distribution experiment (Paper 1) and the monetary-ecology article revision
  ([wiki](docs/wiki/domain/publications.md)).

**Nothing in CI enforces doc-vs-code accuracy.** `/doc-coverage` and `/wiki-lint` are operator-run. The self-audit
series is `docs/analysis/ratings.md` (run via `/mycelium-analysis`).
