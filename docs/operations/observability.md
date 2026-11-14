# Observability

What a running Mycelium cluster exposes, and how to see what it's doing — without
modifying it. Everything here needs the gateway on (`http_port` set; see
[deployment.md](deployment.md)).

> Audience: **DevOps**. Developer-side inspection (reading any layer's KV state
> from code) is in the [cookbook](../guide/cookbook.md).

## Public endpoints (no auth)

These are deliberately *outside* the `/gateway` scope wall — always public,
uncredentialed:

| Endpoint | Tells you |
|---|---|
| `GET /health` | `200` = process alive (liveness probe). With `[persistence]` the body carries a `persistence` block: `wal_refusing_appends` (`true` while the WAL writer refuses every append after a failed write, until a snapshot truncates the torn tail — since 2026-10-09), its `reason`, and `dropped_appends` (appends with no WAL record: queue-full drops, the failed write, the refusals) |
| `GET /ready` | `200` = startup complete → serving KV/signals/membership (readiness probe). Capability discovery gossips independently and does **not** gate readiness — a node advertising no soft state is still ready (changed 2026-07-15) |
| `GET /stats` | `node_id`, `cluster_name`, `store_entries`, `dropped_frames`, `task_count`, and the tripwire counters (`commit_conflicts`, `consensus_decided_floor_anomalies`, `consensus_ballot_space_exhausted`, `sys_namespace_violations`, `cap_authz_violations`, `schema_mismatch`, `governance_changes` / `governance_unaudited` (accepted governance writes at this gateway — `/gateway/govern/*`, `identity/revoke` — and those that left no audit record; 2.27.0. A plain group's `mesh:write` join is audited, not counted), `rate_limited_senders`, `individual_flood_fallbacks`, `kv_future_stamps` with `kv_future_stamps_by_door`), plus liveness (`dead_shards`, `gc_alive`, `health_monitor_alive`) |
| `GET /metrics` | Prometheus scrape (requires the `metrics` feature). Carries a `cluster` label on every series when `cluster_name` is set |
| `GET /.well-known/agent-facts.json` | this node's self-certified AgentFacts (when the [facts lens](#viewing-agentfacts) is mounted) |
| `GET /consensus/{slot}` | a consensus slot's committed value + ballot + lease state — an application's slot, a lock's (`lock/{name}`) or a leader's (`leader/{group}`); a component's slot (`cn/`, `clog/`, `electorate/`, `ae-seat/`, `capauthz/`, and `consistent/{owned key}`) is **403** `protected_slot` whatever the scopes ([deprecations.md §102](../guide/deprecations.md)). **Bearer required** when a token model is set (scope `consensus:read`, since 2026-09-05); the four rows above stay public |

### Reading `/stats`

```bash
curl -s http://node:8080/stats | jq
```

- `store_entries` — live KV keys. Steady within an order of magnitude of your
  working set; unbounded growth means tombstones aren't being GC'd or a writer is
  looping.
- `dropped_frames` — backpressure. Non-zero after a burst is informative, not
  fatal; sustained growth means raise `GOSSIP_WRITER_CHANNEL_DEPTH` ([tuning.md](tuning.md)).
- `task_count` — Tokio tasks in the JoinSet (~17–20 on a 3-node cluster).
  Unbounded growth = a task leak (usually a per-peer writer not exiting).
- `individual_flood_fallbacks` — Individual-scoped frames (RPC, votes) that had **no direct
  route** and fell back to flooding. Persistent growth on a stable topology means an
  RPC-heavy pair isn't directly peered — the node also logs
  `Individual-scoped frame has no direct route; flooding via relay (topology pressure …)`.
  Remedy: pin the pair with `GossipAgent::connect_peer` — see
  [tuning §RPC-heavy pairs](tuning.md#rpc-heavy-pairs--the-topology-pressure-warn).
- `grant_use` — what this node's doors found at a grant's use-marker key (`grant/…/used/{door}`) that was
  neither a verified marker nor *never written*, by reading: `tombstone`, `unverifiable`, `legacy`,
  `release_elsewhere`, `release_forged`, `release_replayed` — **each read as used, so none re-arms a grant** —
  and `superseded`, this door's own marker or release write that lost to a newer value (the call refused
  `grant_use_superseded`, or the grant left spent), and `not_stored`, its marker write refused by the node's
  store at `max_store_entries` (the call refused `grant_use_not_stored`). In memory; zero after a restart. On a fleet at one version
  a non-zero `tombstone`, `unverifiable`, `release_forged` or `release_replayed` means a member wrote a marker
  key that is not its own; `release_elsewhere` is ordinary when a client retries a refused call at another
  gateway than the one that released it; `legacy` is expected only while doors older than 2.41 run. With
  `metrics`: `mycelium_grant_use_reads_total{reading}`. See [deprecations §106](../guide/deprecations.md).
- `kv_future_stamps` — inbound KV entries whose stamp was **beyond this node's drift bound**: physical
  time past this node's wall clock plus `max_clock_drift_ms` (300 s by default). **Counted and still
  applied**: under last-writer-wins such an entry holds its key until the wall clock passes its stamp, and
  nothing refuses it. `kv_future_stamps_by_door` splits the count by where the entry came in —
  `anti_entropy`, `data` (a live unsigned frame), `signed_data`. Always `0` with
  `max_clock_drift_ms = 0`.

  **What it is evidence of: that a far-future stamp is in circulation, not who sent it.** Three causes:
  1. **A sender's clock is ahead** by more than the bound: every receiver counts that sender's writes
     (each logs `remote HLC stamp exceeds the clock-drift bound` at most once every ten seconds).
  2. **This node's clock is behind**: this node counts the fleet's ordinary writes. Not only when it is
     more than the bound behind — the margin is the bound *minus* the furthest any member's stamps run
     ahead, so with one peer four minutes ahead (inside the bound, uncounted) a node 61 s behind counts.
  3. **Honest senders with good clocks, after any node has heard a stamp at or past the bound.** A node
     that hears such a stamp keeps its own clock at its limit. When the stamp's logical counter is full
     (`u64::MAX`, or a member's choice) its **next local write is stamped a millisecond past that
     limit**, so every peer whose clock is not ahead of it counts it; with a lower counter its next
     writes are *on* the limit, which a peer with the same clock does not count and a peer even a
     millisecond behind does. One far-future
     frame therefore makes many nodes count many senders; and a member can cause it with frames stamped
     exactly *on* the bound with a full logical counter, which are themselves never counted
     (`an_honest_write_after_an_observed_future_stamp_is_reported_by_a_peer`, `mycelium-core/src/hlc.rs`).
     "Every node counts, for many senders" is this case, not a fleet of bad clocks.

  **What the figure is not.** It is entries, not keys, senders or incidents, and **a member can raise
  it at will**: any number of frames stamped `u64::MAX`, or unsolicited `StateResponse` entries, which
  are unsigned and applied even by a `[tls]` node (existing behaviour, the
  [mesh mTLS](../features/mesh-mtls.md) page's limit). An entry the fleet **already holds** is counted
  again at `anti_entropy` on every round in which its bucket differs from a peer's — anti-entropy
  sends a differing bucket whole — so with ordinary churn the count keeps rising, on every node, after
  the sender's clock is fixed: the rate then reflects churn, not arrivals. A frame refused **before**
  the clamp — unsigned on a `[tls]` node, a removed member's, a repeated nonce, a failed or unknown
  signature — is not counted: it was not applied and moved no clock. Entries a node reads back from its
  own WAL or snapshot at start are not counted.
- `commit_conflicts` / `sys_namespace_violations` — the **detection-not-prevention
  tripwires**. A non-zero value means someone wrote to a `consensus/` slot or a
  `sys/{identity,identity-signed,identity-proof,load,role,tuple,caller-context,consensus-accepted}/{node}`
  key they don't own; the write was *applied* per LWW but flagged. `consensus_decided_floor_anomalies`
  (2026-10-10) counts slots whose decided floor or ballot key sits implausibly far (more than `2^32`)
  above every ballot this node has itself seen — a forged `consensus/decided/` or `consensus/ballot/`
  entry; still obeyed, so prepares on that slot keep refusing until it is dealt with.
  `consensus_ballot_space_exhausted` counts proposals this node ended because such a slot had no
  ballot left (a `Timeout`, reason `ballot_exhausted`). Investigate the offending node — see
  [00 · Concepts](../guide/00-concepts.md) on promise- vs mechanism-strength.

### Prometheus

With `--features metrics`, `/metrics` exposes the gossip hot-path gauges/counters
(`gossip_store_entries`, `gossip_peers_connected` — live outbound writers, the sockets, beside the
emergent `peers_known` / `peers_heard`, the *view*; `0` while `peers_known > 0` is a node that can see
peers it cannot reach — `gossip_anti_entropy_rounds_total`,
`gossip_messages_received_total`, `gossip_signals_delivered_total` /
`_rejected_total`, `gossip_rpc_latency_ms`, …) plus the emergent-diagnosis, governor,
artifact-library, guardrails, and reason-routing families. Scrape it like any service.

**Full reference → [metrics.md](metrics.md)** — the canonical, complete list of every
emitted series (type, feature, meaning, and what to watch for), one section per family.
The gossip names above are just a teaser.

### A note on the `system` scope name

Signals and consensus proposals carry a **scope** — `Cluster`, `Group`, or `Individual` (you'll see
it as the `scope` label on `gossip_signals_*` and as the `scope` field on a gateway signal emit).
**`Cluster` was called `System` before 2026-07-10.** The wire is unchanged and the gateway + SDKs
still accept `"system"` as a deprecated alias, so an old dashboard, config, or client that says
`"system"` still works and means `Cluster`. (`system_stats()` is unrelated — that is node-local
runtime state, not a scope.) Full vocabulary:
[guide 13 · Scope vocabulary](../guide/13-cluster-topology.md#scope-vocabulary-cluster--group--individual).

## Naming environments & monitoring many clusters

**A cluster has no built-in name** — a node is identified by its `node_id`
(`host:port`). For org-wide monitoring across several Mycelium environments, give
each cluster a name so one Prometheus / Grafana can tell them apart:

```bash
GOSSIP_CLUSTER_NAME=prod-eu    # or set GossipConfig::cluster_name
```

**`cluster_name` is a label, not a boundary** — it disambiguates dashboards; it does *not* define
membership (that's reachability + the CA). See
[guide 13 · what makes a cluster](../guide/13-cluster-topology.md#what-actually-makes-a-node-part-of-a-cluster).

## Monitoring a group (vs the whole cluster)

Cluster-wide health is `/stats` + `/metrics` + `/gateway/fleet` (above). For a **single group**:

- **Membership** — `agent.mesh().group_members("nlp")` → live members (`Vec<NodeId>`); for a
  capability group, `agent.capabilities().resolve(&filter)` lists matching providers.
- **Size vs. intent** — a **governed** group (under a `MembershipGovernor`) surfaces
  `GroupStatus { min, max, observed, conflict }` on `GET /gateway/fleet` (`governed_groups`) and as
  the `mycelium_emergent_governed_group_conflicts` gauge → alert on it (see
  [diagnostics.md](diagnostics.md#governed-group-conflict--thrash-the-56-pattern)).
- **Every group's size, governed or not** — `group_sizes` on `GET /gateway/fleet` (`fleet:read`), one
  `{group, observed}` per group with a live member (2.27.0, #168). A group that empties is **absent** from the
  list rather than `0`. The count is of membership keys, not live nodes: a crashed member stays counted until its
  `grp/` key is removed, as in `governed_groups` — read peer health from `/stats` alongside it. No per-group Prometheus label, by design (cardinality); any node
  answers the snapshot.

Full how-to (define + monitor): the cookbook recipe
[*"define a group, and monitor who's in it"*](../guide/cookbook.md#how-do-i-define-a-group-and-monitor-whos-in-it).

When set, the name is a **pure operator label** (no effect on gossip, identity, or
membership) and propagates to all three surfaces:

- `GET /stats` → a `cluster_name` field;
- `GET /metrics` → a `cluster="prod-eu"` **label on every series** (so a single
  Prometheus scraping many clusters can `sum by (cluster) (…)` and one Grafana
  dashboard variable selects the environment);
- AgentFacts (`.well-known/agent-facts.json`) → a `cluster` field, so a federation
  fetcher sees which environment a node belongs to.

**Mycelium ships no fleet console — by design** (it is a library, not a platform;
each cluster is self-contained). Fleet-wide aggregation is your monitoring stack's
job: point one Prometheus (or Thanos / a federated Prometheus / Datadog) at every
cluster's `/metrics`, label by `cluster`, and build per-environment dashboards.
Mycelium's responsibility ends at *exposing* the per-node signals with a cluster
label; aggregating them across clusters is the operator's.

## Auth on the monitoring endpoints

`/health`, `/ready`, `/stats`, `/metrics` are **deliberately public and
uncredentialed** — probes and scrapers must work without secrets (the M16 edge
criterion). Mycelium does **not** authenticate them. If your environment requires
auth or network isolation on `/metrics` or `/stats`, put it in front: a reverse
proxy (nginx/Envoy basic-auth or mTLS), a Kubernetes `NetworkPolicy`, or a service
mesh. *Management* surfaces (`/gateway/**` — governance, audit, transparency) are a
different story: those are scope-gated and OIDC-SSO-capable — see [rbac.md](rbac.md)
and [sso.md](sso.md).

## Viewing AgentFacts

AgentFacts is the node's self-certified, federation-facing metadata (see
[00 · Concepts](../guide/00-concepts.md), the developer how-to in
[17 · Federation](../guide/17-federation.md) — serve/verify + the domain board — and the
[`federation_facts`](../../examples/coop/src/bin/federation_facts.rs) demo). Mount
the lens (from the `mycelium-agentfacts` crate) before `start`:

```rust
agent.with_http_routes(mycelium_agentfacts::agent_facts_router(agent.clone(), opts));
```

Then anyone can pull and verify it:

```bash
# this node's signed facts (a NANDA-shaped JSON-LD document)
curl -s http://node:8080/.well-known/agent-facts.json | jq

# the converged, multi-author domain board (every node's verified facts)
curl -s http://node:8080/.well-known/agent-facts/domain.json | jq '.nodes[].node'
```

The document is self-signed (Ed25519) — a fetcher verifies it against the
embedded key; trust is the fetcher's decision (no issuer authority). The
[coop demos](../../examples/coop/) all mount the lens, so while any of them runs
you can `curl` the printed gateway port to inspect it live.

## Dashboards

**A Grafana dashboard ships in-repo:**
[`dashboards/mycelium-grafana.json`](../../dashboards/mycelium-grafana.json). Import it
into Grafana (it prompts for a Prometheus datasource) for KV-store, gossip-transport,
signal-mesh, and RPC-latency panels over the `gossip_*` family. It is a **starting point**,
not the whole picture — it covers the gossip hot path plus a header row of emergent /
guardrails / reason panels; the *full* metric set (governor, artifact library, and the rest)
is in **[metrics.md](metrics.md)**, ready to add as panels. Point its datasource at a
Prometheus scraping every cluster's `/metrics` and use the `cluster` label as a dashboard
variable to switch environments.

For a quick visual without Grafana: the `llm_agent` / `three_node_demo` examples ship a
live management UI (a mesh view + KV inspector) on their gateway port — the quickest visual
into a running cluster. SkillRunner exposes `/mgmt` (the audit + skill dashboard); see
[guide 05 · Skills](../guide/05-skills.md).

## The contracts axis — five counters

All `metrics`-gated. Every label below is a **small closed set**: verdicts, reasons, classes and
durability states. Nothing is labelled by principal, operation, resource or group, whose value spaces
grow with your traffic. That is the cardinality rule, and it is why these are safe to graph directly.

| Metric | Labels | Read it as |
|---|---|---|
| `mycelium_ae_decisions_total` | `verdict` ∈ `permit` · `deny` · `indeterminate`; `mapping` ∈ `mapped` · `unmapped` · `ambiguous` | **every** authorisation decision at the gateway, permits included |
| `mycelium_ae_preflight_refusals_total` | `reason` ∈ `action_denied` · `authority_not_established` · `evidence_not_recorded` | the refusals, by *kind* |
| `mycelium_control_decisions_total` | `decision` ∈ `proceed` · `held` · `would-hold`; `class` ∈ the five action classes | the control envelope: what the predicate did, and to which class of action |
| `mycelium_kv_receipts_total` | `local_durability` ∈ `on_disk` · `buffered` · `not_configured` · `failed` | the durability rung writes actually reached |
| `mycelium_gateway_caller_refusals_total` | `reason` | a caller context that failed verification — **never** a fall-back to running the call as the node |

**The decision counter is the denominator.** Refusals alone give you a numerator: ten denials could
be ten out of ten requests or ten out of ten million, and an alert on denial *volume* fires on
traffic growth. Divide by `mycelium_ae_decisions_total` for a rate that means something.

Its labels are the **evidence document's own vocabulary**, so a reader correlating a dashboard with a
record never has to translate. A test asserts the label and the serialised form are the same string,
because a rename would break both at once.

**`permit` with `mapping="unmapped"` should always be zero.** An unmapped or ambiguous operation
cannot be permitted, so a non-zero series there is a defect, not a policy question. Worth an alert.

**`evidence_not_recorded` is the one to page on.** The action was refused *even though the policy
permitted it*, because the decision could not be written down. That is a storage or journal fault
wearing an authorisation costume, and it will not look like one in a dashboard of denials. A rising
`authority_not_established` is usually a policy or deployment problem instead: a stale revision, an
operation outside the reviewed catalogue, or a declared argument callers are not sending.

**`would-hold` is counted as itself, never folded into `proceed`.** Under the `observe` profile the
action *does* go ahead, and this count is the number you watch before stepping the ladder up. Folding
it in would erase the only signal the observe rung produces. See
[control-profiles.md](control-profiles.md).

**`buffered` is not a degraded `on_disk`.** Under the default `Async` sync mode a receipt honestly
reports `buffered`: survives a process crash, **lost to a power failure**. Graph the split and decide
deliberately whether that proportion is what you intended.
See [deployment.md §Choosing a sync mode](deployment.md).

**Still not exposed:** evidence freshness and export lag. Those measure how far an exporter has
fallen behind the evidence journal, and there is no exporter in the public tree to be behind — the
gauge would measure nothing. It lands with the exporter, not before it.

## The Ops Console

`examples/ops_console` is **a static page** you open in a browser and point at the nodes you want to
watch. It reads each node's **existing routes directly**, fans out across them itself and merges their
answers on the page — there is no console server, no proxy, and nothing that polls the fleet and holds
the result (decisions D16, D28: a panel that would need such a process becomes a route on a node
instead). Tabs: **Overview · Fleet · Diagnose · Guarantees · Governors · Tuples · Integrity · Audit · KV ·
Metrics**, each rendered from the routes its [README](../../examples/ops_console/README.md#what-it-reads)
names — the table CI renders, tab by tab, in a real browser against a three-node fleet
(`scripts/ops-console-render.sh`, the *Ops Console* job).

```bash
cargo run --example observed_fleet --features metrics          # three nodes, gateways :9061–:9063
python3 -m http.server 8099 --bind 127.0.0.1 --directory examples/ops_console
# open http://127.0.0.1:8099/?nodes=127.0.0.1:9061,127.0.0.1:9062,127.0.0.1:9063
```

### Cross-origin reads: `gateway_cors_origins`

A browser shows a page the answers of another origin only if that origin says it may. A gateway says
so only when its operator names the page's origin:

```toml
gateway_cors_origins = ["http://127.0.0.1:8099"]     # or GOSSIP_GATEWAY_CORS_ORIGINS=http://127.0.0.1:8099
```

- **Off by default.** With the list empty no response carries `Access-Control-Allow-Origin`, so a page on
  another origin reads nothing — the gateway's behaviour is unchanged.
- **Exact origins only.** `scheme://host[:port]` as a browser sends it — lowercase, no path, no trailing
  slash, no default port (`:80` for `http`, `:443` for `https`). `*`, `null` (the origin of every sandboxed
  frame and `file:` page) and anything else are refused by `validate()` naming `gateway_cors_origins`, as is a
  non-empty list on a node with no `http_port`. A listed origin is answered with itself, never `*`, with
  `Vary: Origin`.
- **Read-only routes only, `GET`/`HEAD` only:** `/health`, `/ready`, `/stats`, `/metrics`, `/gateway/fleet`,
  `/gateway/diagnose`, `/gateway/guarantees`, `/gateway/govern`, `/gateway/kv/keys` (key **names**),
  `/gateway/kv` **for one key under `ui/` only** (the `ui/viz` link; any other key, or two `key` parameters,
  is not answered), `/gateway/audit` and `/gateway/tuple/overview` (where mounted) — `CORS_READ_PATHS` and
  `admits_path` in `src/agent/http/cors.rs`. No write, call, stream or governance route is ever answered
  across origins, and a preflight asking for any other method is not answered. What each listed route
  discloses is [rbac.md § Cross-origin reads](rbac.md#cross-origin-reads-what-a-listed-page-can-see).
- **With a credential, the bearer decides.** A preflight for a listed origin is answered before the gateway's
  auth layer (a browser sends no credential on one) and admits the `Authorization` header; the read itself
  passes `gateway_auth` and its scopes like any other request, so on a gateway with a credential model the
  setting widens *which pages may read in a browser*, not *what a caller may read*. **Without one it does
  widen it:** an open gateway answers anyone who can reach it, and a listed page's script runs in the
  operator's browser, which can. So a gateway with no credential model may list **only loopback origins**
  (`http://127.0.0.1…`, `http://localhost…`, `http://[::1]…`, pages on this host); `start()` refuses any
  other by name (`gateway_cors_origins`). A page's JSON or bearer-carrying write needs a preflight, and no
  write route answers one; a request a browser sends without a preflight (a form post) reaches a gateway
  whether or not this is set, as it reaches any server — the setting changes nothing there, and a gateway
  with a credential model answers it 401.
- **Not handled: Private Network Access.** Chrome's preflight for a public page reaching a private address
  (`Access-Control-Request-Private-Network`) is not answered, so such a read is refused by a browser that
  enforces it. Serve the page from an origin on the same network as the nodes.
- `doctor` lists the setting with its source like every field (`docs/reference/configuration.md`).
- **The page's bearer.** Typed into the page, it is held in the page only and sent only to the nodes the
  operator added in the box (or the page's default). A node a link's `?nodes=` named gets it only once the
  operator confirms the list — or while the bearer in use is the very one the same link carried as
  `#token=…` (its author knew it); decided at each request, so a bearer typed over the link's goes to the
  link's nodes only when confirmed, and an edit trusts only the nodes it added. A fragment never
  reaches a server, but it stays in the browser's history like the rest of the URL. A node entry is
  `host:port` or `http(s)://host[:port]`: a path, query, fragment or credentials in it are refused, and its
  panel says so. A `ui/viz` link a node advertises is made a link only if it is `http:` or `https:`.

### The Integrity tab

The **Integrity** tab is the one to know about among the panels, because the coordination and identity properties are
**refusals** — the node declining to decide alone, to trust transitively, or to accept an identity
it cannot authenticate. A refusal has no dashboard of its own: *you cannot watch a thing not
happen.* So the tab shows the **preconditions**, before a refusal surprises someone:

| Panel | The question | Composed from |
|---|---|---|
| Electorates | would an election here work, or be refused? | `/gateway/kv/keys?prefix=grp/` |
| Sealed identity coverage | does every node publish the sealed record `require_identity_proofs` (on by default) accepts? | `/gateway/kv/keys?prefix=sys/identity` |
| Role concentration (P10) | is one node quietly holding everything? | `/stats` |
| Tripwires | what has been refused or flagged? | `/stats` |

Every panel composes endpoints listed above — **nothing new is emitted for it**. And one panel names
what deliberately has *no* panel: trust non-transitivity and leadership fencing are refusals at a
boundary rather than fleet state, so a tile reading "no breaches" would assert far more than one
node can see.

Nodes advertise their own dashboards at `ui/viz` + `ui/label`, so the console discovers and links
them; each showcase links back with a **⚙ Ops Console** button.

## Logs & tracing

Mycelium uses `tracing`. The `mycelium` and `mycelium-stem` binaries log at INFO by default and read `RUST_LOG`;
a valid `RUST_LOG` **replaces** that default rather than adding to it, so every target it does not name is off. A
directive matches targets by prefix, so `RUST_LOG=mycelium=debug` covers `mycelium_core` and `mycelium_wasm_host` too
and hides only the dependencies' lines; `RUST_LOG=mycelium_core=debug` hides the node's own `mycelium` INFO lines; and
`RUST_LOG=inf` (a typo that parses as a target) silences everything. Name the default too:
`RUST_LOG=info,mycelium_core=debug`. A `RUST_LOG` with no valid
directive keeps the default. `skillrunner` pins `skillrunner=info` and `mycelium=warn` over `RUST_LOG`. Build with
`--features otel` and configure `[skill.otel]` for Jaeger/Grafana spans per
invocation. Failure paths log with actionable context — the tripwire warnings
above, `SignedData from unknown signer`, `Individual-scoped frame dropped`, etc.,
each name the cause (and several are [patterns-chapter](../guide/14-patterns-and-pitfalls.md)
entries).

**`Signal handler channel full; signal dropped`** (with the `kind`) means a subscriber is not draining —
most often an SDK or HTTP client on `/gateway/signal/sse/{kind}` or `/signals/{kind}`, whose subscription
buffers 256 signals. The signal is dropped for that subscriber and **the client is not told**. Signals
are best-effort by design: consume faster, or move work that must not be lost to an ordered log
(`subscribe_log`) or a mailbox. An in-process handler can take a larger buffer
(`signal_rx_with_capacity`).
