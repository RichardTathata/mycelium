# Deprecations — what is on the way out, and what to do about it

**Nothing on this page is removed on the 2.x line.** Every entry is a *replacement added beside the
old thing*, with removal deferred to the `3.0.0` removal ledger. The ledger exists so that a major
release, if one ever happens, removes only things that were announced — and the announcement is this
page, on the day of deprecation rather than the day of removal.

The authoritative ledger is [`docs/plans/v3-contracts-axis.md`](../plans/v3-contracts-axis.md) §6.6.
This page is its adopter-facing half: what to change, and whether the compiler will tell you.

## The one compatibility rule

The substrate ships compatible additions on 2.x, by Rust's definition: **a public field or type never
changes shape**. When a verb's answer gains meaning, the new representation is added *beside* the old
one and the old one goes on this page. Read the entry, adopt the replacement, keep compiling.

## Will the compiler tell me?

Honest answer: **usually not**. Only two entries carry `#[deprecated]`, so for the rest this page is
the notice.

| # | What | Since | Replacement | Compiler warns? |
|---|---|---|---|---|
| 1 | `system_propose` | 2.1.0 | `cluster_propose` | **Yes** — `#[deprecated]` |
| 2 | `cluster_name` as an isolation boundary | — | `DomainId` (item 2) | No — it stays as a display label |
| 3 | the inferred `>=` ack in `set_with_min_acks` | — | exact-identity (`content_hash`) ack | **Yes** — `#[deprecated]` *(resolved 2026-09-15)* |
| 4 | `ConsensusResult::Committed { persisted: bool }` | 2.4.2 | `cluster_propose_receipt` / `group_propose_receipt` | No |
| 5 | node-as-caller gateway dispatch (`GatewayCallerProfile::Legacy`) | 2.5.0 | `GatewayCaller` (item 7) | No — a `warn!` at startup |
| 6 | `BoardConfig` not `#[non_exhaustive]` | 2.8.0 | `..Default::default()` | No — it breaks at 3.0.0 |
| 7 | `GossipConfig` (and sibling config structs) not `#[non_exhaustive]` | — | `..Default::default()` | No — it breaks at 3.0.0 |
| 8 | exhaustive `match` on `RecordKind` | 2.10.0 | add a `_` arm | **Yes** — `#[non_exhaustive]` from 2.10.0 |
| 9 | exhaustive `match` on `Execution` | 2.10.0 | add a `_` arm that reads as **unknown**, not as *nothing ran* | **Yes** — `#[non_exhaustive]` from 2.10.0 |
| 10 | `FederationEdge::authorize(presented, export, now_ms)` | 2.12.0 | `authorize(presented, export, **body**, now_ms)` | **Yes** — it will not compile |

**Entry 10 is the loud kind**, and deliberately so: a signature change, caught by the compiler, not
a behaviour change to discover at runtime. You cannot authorise a federated call without saying
which body it is — see §10 below.

Entries 6 and 7 are the inverse of the usual case: nothing is deprecated *today*, but a future
`#[non_exhaustive]` will break one specific pattern, so the pattern is worth abandoning now.
**Entry 8 is that same case already done**: `RecordKind` was marked `#[non_exhaustive]` in 2.10.0
*before* AE0 §5's remaining record kinds arrive, so their arrival costs nothing. Add the `_` arm
once; that is the whole of it.

---

## 1. `system_propose` → `cluster_propose`

Renamed 2026-07-10 so the consensus verb matches its scope (`SignalScope::Cluster`). The old name is
a `#[deprecated]` alias with identical behaviour; the gateway still accepts `"system"` on the wire.

```rust
// before
agent.consensus().system_propose(slot, value, config).await;
// after — same arguments, same result
agent.consensus().cluster_propose(slot, value, config).await;
```

**Unrelated, and a common confusion:** `system_stats()` is *not* this. It reports node-local runtime
state and is not a scope.

## 2. `cluster_name` is a label, not a boundary

`cluster_name` never provided isolation. A Mycelium cluster is emergent from network reachability —
peer exchange plus CA admission — so two nodes with *different* `cluster_name`s that can reach each
other and pass admission will gossip, and two with the *same* name that cannot will not.

If you were relying on `cluster_name` to keep deployments apart, that separation does not exist. The
mechanism that does is a **federated domain** ([chapter 17](17-federation.md)): a `DomainId`, a trust
bundle, and per-partner authorisation — with the invariant that federation never joins the
transports.

```rust
// not a boundary: a display label that happens to differ
cfg.cluster_name = Some("prod".into());
// the boundary: separate meshes, explicit trust between them
let domain = DomainId::new("prod.example")?;
```

`cluster_name` keeps working as a display name. What is deprecated is **reading isolation into it**.

## 3. The inferred `>=` acknowledgement — resolved

`set_with_min_acks` used to count acknowledgements by inference. It now matches on exact identity
(`content_hash`), so an ack names the write it is for. **Resolved 2026-09-15**: no migration flag was
needed, because the success path a flag would have preserved turned out to be unreachable. See
[chapter 18](18-contracts-and-receipts.md) and `docs/design/contracts-receipts.md` §1a.

## 4. `Committed { persisted: bool }` → the receipt verbs

A `bool` cannot distinguish *"the bytes are on disk"* from *"the write was queued and nothing is
promised"*, and the field's own documentation has to spend a paragraph saying so. Ask for a receipt
instead, which names its rung and nothing above it.

```rust
// before — one bit, and `false` does NOT mean "absent from the WAL"
if let ConsensusResult::Committed { persisted, .. } = result {
    if persisted { /* ... */ }
}
// after — a typed rung
let receipt = agent.consensus().cluster_propose_receipt(slot, value, config).await;
```

The field still works and still means what it documents. Two things to keep in mind while you have
it: add `..` to any exhaustive `Committed` destructure (it gained fields in 2.4.2), and never read
`false` as "absent from the WAL" — it means durability was *not established*, which is a different
claim. [Chapter 18](18-contracts-and-receipts.md) is the full ladder.

**Why the compiler does not warn here, costed rather than asserted** *(2026-09-22)*. It *could*:
Rust does support `#[deprecated]` on a struct-variant field, and it warns on both construction and
destructuring — checked, not assumed, because the first draft of this note claimed the opposite and
was wrong.

The cost is that the field is **still produced by this crate**, in about fifty places across ten
files. Marking it would mean an `#[allow(deprecated)]` beside each one — `make check` runs
`-D warnings` — so the substrate would be suppressing a warning about itself, permanently, and every
future internal use would need the same. That is a lot of noise to buy a nudge for the one pattern
already spelled out above.

So it is left unmarked **deliberately**, and this paragraph is the reason. It is a judgement about
signal-to-noise, not an oversight, and it is worth revisiting if the internal uses shrink — the
day this crate stops producing the field is the day marking it costs nothing.

## 5. Node-as-caller gateway dispatch → `GatewayCaller`

Before item 7, every gateway-originated dispatch ran under the **node's** identity, so a provider's
`authorized_callers` saw the gateway node and never the HTTP client — a confused deputy. A provider
now receives a verified `GatewayCaller`: the originating principal, the gateway acting for it, and
the scopes granted for that request.

The old behaviour survives only under an explicit profile, for a rolling-upgrade window:

```toml
# deprecated: dispatch as the node. Logged at warn! on every start.
gateway_caller_profile = "legacy"
```

**Migration:** upgrade providers first (each writes `sys/caller-context/{self}` when it enforces),
then drop the `legacy` profile. A secure-profile gateway refuses to dispatch to a provider that has
not published the marker, rather than silently falling back. Note that `authorized_callers` now
judges the **client principal**, not the gateway node — so an entry naming a node id stops matching,
which is the point. Operational detail: [`operations/rbac.md`](../operations/rbac.md).

## 6 & 7. Exhaustive struct literals over config structs

`GossipConfig`, `BoardConfig` and the operator-constructed config structs beside them are **not**
`#[non_exhaustive]` today. Every release adds config fields, and each addition silently breaks an
exhaustive struct literal — 2.5.0, 2.7.0 and 2.8.0 each did. Marking them `#[non_exhaustive]` at
`3.0.0` trades that series of unannounced breaks for one announced one.

```rust
// breaks on any release that adds a field
let cfg = GossipConfig { bind_port: 7000, bootstrap_peers: peers, /* ...every field... */ };
// unaffected, now and after 3.0.0
let mut cfg = GossipConfig::default();
cfg.bind_port = 7000;
cfg.bootstrap_peers = peers;
```

The `Default` + assignment pattern is the supported one. Same for `BoardConfig` and `..Default::default()`.

---

## What this page does not cover

Wire-format compatibility, which is versioned separately (`mycelium-core/src/framing.rs`) and
described under [rolling upgrades](../operations/deployment.md#rolling-upgrades). Companion crates
version on their own lines and carry their own notes.

## 10. `FederationEdge::authorize` takes the request body

The credential's signature covered the origin domain, the principal, the export and the validity
window — **and nothing about the payload**. An attacker on the path between two domains could
rewrite a call's body, leave the credential header untouched, and the receiving gateway would accept
the altered call as authentic, then run the authorisation preflight and record an evidence decision
about the attacker's text.

```rust
// before — authorises a caller and an export, and nothing about what was asked
edge.authorize(&presented, skill_id, now_ms)?;
// after — the bytes as they arrived, before parsing
edge.authorize(&presented, skill_id, &raw_body, now_ms)?;
```

**Why a break rather than a second method.** An `authorize_with_body` beside the old one would have
left the *insecure* call still compiling, still public, and still the shorter name. The point of the
change is that you cannot authorise a federated call without saying which body it is, and only the
signature can enforce that.

**Pass the bytes as received**, not a re-serialisation: two encodings of the same JSON are the same
request and different bytes, so digesting a parsed-then-re-encoded body compares your serialiser
against your partner's. `POST /a2a` reads the body as `Bytes` and parses afterwards for exactly this
reason.

**You also choose when to require it.** `CallPolicy::require_body_binding` defaults to `false` so a
partner that predates the binding keeps working — and that default gives **no integrity guarantee
against an active attacker**, because an absent binding is indistinguishable from a stripped one.
Turn it on once every partner has upgraded. `BodyNotBound` means upgrade the partner;
`BodyMismatch` means someone is on the path.
