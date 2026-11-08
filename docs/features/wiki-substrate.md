# The wiki substrate

## What it is

`mycelium-wiki` is a group-scoped, curated wiki for agents: the durable canon a group maintains, beside the tuple
space's ephemeral pull and the blackboard's competitive claim. The corpus is not in gossiped KV. It lives in a
pluggable store — a shared directory (`FsStore`) or a git checkout where every write is a commit (`GitStore`) — and
agents read it directly, in parallel. Writes go through one elected curator per group: agents
propose edits into a small evaporating queue in KV, and the curator applies them with a section-level compare-and-swap,
reconciling a same-section conflict with a model when `llm` is on and by a lossless append-merge when it is not.
Mycelium is the coordination layer: the curator's election and failover, the store's location, access brokering and
an MCP tool — never the storage.

## When to use it, and when not

- **Use it when** a group of agents should keep one curated body of knowledge, edited concurrently, with one writer of
  record and a reviewable history.
- **Not when** the store is on one node's local disk: failover moves nothing, so the corpus goes with that node; and not
  when you need attributable, competing statements resolved by policy, which is the [knowledge layer](knowledge-layer.md).

## Turn it on

This feature is turned on in code, from the `mycelium-wiki` crate. With no feature it is the data plane only — the
`WikiStore` trait and `FsStore`, Mycelium-agnostic. `control-plane` adds the curator, its election, the proposal queue,
access brokering and the MCP tool; `llm` the model reconciler and linter; `gateway` the HTTP routes; `git-store` the
git-as-truth store; `git-mirror` a git projection of the store; `execution-authority` a curator mandate re-checked at
every commit.

- [`Wiki::new`](../../mycelium-wiki/src/agent.rs) with a `WikiConfig` (`control-plane`): the group, the `role`
  (`Auto`, `Curator` or `Reader`), `cap_refresh` (2 s), and a store.
- [`FsStore`](../../mycelium-wiki/src/fs.rs) or [`GitStore::open`](../../mycelium-wiki/src/git_store.rs)
  (`git-store`): put the store where it outlives any node.
- [`GitMirror`](../../mycelium-wiki/src/sink.rs) (`git-mirror`): a best-effort projection, one commit per applied round;
  its push is checked against its own `GitMirrorConfig.egress`, not the node's list, and that is unset by default —
  set it, or a non-local remote is not restricted.
- [`ExecutionGateAuthority`](../../mycelium-wiki/src/execution.rs) (`execution-authority`), passed to the store as its
  authority — `GitStoreConfig.authority`, or [`FsStore::with_authority`](../../mycelium-wiki/src/fs.rs): every commit
  and push then re-checks the curator's mandate window, epoch and revocation standing. Built and not passed, it checks
  nothing.
- The routes `/gateway/wiki/read` and `query` under `wiki:read`, `propose` and the bulk `ingest` under `wiki:write`
  (`gateway`, mounted with `with_http_routes`). Scopes are checked only where the node is built with
  `compliance`: the crate depends on `mycelium` without default features, so the scope wall is your build's, and
  without it any valid bearer reaches the routes ([what-is-proven](../operations/what-is-proven.md#companion-s-rpc-kind-needs-family)).
- `curator_elects_applies_and_fails_over_against_the_same_store`, in
  [`mycelium-wiki/tests/failover.rs`](../../mycelium-wiki/tests/failover.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`wiki_chat`](../../examples/README.md#the-capability-matrix): agents propose edits and a curator reconciles them over a
mock model; CI runs it. [`curator_handover`](../../examples/README.md#the-capability-matrix), also in CI, changes the
curator's appointment while a write is in flight: the write verifies the appointment and commits in one git
transaction, or does neither. [`wiki_council_viz`](../../examples/README.md#the-capability-matrix) is the browser view.

## What it guarantees, and what it does not

- No guarantee or rule id of its own; with `execution-authority` a write is decided by the same mandate fence as any
  [mandate](mandates.md).
- **Not built:** an object-store `WikiStore` — the trait admits one and none ships; a node-independent store of its own —
  provisioning one is yours; garbage collection of orphaned
  sections in `FsStore`; a metric or endpoint naming the curator; a mirror that is current to the write — read the
  store, not the mirror ([companions § mycelium-wiki](../operations/companions.md#mycelium-wiki--durable-curated-canon)).
- **Evidence:** the [what-is-proven](../operations/what-is-proven.md) ledger has no row for the wiki on its own; the
  council-scale measurement is the row [ten councils writing one git store concurrently](../operations/what-is-proven.md#ten-councils-writing-git-store-concurrently),
  from [`plans/council-substrate-hardening.md`](../plans/council-substrate-hardening.md).

## Go deeper

- **Guide:** no guide chapter; the [examples README](../../mycelium-wiki/examples/README.md), and
  [21 § Run the demonstration](../guide/21-mandates.md#run-the-demonstration) for the curator's mandate.
- **Runbook:** [companions § mycelium-wiki](../operations/companions.md#mycelium-wiki--durable-curated-canon).
- **Design:** [`design/wiki-git-store.md`](../design/wiki-git-store.md),
  [`design/wiki-concurrent-edit.md`](../design/wiki-concurrent-edit.md) and
  [`design/transparency-council-substrate.md`](../design/transparency-council-substrate.md).
- **Code:** [`mycelium-wiki/src/`](../../mycelium-wiki/src/lib.rs) (`agent.rs`, `store.rs`, `git_store.rs`, `sink.rs`,
  `execution.rs`, `http.rs`).
