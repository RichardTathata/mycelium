# Deploying Mycelium

**Mycelium is a library, not a platform.** There is no daemon, control plane,
installer, or orchestrator to deploy. You embed the library in your process and
call `GossipAgent::start()`; that process *is* a full mesh node. "Deploying
Mycelium" means deploying *your* binary that embeds it.

> Audience: **DevOps**. The developer-side "how do I embed it" is in the
> [cookbook](../guide/cookbook.md).

## The minimum

```rust
let node = NodeId::new("0.0.0.0", 7946)?;
let mut cfg = GossipConfig::default();
cfg.bind_port = 7946;                                  // gossip (TCP)
cfg.bootstrap_peers = vec![NodeId::new("seed.internal", 7946)?];
let agent = Arc::new(GossipAgent::new(node, cfg));
agent.start().await?;
```

A node needs: a **gossip port** and at least one **bootstrap peer** to find the
mesh (a seed needs none). Everything else is optional.

## Ports

| Config | Purpose | Default |
|---|---|---|
| `bind_port` | gossip transport (TCP, and SWIM UDP if enabled) — node-to-node | required |
| `http_port` | the embedded gateway (diagnostics, AgentFacts, `/gateway/*`) | `None` (off) |
| `http_addr` | interface the gateway binds | `127.0.0.1` |

`http_port` must differ from `bind_port`. Leave `http_port = None` for a
headless node; set it (and `http_addr = "0.0.0.0"`) to expose the gateway. See
[observability.md](observability.md) for what the gateway serves.

## Seeds & bootstrapping

There is no special "master." A **seed** is just a node others list in their
`bootstrap_peers`; it has no extra role and can fail without electing a
replacement (peers re-bootstrap off any reachable member). Run 2–3 seeds for
redundancy and point everyone at all of them. Topology, sizing, and partition
recovery: [guide 13 · Cluster topology](../guide/13-cluster-topology.md).

## TLS / identity (recommended)

```rust
cfg.tls = Some(TlsConfig { auto_cert_dir: "/var/lib/mycelium/tls".into(), ..Default::default() });
```

With `tls` set, the node generates an Ed25519 identity + a CA-signed cert into
`auto_cert_dir` on first start, and all gossip is mTLS. **Every node in one
cluster must trust the same CA** — share the CA cert (`{auto_cert_dir}/ca-cert.pem`)
across nodes, or point them at a shared `auto_cert_dir`. The same key is the
node's identity for signed KV, consensus, audit, and AgentFacts. Rotation
without disruption: [cert-rotation.md](cert-rotation.md). Compliance features
(RBAC, audit, OIDC) build on this: [rbac.md](rbac.md), [audit.md](audit.md),
[sso.md](sso.md).

## Naming the cluster

Set `GOSSIP_CLUSTER_NAME` (or `GossipConfig::cluster_name`) to label the
environment — `prod-eu`, `staging`, … It is a pure operator label (no effect on
gossip, identity, or membership) that flows to `/stats`, the `/metrics` `cluster`
label, and AgentFacts, so one monitoring stack can tell environments apart. See
[observability.md](observability.md#naming-environments--monitoring-many-clusters).

## Containers / Compose

No special base image — it's your Rust binary. Expose `bind_port` (and
`http_port` if used), give each node a stable address its peers can reach, and
mount a volume for `auto_cert_dir` (so identity survives restarts) and for the
WAL/persistence path if enabled. Reference multi-node setups:
[`examples/community`](../../examples/community/) (skillrunner cluster) and the
`tests/integration/docker-compose.*.yml` files.

## Cloud / Kubernetes / bare metal

Mycelium ships **no Helm chart** and no opinionated, hardened product packaging — and that
is deliberate: a node is just a binary/container, and packaging would only track cloud churn
while every org's topology differs. It *does* ship **reference deployment scaffolding** to
copy and adapt: Kubernetes manifests —
[`deploy/kubernetes/`](../../deploy/kubernetes/) (`kubectl apply -k deploy/kubernetes`): a
seed StatefulSet + headless Service, a scalable worker StatefulSet, and a mgmt dashboard,
wired for the *topology* this section describes — **as an ephemeral demonstration**: no PVCs,
no TLS, no gateway auth, a demo image (its README lists exactly what it leaves out) — and
Terraform for the cluster itself —
[`deploy/terraform/`](../../deploy/terraform/) (EKS + ECR, or GKE + Artifact Registry), so
the whole path is `terraform apply` → push image → `kubectl apply -k`. Deploy it (or your
own) like any **stateful** service, minding two requirements that follow from the design:

1. **Stable network identity.** A node's `node_id` is its `host:port`; peers
   bootstrap to it by address. Each node needs an address that survives a restart
   (a static IP, a DNS name, or a Kubernetes *headless Service* + StatefulSet pod
   DNS like `node-0.gossip.svc`). Don't put gossip behind a round-robin load
   balancer — peers must reach *specific* nodes.
2. **Persistent identity + WAL.** Mount a durable volume for `auto_cert_dir` (so
   the Ed25519 identity survives restarts) and, if persistence is on, the WAL path
   (so state replays). On k8s that's a `volumeClaimTemplates` PVC per pod.

**Kubernetes:** a **StatefulSet** (stable pod identity + per-pod PVC) behind a
**headless Service** (stable per-pod DNS for bootstrap) is the natural fit; set
`GOSSIP_BOOTSTRAP_PEERS` (or the demo image's `MYCELIUM_PEERS`) to a seed pod's DNS
name, `readinessProbe` → `/ready`, `livenessProbe` → `/health`, and
`GOSSIP_CLUSTER_NAME` to the environment. Scrape `/metrics` with a `ServiceMonitor`.
The manifests in [`deploy/kubernetes/`](../../deploy/kubernetes/) give you the StatefulSet,
headless Service, probes and scrape annotations — **and stop there**: they mount no volume,
gossip without TLS, and leave the gateway unauthenticated, so they are the demonstration
profile, not this section's persistent one. Start from them and add the two requirements
above plus [production-readiness.md](production-readiness.md)'s gates; and see
[`deploy/terraform/`](../../deploy/terraform/) to provision the EKS/GKE cluster they run on. **AWS/GCP/bare metal:** an instance/ECS-task
per node with a stable address (Elastic IP / internal DNS) + a durable disk (EBS /
PD) for `auto_cert_dir` + WAL; a sample systemd unit is just `ExecStart=mycelium`
with the `GOSSIP_*` env in the unit's `Environment=`. Elastic membership (add/remove
nodes) is then driven by [dynamic-scaling.md](dynamic-scaling.md) — the governors
self-heal the count; the substrate needs no orchestrator hook.

## Sizing & tuning

Most defaults auto-derive from cluster size (WS-C). For large clusters or
constrained nodes, [tuning.md](tuning.md) covers `gossip_shards`,
`writer_channel_depth`, health/anti-entropy intervals, and
`GOSSIP_MAX_ACTIVE_CONNECTIONS` (the partial-mesh cap that avoids the O(N²)
connection ceiling). These can be set live — see [dynamic-scaling.md](dynamic-scaling.md).

## Restart behaviour

A restarted node re-bootstraps, re-learns the full KV state via anti-entropy,
and re-advertises its capabilities — there is no rejoin ceremony. With a
persisted `auto_cert_dir` it keeps its identity; with persistence enabled it
replays its WAL. Capability advertisements evaporate while a node is down and
reappear on restart (see [00 · Concepts](../guide/00-concepts.md) on evaporation).

### Persistence modes

`PersistenceConfig` (`base_path` · `sync_mode` · `snapshot_wal_threshold`, default 10 000 ·
`snapshot_interval_secs`, default 300) writes `{base_path}/{node_id}/kv/{wal.bin,snapshot.bin}`.
What a write acknowledgement means is the one operator decision:

| `sync_mode` | Meaning of an acked write | Cost |
|---|---|---|
| `Flush` | the write returns after the record's `fdatasync`; a stopped WAL writer or disk error is **logged at `warn`** — plain `set`/`set_async` do not surface it (their `bool` is the gossip-queue result). Consensus commits and leases *do* surface it via `persisted`, and **since v2.5.0 any write can ask for a receipt** — see below | ~1 ms/write on SSD |
| `Async` (default) | OS-buffered; the last few writes can be lost on power failure | none |
| `Os` | no explicit sync — development only | none |

In every mode: **consensus committed slots and leases are fsynced** (`append_sync` forces it) and
the commit result carries `persisted` (gateway JSON `"persisted"`; `false` = committed
cluster-wide but not on this node's disk — logged at `error`, repaired from peers by anti-entropy
after a restart; treat a run of `false` as a disk or writer fault on that node). Since v2.8.0 the
JSON also carries `"local_durability"` (`on_disk` · `buffered` · `not_configured` · `failed`, with
`"local_durability_error"` for the last): `"persisted": true` on a node **without** persistence
means *nothing was promised*, and only `"on_disk"` says the slot is on that node's disk. A snapshot merges
the on-disk WAL tail before truncating, so it never discards a record, and the snapshot rename is
fsynced at the directory before the WAL is truncated, so a power loss cannot leave an old snapshot
beside an emptied WAL; replay is last-writer-wins over every record. **Storage assumptions:** directory
fsync honoured (ext4, XFS, btrfs — the deployment targets); on **macOS** `fsync` does not force the
drive's write cache (`F_FULLFSYNC` would), so a laptop's power-loss durability is weaker than Linux's for
every sync in the WAL, not only this one — development only. Tune `snapshot_interval_secs` / `snapshot_wal_threshold` so replay time is
bounded; the snapshot pass raises the node's opacity for its duration. Since v2.4.2
(`CHANGELOG § [2.4.2]`); the invariants are canon in `mycelium-core/src/persistence.rs`.

### Choosing a sync mode with the receipt contract in hand

Since v2.5.0 the sync mode is no longer the *only* lever, because a caller can ask per write what it
actually got. That changes the operator decision: you are choosing the **default** each write reports,
not the ceiling.

| A write made with | Reports | Costs you |
|---|---|---|
| `set` | a bool meaning *queued for gossip*. **`false` is ambiguous** — the local store may still have been updated | nothing |
| `set_with_receipt` | whichever rung was actually reached under this node's `sync_mode` | one receipt allocation |
| `set_requiring_sync` | `OnDisk`, or it **refuses** | an `fdatasync` per write, whatever the mode |

The middle row is where `Async` bites. Under `Async` a receipt honestly reports **`Buffered`**: the
record survives a process crash and is **lost to a power failure** until the next sync or snapshot.
That is a different claim from `OnDisk`, not a weaker one, and a caller that needs the stronger claim
has to ask.

**`set_requiring_sync` is the one place the substrate prevents rather than detects.** When durability
cannot be established it applies nothing and gossips nothing, so no reader, subscriber or peer sees
the value from that call. Read the limit precisely: it does *not* promise the value can never appear
here, because the log writes before it syncs and a later replay may restore bytes from a failed sync.

**Operator consequence.** You do not need `Flush` cluster-wide to get durable writes for the few
operations that need them. Leave the default and let those callers use `set_requiring_sync`; reach for
`Flush` when *most* writes need the guarantee and you would rather pay it once in configuration.

A timeout on any of these is `DeliveryUnknown`, never a failure. Do not page on it as an error and do
not retry an at-most-once operation on the strength of it. Full contract:
[guide 18 · Contracts & receipts](../guide/18-contracts-and-receipts.md).

## Rolling upgrades

Mycelium tolerates a **mixed-version cluster** during an upgrade: `read_frame` accepts both the
current and previous wire version (`WIRE_VERSION` / `PREV_WIRE_VERSION`, currently **12 / 11** in
`mycelium-core/src/framing.rs`). There is no cluster-wide upgrade command — it's a library, so rolling a fleet is
your orchestrator's job (restart the processes); Mycelium's contract is that the mixed window is
safe. The procedure:

1. **One wire step per rollout.** Nodes one wire version apart interoperate; **two apart do not**. If
   a release bumps `WIRE_VERSION`, get the whole fleet onto it before the next bump. Most releases
   don't touch the wire — when `PREV_WIRE_VERSION == WIRE_VERSION` there is no live window and any
   order is safe.
2. **Upgrade node-by-node.** Replace one node's binary and restart it; it re-bootstraps and re-learns
   state via anti-entropy (the [restart](#restart-behaviour) path). The window is safe because a
   new-version node re-encodes forwarded frames at `WIRE_VERSION`, so the cluster converges toward
   the new format as you progress.
3. **Verify before the next node.** `GET /stats` on the just-upgraded node — `dropped_frames` steady
   (not climbing) and it is hearing peers. A climbing `dropped_frames` or an `UnsupportedWireVersion`
   in logs means a **two-step** gap: stop and reconcile before continuing.

The back-compat this relies on is regression-gated — the wire corpus + `decode_wire_v11_*` gate
(see the [testing page](../wiki/dev/testing/testing.md)) — so a release that would break the mixed
window fails CI, not your rollout.

**Client-facing changes that are not wire changes** still need a plan. Since 2.14.0 a
`POST /gateway/kv` without `value_b64` answers **400 and writes nothing** (it used to store an empty
value); an HTTP client or an old SDK that relied on the silent default breaks at upgrade, not at the
wire. The list is [deprecations.md](../guide/deprecations.md) (§12 for this one).

## Backup & restore

Persistence is a **WAL + periodic snapshot** in the node's data directory, and identity is
the Ed25519 key/cert under `auto_cert_dir`. Both are plain on-disk state — but *"plain on-disk
state"* is not *"copy it whenever"*, and an earlier version of this section said it was.

**Do not copy the directories while the node runs.** The snapshot writer merges the WAL tail into
a new snapshot, renames it into place, and *then* truncates the WAL — each step durable on its
own. A copier that reads `snapshot.bin` before the rename and `wal.bin` after the truncation gets
the **old** snapshot and an **empty** tail: every record that was in the tail is in neither file
it took. The WAL makes a *single* file consistent under a torn write; it does not make two files
consistent with each other across a compaction. (External readiness review, 2026-09-26.)

**Supported backup, pick one:**

1. **Quiesced copy.** Stop the node (or `SIGTERM` and wait for `/health` to go away), copy the
   persistence dir and `auto_cert_dir`, start it. The node re-learns anything it missed from
   peers via anti-entropy on rejoin, so a quiesced node costs nothing but its own absence.
2. **Filesystem snapshot** covering *both* directories in one atomic point in time — LVM, ZFS,
   an EBS/PD volume snapshot of the one volume that holds both. A volume snapshot of two volumes
   is two points in time, and is the live-copy problem again.

**Back up together, or not at all.** A restore that brings back some of this state at an older
point than the rest is the failure the review named *"put the directories back"*. The set, per
node, and what an older copy of each does on its own:

| State | Where | An older copy alone… |
|---|---|---|
| KV WAL + snapshot | persistence dir `kv/` | re-learned from peers; safe |
| Node identity | `auto_cert_dir` | a *different* node to the mesh if the key changed; if an operator removed the old identity, its removal record still stands |
| Revocation epochs, mandate floors | the `DurableEpochs` journal (when attached) | **resurrects revoked authority** — never restore this older than the identity |
| Evidence journal | the `with_evidence_journal` path | records the consumer already has re-send under the same ids (safe); records after the copy are **gone**, and the exporter's cursor must be reset with it |
| Companion stores (tuple-space WAL, blackboard, wiki store) | each companion's own dir | [companions.md](companions.md) — each has its own replay rule |
| A private companion's outbox | its dir | sequences reissue under a stale copy unless the outbox's *generation* is bumped after the restore — the companion documents the call |

**Restore** = stop the node, put **all** of the above back from the **same** point in time, and
start it. On boot it loads the latest snapshot and replays the WAL tail on top (last-writer-wins
per key, every record), re-bootstraps, and re-learns any newer KV from peers via anti-entropy.
Then verify: `/ready` answers, `/stats` shows it hearing peers, and — if an evaluator or authority
is attached — a **revoked** mandate is still refused (the check that a restore did not resurrect
authority; `examples/authority_drain` is the shape of it).

**What is tested.** The WAL/snapshot replay path has golden fixtures replayed in CI
(`tests/fixtures/persistence/`). The two backup procedures above are **not** exercised by CI — they
are a filesystem and an operator, and this page says so rather than implying a green suite covers
them.
