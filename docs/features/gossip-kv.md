# Gossip KV

## What it is

Layer I: every node holds a replica of one key-value store and gossips its writes to its peers, which forward them
on. Conflicts resolve by last-writer-wins on a hybrid logical clock, so "last" means causally last, and a
periodic, chunked anti-entropy exchange repairs whatever a round missed, including for a node that joins late. A read
is local and makes no network call; the replica is eventually consistent, so a read can be stale. A write is
size-capped, a delete is a tombstone collected after a window, and the store never enforces a higher layer's rule on
what is written.

## When to use it, and when not

- **Use it when** state every node should see — advertisements, presence, configuration, application keys — can
  tolerate a brief stale read and settles by last write.
- **Not when** two writers must agree on one value: last-writer-wins keeps one of them silently;
  [consensus](consensus-electorates.md) decides instead.

## Turn it on

This feature is turned on in code: every node has it from [`GossipAgent::new`](../../src/agent/mod.rs) and `start()`.
The two-domain scaffold's [`kitchen-gw.toml`](../../templates/two-domains/nodes/kitchen-gw.toml), which CI's
two-domains job starts, sets the listening address.

- [`bind_address`](../reference/configuration.md#bind_address) and [`bind_port`](../reference/configuration.md#bind_port):
  where the node gossips.
- [`bootstrap_peers`](../reference/configuration.md#bootstrap_peers): the peers it dials first.
- [`default_ttl`](../reference/configuration.md#default_ttl): how many hops an update is forwarded, not how long an
  entry lives — an entry has no lifetime of its own.
- [`propagation_window_secs`](../reference/configuration.md#propagation_window_secs): a tombstone is collected once it
  is older than `default_ttl × propagation_window_secs × 10` seconds — about 50 minutes at the defaults — measured on
  the wall clock by the GC tick, not through the replay seam.
- [`KvHandle`](../../mycelium-core/src/kv_handle.rs), from `agent.kv()`: `set`, `get`, `delete`, `scan_prefix`,
  `subscribe` and `subscribe_prefix`; the receipt methods are [durability receipts](durability-receipts.md).
- `lww_equal_timestamp_concurrent_data_converges`, in [`mycelium-core/src/store.rs`](../../mycelium-core/src/store.rs):
  two writes with the same timestamp, applied in either order, leave the same value.

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`hello_mesh`](../../examples/README.md#the-capability-matrix), two nodes sharing a key; CI compiles it and does not run
it. [`conway`](../../examples/README.md#the-capability-matrix) is a grid of nodes each reading its neighbours from its
local replica, in a browser.

## What it guarantees, and what it does not

- **Rule:** [`kv.propagation`](../reference/rule-catalogue.md#kvpropagation) — every update converges by
  last-writer-wins; a frame too large, a bad signature or a removed sender is refused, and a TLS node refuses an
  unsigned KV write frame or one in the bare pre-2.33 signature form (rule revision 2;
  [`mesh.signed_writes`](mesh-mtls.md)).
- **Rule:** [`kv.expiry`](../reference/rule-catalogue.md#kvexpiry) — no entry carries a TTL. A capability or
  requirement advertisement its writer stopped refreshing stays in the store until overwritten or tombstoned; every
  reader skips it once it is older than three refresh intervals ([`CapEntry::is_fresh`](../../src/capability.rs)) —
  resolve, watch, wiring, demand, coverage gaps, role concentration, the A2A provider resolution and card, the
  membership governor's eligibility, the gateway's resolve, demand, shard and LLM lookups, the group projection and
  agent facts (rule revision 3; until it, the A2A, governor and gateway readers did not). A node judges its **own**
  entries on staleness only — the provider preflight, the group projection, its own eligibility and its agent facts
  (revision 4) — so a clock that reads its own stamp as ahead does not cost it its own skill; a peer's entry is judged
  both ways. An own entry stamped far ahead and persisted before a clock correction therefore stays live until the
  wall clock catches up with its stamp. A
  tombstone is collected by the sweep (revision 2 replaced a function and tests the tree does not have).
- **Not built:** agreement between writers; authenticated anti-entropy (a state reply's entries carry no author, the
  [mesh mTLS](mesh-mtls.md) page's limit); the store enforces no ownership of a namespace — the doors do, and the
  `sys/` tripwire counts rather than refuses. An inbound entry stamped beyond the receiver's drift bound
  ([`max_clock_drift_ms`](../reference/configuration.md#max_clock_drift_ms)) is counted and still applied
  (`kv_future_stamps` on [`/stats`](../operations/observability.md#reading-stats)): it holds its key until the
  wall clock passes its stamp, and nothing refuses it
  ([the ledger row](../operations/what-is-proven.md#far-future-stamp-holds-its-key)).
- **Evidence:** the ledger rows [a TLS node refuses unsigned KV frames](../operations/what-is-proven.md#tls-node-refuses-unsigned-kv-frames),
  [the 100-node cluster](../operations/what-is-proven.md#100-node-cluster) and the
  [hot-path costs](../operations/what-is-proven.md#hot-path-cost).

## Go deeper

- **Guide:** [01, gossip KV](../guide/01-gossip-kv.md).
- **Runbook:** [deployment § Seeds and bootstrapping](../operations/deployment.md#seeds--bootstrapping) and
  [tuning](../operations/tuning.md).
- **Design:** the [runtime invariants](../wiki/dev/architecture/runtime-invariants.md); the HLC's design and limits
  are the module doc of [`mycelium-core/src/hlc.rs`](../../mycelium-core/src/hlc.rs).
- **Code:** [`mycelium-core/src/store.rs`](../../mycelium-core/src/store.rs) (LWW, the tombstone sweep),
  [`mycelium-core/src/connection.rs`](../../mycelium-core/src/connection.rs) (frames and anti-entropy),
  [`mycelium-core/src/kv_handle.rs`](../../mycelium-core/src/kv_handle.rs).
