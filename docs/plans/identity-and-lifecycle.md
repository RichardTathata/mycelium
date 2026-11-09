# Identity and lifecycle — keyed principals, signed decisions, cancellation under a per-call mandate, and the developer surface

**Status:** **agreed with the maintainer on 2026-11-07**, rev 0.10 (rows I2 and I4 are **not yet buildable**: I2's **R1–R4 open**, the round-3 review's questions, and I4's **N1–N5 open**, the round-2 review's questions, which each row's build PR must settle; rev 0.10 writes row I4 from spike S-cancel's note, `docs/plans/spikes/2026-11-07-s-cancel.md`, and takes #731's review and its confirm round: S-cancel re-measured, its reclamation fixed for the feeder, and I4's exit given an item for every D2 decision; rev 0.9 writes row I2 from spike S-succeed's note, `spikes/2026-11-07-s-succeed.md`, and #732's review: a `cert_pem` node re-roots live, the trust set and the successor certificate persist under `succession/`, every cached judgment is re-judged when the roots change, and old-CA records lose standing (decided in the confirm round); rev 0.8 records the agreement, its text rev 0.7's); proposed 2026-10-31. Work starts in §4's order: wave 1a's I0 and wave 1b's spikes in parallel. Five independent adversarial reviews —
four on one day, the fifth on the merged document (#715's thread, 2026-11-05; the thread, this paragraph and the *Revised
from* column are the record of all five): rev 0.1 against the decisions, rev 0.3 against the mechanisms, rev 0.4 and rev 0.5 each against five
premises, both with the verdict *not yet buildable*. Every premise failure sat in wave 1 (I0–I4) or in the developer
identity (D7); none in I3, I5, N3, N5, N6 — the reviewer named those buildable. **Rev 0.6 is a split, not a rewrite
(D8):** wave 1a holds the rows a plan can close — I0 narrowed to this node's own unit, I1 narrowed to service principals
holding an Ed25519 key, I3, I5 — and wave 1b holds three **spikes** whose code, not this document, settles the three
premises the reviews could not: a `call:` mandate carried with its first checkpoint (I4), a certificate reload without a
restart (I2), and an OS-presence signer from a signed bundle (the developer identity, now its own row I7). Wave 1b's
rows are written *after* their spike, from what ran. Every stated fact the fourth review contradicted is corrected in
place. **Rev 0.7** takes the fifth review: `cn` carries a per-process instance id, without which two processes under one
certificate still collide (H1); `identity:write` exists and is `compliance`-gated (M1); the `sys/` tripwire counts only the
families it lists, so `sys/client-cert/` needs no exemption and its fold judges by signature and issuance, never by KV
time (M2); a provider without `compliance` refuses `ot` by name and the envelope's 8 KiB cap is stated (M3); the index entry
rewritten (M4); S-cancel's questions gain the gossip-latency bound and the unbounded drain classes (M5); Q5 un-cited — the
digest vectors exist, the signature vector is new (L1); `max_drain_secs`/`grant_retention_secs` marked new and
`cancellable`'s window restated as the 2.38 refusal (L2, L3); the shared `TrustedExternalIssuers` handle named as an API
change (L4); a client key's revocation given a home — supersession by re-issue (L5); E4 cites the thread (L6); paths
corrected (L7). Its confirming round found five more on the new text, taken in the same revision: the `cn` cache is a new
window type with a per-instance floor, since `ReplayWindow`'s `(u64, u64)` key cannot hold an instance and a counter
(N1); supersession keyed on the certificate's serial — its `issued_ms`, `notBefore` being fixed — as a high-water mark
kept in memory and the journal, so an overwrite or a restart under `[persistence]` cannot lower it (N2, with its journal budget stated, N6); `ot` refused as `not_in_build` and an
oversized envelope as the existing `caller_context_too_large` (N3); a clock step-back refused `stale`, `Replayed` only
under a floor (N4); the SDKs' `instance` drawn outside the seam (N5). D7's and N5's external facts are the spike's and E6's walk's to verify, not a review's. It comes out of a comparative reading of [Orca](https://github.com/stablyai/orca) (`981ad63b`, 2026-10-31) and
[Eve](https://github.com/vercel/eve) (`1.0.0-rc.1`, 2026-10-31) against `main` at `9cbd3b82` (v2.38.3 plus Y1–Y8 and
Y10–Y12 merged). Rows close on evidence (verification policy rule 2).

## 1 · Where this plan comes from

The adopter-surface plan ([`adopter-surface.md`](adopter-surface.md)) came from a 2026-10-23 comparison with Eve and
four SDKs and built only what did not already ship. This plan comes from the next reading, of two projects that sit at
different layers from Mycelium:

- **Orca** is an agent development environment — a desktop product where a developer runs coding agents, each in a
  worktree, with a coordinator/worker orchestration whose one home runtime holds one SQLite database and whose
  settlement authority for a federated dispatch is `run_home`. It authorises nothing itself: an agent's permission mode
  is chosen at launch and an ACP permission request is relayed to a human. It serves no MCP; it consumes MCP, a CLI
  with `--json`, and plugins with a declared capability set. Its stop report for a worker that ends without
  `worker_done` "never settles it" — the posture Mycelium states as *a timeout is `DeliveryUnknown`, never a negative*.
- **Eve** is an agent framework an application embeds — an agent is a directory of files, every conversation a durable
  workflow. Its approval model states three rules Mycelium holds independently: *approval is a gate, not
  authorization*; *approval doesn't make a repeated step safe — use idempotency keys*; *the approved call runs as the
  requester, never the approver*. Eve decides approval once, where the agent runs; Mycelium verifies a grant where the
  tool runs, at three doors. Eve has the surface Mycelium lacks — Slack and Teams approval cards; Mycelium has the
  verification Eve lacks.

**Eve is what an agent looks like on top of Mycelium; Orca is what a developer looks like in front of it.** Neither is
a dependency: both moved 60–300 commits in the five days of the reading, and D24's *pin, don't follow* stands.

What the reading exposed, and what already ships so it is not rebuilt (each *already ships* cell verified against the
code by the four reviews):

| Found | Already ships | Still missing |
|---|---|---|
| An organisation running interactive fleets needs a person to approve, stop and resume an agent's work | the grant (X3): routes, SSE stream, SDK verbs; the Ops Console (Y3), read-only; the authority-lapse sweep that stops running work registered under an **established** mandate within its drain bound (C9) | any surface that shows a pending grant to a person; a cancel that stops work (*not carried* on six rows of [`adapter-contract.md`](../reference/adapter-contract.md), *not applicable* on two); `admit_after` registers only established, mandate-bearing work, and a mandate is established by reading its issuer's checkpoint **from the KV** — an unknown standing is never read as established ([`gateway_authority.rs`](../../src/agent/gateway_authority.rs) `feed_grant_checkpoint`, [`authority.rs`](../../src/mandate/authority.rs) `RevocationStanding::Unknown`) |
| An SDK or Eve agent never runs *under* a grant | the grant, verified at three doors; possession verified for any principal whose key is in `TrustedExternalIssuers` | a grant *issued* to a client: `decide_grant` and `revoke_grant` refuse a `/` in the agent and every `token:` principal carries one (`token:{issuer}/{name}`, the issuer this gateway's node id unless `gateway_identity_issuer` is set); `revoke_grant` derives the evaluator's resource from a `node:` holder; `is_member_agent` parses a `NodeId`; every certificate reader is **Ed25519-only** (`ed25519_key_from_cert_der`, `cert_identity`, [`tls.rs`](../../mycelium-core/src/tls.rs)); `sys/` is refused on the raw KV routes, and the fifteen `sys/` families `SELF_OWNED_SYS_PREFIXES` lists ([`connection.rs`](../../mycelium-core/src/connection.rs)) are tied to a node-id segment (eight others — `sys/govern/`, `sys/membership/`, `sys/rate/`, `sys/quorum/`, `sys/health/`, `sys/config/`, `sys/capauthz/`, `sys/topology-override/` — are not, and a family the array omits is never counted), so **no door publishes a record for a principal that is not a node**, though any member's own `kv().set` can write one (detection, not prevention); `TrustedExternalIssuers` is an immutable value at three holders; **the published binary attaches neither an `ActionEvaluator`, an `ExecutionAuthority` nor an evidence journal** |
| A provider trusts the gateway's assertion of who called | the caller envelope, signed by the gateway's key naming the principal; row E's `sn`; `ReplayWindows` failing closed on eviction because envelope ids are ordered | the client's own signature over fields it can know, with a replay bound that fails closed and a canonical form both SDKs reproduce byte for byte |
| A coding agent needs the fleet's operations as tools | `doctor`, `rules`, `test --faults` and `wire-check` each with `--format json` and a `schema`; `explain --closure`'s `mycelium.closure/1`; `declare --watch`; three `init` templates; the pilot kit | `--format json` for `explain`, `declare`, `tls`; an MCP server over the node's operations; a local fleet per worktree; an agent guide the binary serves |

## 2 · Decisions

Each option was weighed for soundness, not for the size of the change (the maintainer's instruction, 2026-10-31). The
*revised* column records what a review found and what changed.

| Id | Decision (rev 0.7) | Why | Revised from |
|---|---|---|---|
| **D1** | **Every acting principal is keyed by a certificate the fleet CA issues off-node.** In wave 1a the holder is a **service** — a process holding an Ed25519 key (the Eve supervisor, an SDK agent): `mycelium tls issue --client <name>` issues a certificate whose **SAN** is `mycelium-client://<issuer>/<name>` *(new scheme)*; **both mTLS verifiers refuse a certificate carrying that scheme** (a pre-row-Q node certificate naming no URI stays admitted, as `refuse_if_revoked` deliberately does today); a **gateway door** *(new, I1)* publishes the record under `sys/client-cert/` with a stated tripwire exemption; `TrustedExternalIssuers` becomes a shared source at all three holders. `gateway_identity_issuer` is required for a client's **call** to be admitted (a start-time check is impossible: records arrive by gossip). A keyed principal may hold a grant and, where the evaluator names it, issue one. `oidc:` key-less in wave 1 (D6); `federation:` exempt by design. **The developer's key is not a fleet certificate in this plan** — it is I7's question, settled by spike. | the one trust path every provider already has; a gateway publishing a key re-opens rebinding. | rev 0.5's SAN refusal cut off every pre-row-Q node; its "start refusal" could not be decided at start; its developer key could not be verified by any Ed25519 reader. |
| **D1′** | **The envelope stays at v1 with optional `cs` and `cn`.** `cs` covers the operation, the resource, the arguments' digest over the **canonical form** — `serde_json`'s compact, key-sorted encoding, as `admit_after` digests today — — the digest already has a cross-language vector (`arguments_digest_golden_vector`, [`gateway_authority.rs`](../../src/agent/gateway_authority.rs); `a2a.py`'s `sort_keys` form; `a2a.ts`'s `canonicalJson`), and **one new vector pins the signature** — the client's `issued_at` and `cn`; verified before the nonce cache. `cn` is `(issued_at, instance, counter)`: `instance` a random 64-bit id the process draws once at start (through the seam), `counter` monotonic in the process; the Rust client draws `instance` through the seam, the SDKs from `os.urandom` / `crypto.randomBytes` outside the replay claim, and a `sim` replay takes a recorded envelope's `instance` from the recording. Two processes under one certificate share the principal and the key and no counter, so without `instance` their `(t, 5)` and `(t, 5)` are one key and the second is `Replayed` — the gateway's own `n` is a per-process HLC and never collides, which is what `instance` restores for a client. **The cache is a new window type** *(new)* keyed per **(principal, certificate key)** with entries `(issued_at, instance, counter)` and an **eviction-only floor per instance**: `ReplayWindow`'s own key is `(u64, u64)` fed from the gateway's `n`, and a lexicographic floor over `(t, instance, counter)` would order honest processes by their random id, so the existing window cannot carry `cn` — the new one keeps its *rule* (a delayed envelope inside the window is admitted; refused only past 65 536 later ones of the same instance or the freshness horizon), so a gateway reordering two honest envelopes trips nothing. Freshness runs on the client's own clock under the existing drift bound — stated; a client whose wall clock steps back is refused `stale` past the freshness horizon, and `Replayed` only while a floor raised by sustained eviction sits above the stepped-back time (no floor exists until an eviction, and `prune` clears it) — stated. | the floor *rule* exists and is proven at `ReplayWindows::admit`; the window for `cn` is new because the existing key shape cannot hold an instance and a counter without ordering by the random id; a last-seen floor would refuse reordering. | rev 0.5 read as a last-seen floor and keyed the cache by principal alone; rev 0.6 keyed the counter by certificate, which two processes under one certificate share (#715 H1), and cited Q5, which is about artifact signing (L1). |
| **D2** | **A cancellable call runs under a `call:`-scoped mandate the admitting gateway holds; cancel is its revocation — the row is written after the spike S-cancel.** What is decided: `call:` is a scope prefix; holder = `established_by` = the gateway; a revoked term is `Refused`; memory bounded; the issuer down ⇒ the term expires. What the spike settles: **how the provider establishes a mandate that outruns its checkpoint** — carried inside the envelope beside the mandate (as `PresentedMandate` carries grant + proof) or a bounded wait — and the latency each costs. | the one shape the provider can establish; the one premise a document cannot settle. | revs 0.1–0.5. |
| **D3** | **A decision's actor is the principal whose key signed it**; the signed bytes name the node the decision is addressed to; delivered elsewhere it is refused `not_addressed_here` *(new reason; `not_this_node` was a retired name on this route)*. The route admits **either** `grant:issue` on the bearer or `grant:relay` with a signed body — a one-of-two shape `required_scope` does not have today, stated. | — | the scope shape and the retired word. |
| **D4** | **CA succession is a pre-committed successor, acted on once and persisted — the row is written after the spike S-succeed.** Decided: `ca-commit.pem`, `tls commit`, a successor-signed record with the window's end inside it, act-once-persist, the KV shadow counted. Settled by spike: **whether a node on `cert_pem` can take a successor-issued certificate without a restart** (no reload path exists; `generate_rotation_after` needs `ca-key.pem`, which the secure profile forbids) — if not, the exit is one restart per node inside the window, and the row says so. | — | rev 0.5's exit "without a node restart" had no path. |
| **D5** | **A relay carries a signed decision under `grant:relay`**; an older gateway accepts the scope silently and it admits nothing. | — | unchanged. |
| **D6** | **Key-less principals keep read scopes and `grant:relay` after the window and lose action scopes** — `oidc:`, `anonymous`, the legacy `*` token — at `/mcp`, the KV routes and `/a2a`; OIDC returns by **token exchange** (I5) where the IdP supports it. | — | unchanged. |
| **D7** | **The consent primitive is an OS user-presence check whose result the repository verifies, graded on the record — the mechanism is settled by the spike S-presence (I7).** What the fourth review established from Apple's and Microsoft's documentation: a Secure Enclave key is **P-256**, reachable only from a **team-signed bundle with a provisioning profile** (a plain `cargo build` binary cannot use it, and the release pipeline builds plain binaries); the ACL cannot be read back, so the check is a negative probe (`interactionNotAllowed` must fail) plus `kSecAttrTokenIDSecureEnclave` and the enrolled public key compared on every use; `biometryCurrentSet` removes the typed-password leg; a fresh `LAContext` per approval. **Windows Hello keys are user+device scoped** — a same-user process that knows the credential name can request a signature — so Windows grades **no higher than `tty`** until shown otherwise. The grade on the record: `os_presence(macos)` · `tty` (Linux, Windows) · `recording` (a `sim` test build). | the rule "a check no co-resident process can satisfy, whose result we verify"; where none exists the record says so. | rev 0.5 assumed an Ed25519 key, a readable ACL and a Windows primitive. |
| **D8** *(rev 0.6)* | **Wave 1 is split: 1a is built from this plan; 1b's rows are written from their spikes.** Four reviews found every premise failure three levels down in `tls.rs`, `gateway_authority.rs` and `provider_enforcement.rs`, where a document cannot see; the policy says rows close on evidence, and 1b's evidence is code. A spike is a day's work on a branch, kept or discarded, whose result (what ran, what it cost, what it could not do) is quoted in the row it settles. | the maintainer's standing rule: after ~3 rounds whose findings sit in review-added mechanisms, simplify and ship. | new. |

**A re-decision recorded.** [`adopter-surface.md`](adopter-surface.md) line 52 decided a task API "belongs in a companion
on tuple-space claims, never in the substrate". This plan **narrows** it: cancellation is the authority law applied per
call — a mandate's revocation — not a task API; resumable states, deadlines and progress stay in the companion.

## 3 · Rows

Every row follows the standing rules: the regression test is written first and seen to fail on the unfixed code; every
entry point is enumerated with its grep; an independent adversarial review runs before merge; a behaviour change
carries a CHANGELOG entry and an upgrade note; a row lands a feature page and a [`registry.toml`](../features/registry.toml)
entry. Names marked *(new)* do not exist in the tree today.

### Wave 1a — identity for services (one MINOR, buildable from this plan)

#### I0 · This node's evaluator, authority and journal from its declaration

**What:** `[authority]` *(new config table)*: the declaration directory, **`unit`** — the unit that is this node's
(`principal`, the join key in `capability_config.rs`) — the evidence journal's path, the tier, the freshness bound and
the entitlement rule. At `start()` the binary builds the reference evaluator **from that unit's `[[rule]]`s only**
(the `test --faults` builder, [`src/faults/grant.rs`](../../src/faults/grant.rs), keeps a bare `tool:x` rule *and* adds
`tool:x@node`; this row keeps the bare form for this unit and loads no other unit's rules — a declaration carries no
placement, so another unit's rule cannot be qualified here), the operation map covering every door the binary serves
(an unmapped door refuses the start by name), constructs the authority and attaches all three with
`with_action_evaluator`, `with_execution_authority`, `with_provider_enforcement` and `with_evidence_journal`. **A build
that cannot enforce the table (no `compliance`) refuses the start by name** — the v2.18.1 class; an older binary ignores
the table silently and the upgrade note says so. `doctor` reports `ae.*`, `prov.enforcement` and the journal.

**Stated, not solved:** a grant issued at this gateway for an agent on another node is evaluated here under this unit's
rules; the issuing rule (`requires_grant`, its issuer) must therefore be declared in **every** unit that may issue, as
the scaffolds already do. A fleet-wide rule set is a later row.

**Depends on:** nothing. **Enumerate:** `grep -rn "with_action_evaluator\|with_execution_authority\|with_provider_enforcement\|with_evidence_journal\|ExecutionAuthority::new" src examples tests`,
the faults builder, the operation map in `action_evaluator.rs`, `GossipConfig::load_from_file` in
[`mycelium-core/src/config.rs`](../../mycelium-core/src/config.rs) (tolerant — `#[serde(default)]`, no
`deny_unknown_fields` — which is why an older binary ignores the table silently; `OidcNotInBuild` there is the refusal to
copy) and the unit loader in [`src/capability_config.rs`](../../src/capability_config.rs), which does deny unknown keys.
**Exit:** the binary over the `governed-call` scaffold refuses a call no rule permits at its gateway door and a call
under a revoked grant at its provider door, both journaled; `no_evaluator` never appears; a second unit's rule in the
same directory does not admit at this node; an unmapped door, an empty directory, an undeclared unit, and a build
without `compliance` each refuse the start by name. **Not built, stated:** other units' rules; the Cedar evaluator from
configuration (private); a gate spanning more than one `ResourceAuthority`.

#### I1 · Client certificates for services (D1, D1′)

*Delivered (#741–#744 and its PR 4); closed promise by promise in §8; the refusal of an unsigned call is §9 O3.*

**What:**
- `mycelium tls issue --client <name>` *(new flag)*: an Ed25519 certificate with SAN `mycelium-client://<issuer>/<name>`;
  the key held by the service's process (environment variable or file; **an agent's shell inherits either**, so a key an
  agent can reach is an agent's key). Both mTLS verifiers refuse the scheme (seen failing first). Revocation of a client
  key is **its re-issue, by supersession**: the revocation ledger is node-keyed (`sys/revocation/{node}/`;
  `counted_revoker_of` and `refuse_if_revoked` judge a *node's* standing), so a client has no entry there; instead the
  client-cert fold keeps, per principal, a **high-water mark of the newest verified issuance** — the certificate's
  serial, which is its `issued_ms` (`CertIdentity.issued_ms`; every certificate this tree issues shares one fixed
  `notBefore`, so that field orders nothing) — **in memory and in the node-local journal**, the ledger's own pattern
  (`merge_anchors_seen`, the counted revocations kept across a restart), and a certificate at or below the mark is
  refused for good: a later overwrite of the newer record, by LWW or anti-entropy, cannot lower it. The journal half
  holds only under `[persistence]` and within a budget — the ledger's is 64 records per node and 65 536 in all, a
  node past it *saturated*, and a failed append keeps the fact in this process only — so the client arm gets its own
  line in that budget, one record per supersession per principal, and the exit says *under `[persistence]`*. Two certificates
  of one principal in one millisecond tie-break on the greater key hex, stated. The bound is gossip latency, not a
  counting window; a client arm of `sys/revocation/` is the alternative, the maintainer's to prefer — it would inherit
  the ledger's memory under the same `[persistence]` and budget, which is the argument for it.
- **`POST /gateway/identity/client-cert`** *(new door, under `identity:write` — an existing scope, `compliance`-gated at
  `/gateway/identity/revoke`; this door is gated the same, and a token already holding the scope for revocation gains it
  at the upgrade — §5)*: the gateway verifies the chain
  against the CA anchor and publishes `sys/client-cert/{principal-hex}/{key-hex}` *(new namespace, absent from
  `SELF_OWNED_SYS_PREFIXES` and therefore never counted by the tripwire — no exemption is needed; any member's own
  `kv().set` can write it, and the raw KV route's `sys/` refusal does not bind a member, so the fold judges by the
  record's signature and the certificate's CA issuance — `judge_cert`'s shape,
  [`revocation.rs`](../../src/agent/revocation.rs) — never by KV LWW time: a stale-but-genuine record carried by
  anti-entropy, or a member's overwrite of the newer record with garbage, cannot re-admit a superseded key once the
  mark has risen)*; a client-cert fold beside the node one; a superseded client key refused once the newer record is
  read, and still refused after that record is overwritten and, under `[persistence]`, after a restart.
- `TrustedExternalIssuers` shared at `ExecutionAuthority`, `ProviderBudget`, `MembershipAuthorities` (the last never
  entitles a client) — held by value with no setter at all three today, so the shared handle changes
  `ExecutionAuthority::new`'s signature (§5); a lock-order row.
- A `token:` principal with a certificate on record and no `cs`: counted (`gateway_actions_unsigned{kind}` *(new)*),
  then refused; a key-less principal: D6's window. A client's call at a gateway without `gateway_identity_issuer` is
  refused by name at call time.
- `cs`/`cn` (D1′): `frame_with_context_at` writes them; `verify` checks `cs` before the cache; the signature vector under
  `tests/fixtures/cs/` *(new)* beside the existing digest vectors, checked by Rust, jest and pytest.
- `grant/` keys: `node:` literal, client principals hex, readers decode by prefix (`pending()`, the stream's `rsplit`,
  the faults reader, `decide_grant`/`revoke_grant`'s `/` check, `is_member_agent`, the checkpoint and request keys).
  **The resource a client-held grant's decisions evaluate against** (`revoke_grant` derives `tool:{tool}@{node}` from a
  `node:` holder today) is **the provider node of the tool**, resolved from the `tools/` scan as `/mcp` does — stated,
  a maintainer may prefer the gateway. A client request door `POST /gateway/grants/request` *(new)* under `mcp:invoke`.
- SDKs: a `tools/call` client *(new in both)* and the A2A client carry `cs`/`cn`; `mycelium-ts` via `node:crypto`,
  `mycelium-py` via `cryptography` *(new optional dependency)*.

**Depends on:** I0. **Enumerate:** both verifiers and `verify_member_cert` (which must **not** apply the scheme refusal
— the client fold checks the chain), `grep -rn "fn verify_client_cert\|fn verify_server_cert\|fn verify_member_cert\|refuse_if_revoked" mycelium-core/src/tls.rs`;
possession; the envelope and `ReplayWindows`; `gateway_identity_issuer`; every `grant/` reader and writer (`grep -rn '"grant/\|grant_checkpoint_key\|is_member_agent\|unreadable_grant\|rsplit' src`);
the three holders; `SELF_OWNED_SYS_PREFIXES`, `OWNED_KV_PREFIXES` and `refuse_protected_key`; the folds and `judge_cert`;
`arguments_digest` and `admit_after`'s digest; both SDKs' A2A clients; the lock-order table.

**Exit:** a client certificate at the mesh refused by both verifiers; a pre-row-Q node certificate still admitted;
Y6's weather tool called through `/mcp` **under** a grant held by the example's `token:fleet/forecaster` on a
three-node fleet of published binaries (I0) with `gateway_identity_issuer = "fleet"`, refused without one, surviving a
restart; a foreign-key `cs` refused; a replayed `cn` refused and a reordered honest pair admitted; two processes under
one certificate not colliding (distinct `instance`); the cross-language vector green in all three suites; a superseded client key refused once the newer record
is read, still refused after the record is overwritten and, under `[persistence]`, after a restart; `revoke_grant` on a client holder evaluating `tool:{tool}@{provider}`; a pre-upgrade `grant/node:…` record
read unchanged. **Not built, stated:** OIDC (I5); the developer's keystore key (I7); M-of-N; an un-upgraded node's
reading of `sys/client-cert/` (shown against the previous binary).

#### I3 · Signed decisions (D3, D5)

**What:** the decision routes take a body signed by a certificate-holding principal; the gateway verifies it, checks
the signer's `grant:issue`, checks the addressed node is itself (`not_addressed_here` *(new)*), and records the signer;
the route admits `grant:issue` on the bearer **or** `grant:relay` *(new scope)* with a signed body — `required_scope`'s
one-scope shape becomes one-of-two for these routes. SDK verbs sign. **And O1 (§9), placed here by the maintainer
(2026-11-09):** the mandate/grant `arguments_digest` moves to RFC 8785 (JCS) in Rust, Python and TypeScript — every reader
§9 O1 lists — with **a window for grants already issued** (a grant issued before the change binds the old digest: one
release accepts a binding or possession over either digest, and says which it matched, or the issuer re-issues; the
row chooses and states which) and **the large-integer rule** (an integer outside ±(2^53 − 1) refused where the digest is
taken, as I1's client signature refuses it, since JCS cannot bind it exactly). **Depends on:** I1. **Exit:** `not_the_decider`
*(new)* on a foreign signature; `not_addressed_here` on misdelivery; a relay with only `grant:relay` reaching the route
and succeeding with the signer as actor, refused unsigned; a re-targeted body refused; the journal names the signer; the
Cedar re-pin with a refusal case; for O1, an exponent-form float (`1e-5`, `1e21`) in a grant's arguments admitted from
Python and TypeScript, the cross-language vector carrying such cases in all three suites, a grant issued before the change
honoured or refused as the window states, and an integer outside ±(2^53 − 1) refused by name. **Not built, stated:**
M-of-N; an `oidc:` decider; a relay at an un-upgraded gateway.

#### I5 · OIDC by token exchange (D6)

**What:** RFC 8693 exchange at the IdP for `aud = mycelium:<domain>` bound to `cn`; the assertion in `ot` *(new)*;
the provider validates it against the issuer's JWKS — the verifier is `#[cfg(feature = "compliance")]`, so a provider
built without it refuses a call carrying `ot` at call time as `not_in_build` naming `compliance` — the response error
`govern.rs` already answers for a feature the build lacks, not v2.18.1's start-time refusal — rather than running it
unverified; the envelope is capped at `MAX_ENVELOPE_BYTES` (8 KiB under a `u16` prefix), so an assertion beside `m` and
`cs` can exceed it and the producer's existing refusal answers — `GatewayDispatchError::ContextTooLarge`,
`caller_context_too_large`, counted in `mycelium_gateway_caller_refusals_total` — whether the cap rises is the
maintainer's; a gateway-audience token is never forwarded; `doctor` reports
`id.oidc_exchange` *(new)*. **Depends on:** I1. **Exit:** an OIDC user's call verified at the provider; a replay
refused; a gateway-audience token refused by name; a provider without `compliance` refusing `ot` as `not_in_build`; an
oversized envelope refused `caller_context_too_large`; a member's forged `ot` refused by the JWKS. **Not built, stated:** an
IdP without exchange (the client certificate is the path); a decision signed by an assertion.

### Wave 1b — three spikes, then the rows they settle (D8)

A spike is one branch, at most two days, read-only on `main`, whose report is a dated note in `docs/plans/spikes/`
*(new directory)* quoting what ran, what it measured and what it could not do. Its row is written afterwards from that
note and goes through the standing review as any row does. No spike ships.

| Spike | Question only code can answer | Done when |
|---|---|---|
| **S-cancel** | A `call:` mandate minted at admission reaches the provider before its checkpoint gossips. Carry the first checkpoint **inside the envelope** beside the mandate, through `assess` and `admit_after` to `begin`; measure the added bytes and the admission latency against today's grant path; show a revoked term read `Refused`; and two bounds the row must state: a term revoked between the gateway's framing and the provider's `admit_after` carries a checkpoint that predates the revocation, so the provider registers the work and stops it only when the revoking checkpoint arrives — the cancel's effect is gossip latency plus the drain bound; and `drain_bound` is `Unbounded` for the unenforced and unconfirmable classes ([`authority.rs`](../../src/mandate/authority.rs)), where a cancel writes `Refused` and stops nothing — the cancel receipt names the drain class it hit, or answers `DeliveryUnknown`. | a call registered with `begin` without a KV round-trip, or the bounded wait's cost measured; the memory-reclamation hook shown on 10⁵ calls |
| **S-succeed** | A node on `cert_pem` (secure profile, no CA key) takes a certificate issued under a successor CA **without a restart**: a reload path through `activate`, a two-root `RootCertStore`, `verify_member_cert` across the window. | the handshake succeeds under the new root on a live node, or the spike records that a restart is required and why |
| **S-presence** | `dev` signs with a Secure Enclave P-256 key from a **team-signed bundle**, verified by one P-256-aware verifier; the negative probe (`interactionNotAllowed` fails), `kSecAttrTokenIDSecureEnclave`, `biometryCurrentSet`, a fresh context per approval; and whether the release pipeline can produce such a bundle at all. | a signature produced only after the prompt and verified; the pipeline question answered yes or no; if no, I7 falls back to `tty` everywhere, graded |

#### I4 · Cancellation under a `call:`-scoped mandate — *written from S-cancel ([note](spikes/2026-11-07-s-cancel.md), 2026-11-07; evidence at tag `spike/s-cancel-2026-11-07-r2`) — **not yet buildable: N1–N5 open***

Decided (D2): scope prefix; holder = the gateway; second mandate slot; `Refused` on revoke; `is_entitled`'s `call:`
arm; incremental feeder; in-memory reclamation with a lock-order row; a cancel-forward RPC kind *(new)*; A2A
`tasks/cancel` revokes before it removes (`handle_tasks_cancel` is sync — the revocation is spawned, stated); MCP
`notifications/cancelled` *(new handler)* with a bounded id → term table; retention `max_drain_secs` *(new)* + the sweep
ceiling for `call:`, `grant_retention_secs` *(new, default 7 days)* for `grant/`; a `cancellable` key is **refused by
name by a 2.38 reader** (`RuleDecl` and `MandateDecl` deny unknown keys, `deprecations.md` §58–§65) and read as a plain
allowance by a ≤ 2.37 one, so it is declared only on an upgraded fleet.

**From the spike** ([S-cancel's note](spikes/2026-11-07-s-cancel.md), 2026-11-07). The evidence is the tag
`spike/s-cancel-2026-11-07-r2` (never merged), and the leak's reproduction is at `spike/s-cancel-leak-2026-11-07`. Its
numbers supersede the first version's, which #731's review corrected.

**Establishment.** The admitting gateway carries the mandate's **first revocation checkpoint inside the envelope,
beside the mandate**, as `PresentedMandate` carries a grant and its proof.
- Once the mandate has verified (issued, entitled, current, possessed), the provider offers the carried checkpoint and
  its own KV copy of the call's checkpoint. It offers them only for the mandate's own authority and scope.
- A provider that has never seen the checkpoint then establishes the mandate, and `admit_after` registers the work with
  `begin`. There is no network round trip and no wait (note S1, S7). There is one **local** read: the provider's own
  KV copy of the call's checkpoint (`local_call_checkpoint`, a `kv_get` and a parse), done outside the S2 measurement.

**The bounded wait is rejected.** One hop is fast on loopback, 0.43 ms at p50 and 0.48 ms at p95 (n = 100, note S6′).
The reason for rejecting it is that **under a partition it has no bound**.

**Without either, the outcome depends on the rule.** Where a rule admits the call without a mandate, the call is not
registered, and a cancel cannot stop it: the uncarried call ran to completion (note S7). Where a rule requires a
mandate, the `Unknown` binding is refused (`src/agent/action_evaluator.rs:2106–2116`).

**What it costs.**
- **On the wire:** +360 B on the presented mandate (612 → 972 B) against the 8 192 B envelope cap. The checkpoint's
  signature serialises as a JSON array; base64 would bring it to about 180 B. The full envelope was not measured.
- **At the provider:** the same `assess` cost as today's grant path. Carried: 114.6 / 113.8 / 136.4 µs mean / p50 /
  p99. Grant path: 116.1 / 115.0 / 138.8 µs. That is release, 20 000 calls interleaved, three repetitions'
  means within 1.5 µs of each other (their p99s spread 136.4–143.9 µs) (note S2). What the 1.2–1.5 µs gap consists of is **not measured**. The local KV read above is outside both
  figures.
- **At the gateway:** three signatures per call (the mandate, its first checkpoint, the possession proof), 38.7 µs mean.
  The durable write of each call's first checkpoint (exit 11) **is not measured**. It adds an fsync per admission, where
  today a grant's checkpoint is written once, at issue. Exit 11 measures it.

**The two bounds the row states.**
1. **A term revoked between the gateway's framing and the provider's `admit_after`** is admitted and registered,
   because its carried checkpoint predates the revocation. It stops when the revoking checkpoint arrives. **This is
   the design, not yet what the spike's code does** (N1, N2): a revocation that lands between the preflight and
   `admit_after` today runs the call unregistered, and one written between the KV read and the window's insert is
   never offered.
   - The cancel's effect is **the gossip latency + the sweep interval (*F* / 20, clamped to 100 ms–5 s) + the class's
     `drain_bound`**.
   - Measured on loopback, n = 20: **41.7 ms p50, 43.4 ms p95** from the revoking write to the call ending, with *F* =
     2 s (a 100 ms sweep) and a handler that stops at once (note S7).
   - A carried stale checkpoint cannot un-revoke (note S3).
   - **The bound holds only while the revoking checkpoint can arrive.** Under a partition, or for a class with an
     `Unbounded` drain, only the mandate's window caps the work.
2. **`drain_bound` is `Unbounded`** for a class whose resource cannot confirm a stop, and for an unenforced
   `max_duration` (`src/mandate/authority.rs`; note S4). There, a cancel writes `Refused` and stops nothing the
   resource confirms. The cancel's receipt names the drain class it hit, or answers `DeliveryUnknown`.

**Reclamation, as the spike now builds it.**
- **What is kept:** a `call:` scope's checkpoints and revoked set are kept **until its mandate's window ends, not until
  its work ends**, so a revoked call re-presented inside its window stays refused.
- **What may be held:** only **scopes this node admitted**. The background feeder skips a `call:` checkpoint for any
  other scope. The spike's first version let the feeder offer every fleet-wide `call:` checkpoint and reclaimed only
  its own scopes. That leaked every other call for good: 2 000 fleet checkpoints were held and never reclaimed. With
  the feeder skipping, they are not held at all (note H2).
- **Why this option:** it keeps a node's memory proportional to the calls it admitted. Tombstoning forgotten scopes
  would grow without bound. Keying reclaim on the checkpoint's own window would need a format change, and would still
  hold every fleet call until it expired.
- **The race survives it:** a node that admits a call after its revocation was written reads its own KV copy at
  admission, so the race of bound 1 closes once N2 is settled (not yet: N1, N2).
- **A second mandate on one scope** with a later window moves the scope's expiry to the later end, so the scope is
  never reclaimed while that mandate still runs (note S7′).
- **How it is reclaimed:** from an expiry queue, at most **1 024 scopes per sweep**. On 10⁵ windows, an idle sweep holds
  row 43 for under 2 µs and the worst reclaiming sweep for 571 µs; 98 sweeps drain 10⁵ in 51.0 ms in total, through `sweep()` (note S5,
  S6). The first version scanned every window on every sweep and held row 43 for 130 ms.

**Three findings the row handles.**
- **`PresentedMandate` is public**, so a new field breaks an exhaustive literal. There are six outside the library at
  `e8e58b2e`: `tests/gateway_mandate_external.rs`, the examples `authority_drain`, `confined_fleet_node` and
  `composed_commit`, and two in `mycelium-effects/tests/composed.rs`.
  - The row makes the struct `#[non_exhaustive]` with a constructor, or carries the checkpoint under its own `_meta`
    key. Either way it gets a `deprecations.md` entry.
  - The SDKs carry the shape as JSON (`mycelium-ts/src/a2a.ts:10`, `mycelium-py/src/mycelium/a2a.py`), where an
    optional field is additive. Each learns it.
- **Every offered checkpoint opens a revoked-set entry for its scope**, even one that revokes nothing: one per call, not
  per revoked call. The row opens the entry only when a term is revoked, or counts it in its memory bound.
- **The revoking checkpoint travelled under the grant key family by a plain KV write.** The row gives `call:` its own
  key family and feeder, and writes the first and the revoking checkpoint **durably** (`write_record_durable`), as a
  grant's checkpoint is.

**Not built by the spike, the row's work.**
- the gateway minting the `call:` mandate at admission, with the second mandate slot;
- the holder as the gateway behind a client's call;
- the gateway's entitlement for `call:` (the spike entitled any `node:` issuer fleet-wide, which the row must not);
- the cancel receipt;
- the cancel-forward RPC kind;
- A2A `tasks/cancel`;
- MCP `notifications/cancelled` and its id → term table;
- `max_drain_secs` and `grant_retention_secs`;
- the `cancellable` key;
- the window table across a restart.

**Rev 0.5's exit list.** The row said *"the exit list of rev 0.5 then stands, with the soak"*. That list is not in the
repository: the plan entered the tree at rev 0.6 (#715, one commit), and a `git log --all -S` search confirms it is not
in #715's thread. The exit below is written from D2 and the note. **The soak is stated here for the first time**, for
the review to confirm.

**Open before build (round-2 review).** The build PR settles each of these questions, with the exit beside it.
- **N1 (high): a revocation landing between the preflight and `admit_after` runs the call unregistered.**
  - The preflight finds the mandate `Established` and admits the call. If the revoking checkpoint lands before
    `admit_after`, `admit_after`'s second assessment reads `Refused`. It then returns `work: None`, and the handler runs
    with nothing registered, so nothing can stop it (`src/agent/provider_enforcement.rs:329–343` on `main`).
  - `main` has the same shape for grants today. The PR fixing `admit_after` for grants was opened 2026-11-07.
  - **Exit needed:** `admit_after` refuses a call whose mandate the preflight found `Established` and it does not. It
    never admits it unregistered. Shown with the revoking checkpoint offered between the two assessments, for a
    `call:` mandate and for a grant.
- **N2: the window must exist before the KV copy is read.**
  - The KV copy is read before the call's window is inserted. A revocation written to the KV between that read and the
    insert is skipped by the feeder, which offers only scopes this node admitted. So it never reaches the authority, and
    the work runs unrevoked.
  - **Exit needed:** at every door that admits under `call:` (the gateway's preflight, the provider's `admit_after`), the
    scope is admitted (its window inserted) **before** the KV copy is read. Shown with the revocation written in that
    interval.
- **N3: the holder check is against the wrong principal.**
  - At the provider, `assess` compares the mandate's holder with `principal`: the verified **client** principal for a
    gateway-dispatched call (`src/agent/provider_enforcement.rs:254`, passed to `admit_after` at `:278`, `:283` (the deferred, `Settle` path) and `:287`, and assessed at `:330` on `main`). D2's holder is the **gateway**.
  - **Exit needed:** a `call:` mandate's holder is compared with the gateway that signed the call's envelope, not with
    the client it acts for. A `call:` mandate whose holder is any other gateway is `Unknown`. A client presenting the
    gateway's `call:` mandate as its own is refused.
- **N4: with two slots, revoking either mandate must stop the work.**
  - A client's call runs under its own grant in one slot and the gateway's `call:` mandate in the other. The work is
    registered under one mandate today.
  - **Exit needed:** work admitted under both is registered under both. Revoking the client's grant stops it, and so
    does cancelling the `call:` term, each within its bound.
- **N5: exit 11's restart clause, and `call:` retention.**
  - The window table is in memory. After a restart, a revoked `call:` re-presented inside its window is refused only if
    the revoking checkpoint is still in the node's KV. What keeps it there, and for how long, is not stated.
  - **Exit needed:** after a WAL-only restart, a revoked `call:` re-presented inside its window is refused. The `call:`
    key family's records are kept at least until the mandate's window plus `max_drain_secs`, and collected afterwards,
    each shown by a test.

**Exit.** Each item is a test or a run. Every D2 decision has an item or is marked *not built*.
1. **Establishment.** A `call:` mandate minted at the gateway's admission is established at a provider that holds no
   checkpoint for it, and the work is registered (`running_count`). The same call with the checkpoint stripped is
   `Unknown`: not registered where the rule admits without a mandate, refused where it requires one. Seen failing first:
   before the row, the stripped form is the only form.
2. **The holder and the second slot.** The holder rule is N3's to settle, and this item takes N3's resolution. A client's call carries it
   in the second mandate slot beside the client's own grant, and both are assessed. A `call:` mandate presented by any
   principal other than its holder is `Unknown`.
3. **Entitlement, negatively.** Only a gateway configured to mint `call:` mandates is entitled for the `call:` prefix. A
   `call:` mandate signed by another member node is refused `NotEntitled`, with `entitle_grants_from_fleet` on.
4. **A cancel stops admitted work within bound 1.** Revoking at the gateway ends a running cancellable call at the
   provider within gossip + *F* / 20 + the class's `drain_bound`, asserted against the configured *F*. The stop record
   is in `stop_records`.
5. **The race** (bound 1): a call framed before the revocation and admitted after it is stopped when the revoking
   checkpoint arrives. Re-presenting it afterwards is refused `Revoked`.
6. **Refused at the assessing doors.** A revoked term reads `Refused(Revoked)` at the gateway's preflight and at the
   provider.
7. **The cancel doors revoke.** A2A `tasks/cancel` revokes the call's term **before** it removes the task (the
   revocation spawned, since `handle_tasks_cancel` is sync, and stated). MCP `notifications/cancelled` revokes through
   a bounded id → term table, where a full table refuses a new entry and is counted. The cancel-forward RPC kind reaches
   the provider. Each is shown stopping a running call.
8. **The unbounded classes** (bound 2): a cancel of work whose class is `Unbounded` answers with a receipt naming that
   class, or `DeliveryUnknown`, never a stop it cannot confirm.
9. **Reclamation with the feeder running.** A node that admitted none of 10³ calls whose `call:` checkpoints are in its
   KV holds none of them (view sizes zero). A node that admitted them returns to zero one sweep past the last window,
   and stays at zero when the feeder is woken again. A revoked call re-presented inside its window stays refused.
10. **The sweep's hold.** No sweep reclaims more than its per-sweep cap. Row 43's entry in
    [`lock-order.md`](../wiki/dev/concurrency/lock-order.md) states the `call:` windows it now holds and the hold bound,
    from a measurement on the shipped code. D2's *"a lock-order row"* is met by row 43's entry: the windows live inside
    `AuthorityState`, so no new lock is added.
11. **Durability.** The first and the revoking checkpoint are on disk before the call is admitted or the cancel
    acknowledged, and the fsync's added admission latency is measured. After a WAL-only restart, a revoked `call:`
    re-presented inside its window is refused (N5).
12. **Retention.**
    - `max_drain_secs` caps how long a cancelled `call:`'s records are kept past its stop, and the sweep ceiling applies
      to `call:` work.
    - `grant_retention_secs` (default 7 days) collects `grant/` records past it.
    - A test for each shows the records gone after the setting, and kept before it.
13. **The issuer down.** With the admitting gateway stopped, a running `call:`'s work ends at its mandate's window. The
    sweep ends it past `not_after_ms`, and the term then reads expired.
14. **The soak.** 10⁵ cancellable calls over a two-node fleet, half of them cancelled while running.
    - Every cancelled call ends inside bound 1.
    - No call ends without a stop record or its own completion.
    - Both nodes' views return to zero after the last window.
15. **Compatibility.**
    - The six `PresentedMandate` literals compile under the chosen shape, and its `deprecations.md` entry exists.
    - Both SDKs' types carry the optional checkpoint.
    - A 2.38 reader refuses a `cancellable` key by name.
    - The ≤ 2.37 reader, which reads a `cancellable` key as a plain allowance, is **not built as a test**: no 2.37
      binary runs in CI. The upgrade note says to declare it only on an upgraded fleet.
16. **The incremental feeder.** The `call:` feeder offers only the checkpoint whose key was written, not a rescan of
    the key family. Its cost per write is measured independent of how many `call:` checkpoints the KV holds (10², 10⁴).
    Today's grant feeder rescans `grant/` on every write (`feed_grant_checkpoints`), and the spike kept that shape.
17. **The measurement.** S-cancel's figures are re-run on I4's shipped code and dated on
    [`what-is-proven.md`](../operations/what-is-proven.md) (§6.4). That covers the provider's `assess`, the gateway's
    mint with its durable write, the bytes, and bound 1 with its n. The spike's row there is replaced by the shipped
    one.

#### I2 · CA succession by a pre-committed successor — *written from S-succeed ([note](spikes/2026-11-07-s-succeed.md), 2026-11-07; evidence at tag `spike/s-succeed-2026-11-07-r2`) — **not yet buildable: R1–R4 open***

Decided (D4): `ca-commit.pem`, `tls commit`, `tls succeed`, the successor-signed record with its window end, act-once
and persist, a **gateway door** *(new)* that publishes the record (as I1's does), the shadow counted, the offline node
cut off and counted, `secure-single-domain` rev 6 / `secure-multi-domain` rev 2 requiring `id.ca_succession_committed`.

**From the spike:** a node on `cert_pem` with no CA key **re-roots live, without a restart**. Two live nodes (A, B)
stayed up through widen, cut over and narrow. A fresh node holding a successor certificate joined live A, and A, once
cut over, presented its own successor certificate on a socket to a node that trusts only the successor. Building the
material took 0.97–1.14 ms, `activate` 23–26 µs and the whole call 1.23–1.40 ms, and an existing session carried the next
write in 25–29 ms.
A new node's first write reached A in the same time as a control join. Measured: that time tracks
`health_check_interval_secs` and not `handshake_timeout_ms`. Inferred, not measured: the mechanism is the anti-entropy pull
on a tick, and whose tick it is has not been attributed. The note gives each figure with its run.

`main` has no such path:
- `cert_verifier` and `fleet_server_verifier` are fixed for the node's life;
- `RotationMaterial` comes only from `generate_rotation_after`, which needs `ca-key.pem`;
- `build_rustls_configs` builds a store of one root;
- `pem_cert_to_der` reads only the first certificate of `ca_cert_pem`.

The re-root does **not** survive a restart. The node's identity key after succession is its **current** key, re-issued
under the successor; `rotate_identity` still cannot run on the off-node posture.

**What:**
- **The reload path.** `cert_verifier` and `fleet_server_verifier` become swappable, and `activate` swaps them when the
  material carries new ones; a same-CA rotation carries none and behaves as today. Rotation material can be built from
  a pre-issued certificate and key over a **set** of roots (the spike's `reroot_material` and
  `build_rustls_configs_roots`), with no CA key.
- **Every cached judgment of a certificate is re-judged when the roots change.** Three readers judge a certificate record
  against the roots, and only one does so afresh:
  - the KV arm's `certificate_record_proves_itself` (`connection.rs`) calls `verify_member_cert` each time;
  - the revocation fold's `cert_facts` (`revocation.rs`) caches each verdict by the certificate's hash, cleared only at
    4,096 entries;
  - `merge_anchors_seen` keeps every anchor it has merged for good.

  Under today's code a change of roots therefore leaves a long-running node judging by the old roots while a restarted
  one judges by the new, and the KV arm can disagree with the fold on the same node. Concretely: a node that judged a
  successor-issued record before it widened keeps it unanchored, and one that judged an old-CA record before it
  narrowed keeps it anchored. I2 clears `cert_facts` on every activation that changes the roots, removes from
  `anchors_seen` every anchor whose only record no longer verifies, and re-folds every node. A long-running node then
  judges as a restarted one does. Until that is built, the disagreement is the stated state.
- **The cutover order is the protocol: widen, cut over, narrow.** Every node widens its trust to the old and successor
  roots before **any** node presents a successor certificate. A node that cuts over early loses new handshakes with
  every peer still on the old root, in both directions, while its existing sessions hide that until the next reconnect
  (note, step 1). `tls succeed` acts on the successor-signed record once per phase:
  - **Widen:** at once, on reading the record. The node persists the widened phase and its trust set **before** it
    publishes that it has widened (D4's shadow), so a node that says it has widened does so after a restart too.
  - **Cut over:** at the record's window start, whether or not every node has reported widening. Holding would let one
    offline node stall the fleet; proceeding cuts it off. A member not reported widened by then is counted and named. A
    node with **no successor certificate** in `succession/` when the window starts does not cut over and does not
    narrow. It stays widened, presenting its old-root certificate, is counted (`ca_succession_uncut`) and named, and is
    cut off by its peers when they narrow.
  - **Narrow:** at the window's end. A node still on the old root is cut off and counted, which is D4's offline node. A
    node that was offline through widening and returns inside the window widens on reading the record at start; one
    that returns after the window's end stays cut off and is counted. **The operator's recovery for it:**
    1. On the issuer's host, issue the node a certificate under the successor (`mycelium tls issue` against the
       successor's directory).
    2. Point its `cert_pem`, `key_pem` and `ca_cert_pem` at the successor's files, and remove its `succession/`
       directory.
    3. Restart it. It starts on the successor alone, as a node issued after the succession would.
- **Where the successor certificate lives, and the trust set.** The node never writes the operator's `cert_pem`,
  `key_pem` or `ca_cert_pem`: they may be on a read-only mount, and configuration is not state. Succession state lives
  in the node's own directory (`auto_cert_dir`, `succession/`): the phase, the successor's root, and the successor-issued
  certificate. The certificate reaches the node **by operator copy only**: the issuer's host writes it, and the
  operator copies it into the node's `succession/`. D4's gateway door publishes the successor-signed **record**, not
  certificates. The node reads the copied file and records it, with its directory synced (R6's `sync_parent_dir`) before the cutover is
  recorded. At start, the persisted phase decides:
  - **Widened:** the roots are `ca_cert_pem` and the successor.
  - **Cut over:** the same roots, presenting the successor certificate.
  - **Narrowed:** the successor's root only, presenting the successor certificate.

  A node restarted in any phase comes back in that phase. **End state:** after narrowing, the operator replaces
  `cert_pem` and `ca_cert_pem` with the successor's files at leisure. Until they match, the node warns at start naming
  both files and still starts on the persisted state.
- **No silent root drop.** A `ca_cert_pem` holding more than one certificate is refused by name, because
  `pem_cert_to_der` silently keeps only the first today. The refusal applies to `ca_cert_pem` only, not `cert_pem` (a
  chain) and not the on-node CA's `ca-cert.pem`. It is a behaviour change with an upgrade note (§5 and `deprecations.md`):
  no deployment can rely on a second certificate there today, since it was ignored.
- **A distinct successor subject.** `tls ca` mints a CA whose subject names the fleet and its generation, and
  `generate_node_cert` issues under **that** subject. Today it rebuilds the signing CA from fixed parameters, which
  carry rcgen's default subject, so a node certificate's issuer name always reads the default. Its comment says rustls
  chains by the SubjectKeyIdentifier; the spike's refusals say otherwise. Across two same-named CAs the refusal reads
  `BadSignature`, not `UnknownIssuer`, because the issuer is found by **name** and only then fails its signature (note,
  finding 3). The comment is corrected with the change. A CA already deployed keeps its name.
- **The re-issue of the current key, and of every unrevoked earlier one.** On the issuer's host, `tls succeed`
  re-issues each node's **current** public key under the successor, issued after that node's newest issuance (row Q's
  floor), and every unrevoked earlier key in its history (the decision below), each from its public key. The successor record is written at the
  same path as the old one, `cert_record_key(node, key)`, and replaces it under LWW. That the node's identity history,
  revocation standing and `anchors_seen` rank are unchanged by the overwrite is **asserted, not measured**; the exit
  tests it.
- **Old-CA records after the window narrows: decided, loss of standing.** Once a node trusts only the successor,
  certificate records issued under the old CA no longer verify, and the anchors they gave are removed (above).
  - **Why not the retired-roots rule.** Bounded safely, it needs the set of records merged before narrowing, identical
    on every node and persisted: `anchors_seen` is in memory only (`revocation.rs`, "kept for the process's life"), so a
    restarted node would hold a different set from a long-running one, and exits 2 and 9 could not both pass. Unbounded,
    a holder of the old CA's key can mint a record backdated to before the window's end.
  - **Why loss of standing holds.** It fails closed. A revoked old key that loses its anchor is refused at the
    handshake, because no node trusts the old root, and at every signature check, because the key is not anchored.
  - **What it costs, and the answer.** Signatures made earlier by a key anchored only by an old-CA record would stop
    verifying. So `tls succeed` re-issues under the successor **every unrevoked key** in the node's history, not only
    its current one, from their public keys on the issuer's host. A revoked key is not re-issued, and it stays refused.

**Open before build (round-3 review).** The build PR settles each of these questions, with the exit beside it.
- **R1 (high): a node left with no anchor becomes legacy, and its revoked keys verify again.** A node with no verifying
  certificate record is judged from `peer_keys` as before row Q, and no revocation of its keys counts
  (`src/agent/revocation.rs:12-16`, `:601`). Loss of standing that removes a node's last anchor therefore re-admits its
  revoked keys. **Exit needed:** after narrowing, a node whose every old-CA record lost standing is never judged
  legacy. Its revoked keys stay refused at the signature check and the handshake, on a long-running node and a
  restarted one.
- **R2 (high): the re-issued serials must keep the original order.** They must be distinct, in the original issuance
  order, with the current key last, or an old key can revoke the current one. Rank is the key's **first** issuance,
  carried in the serial (`revocation.rs:19-24`, `mycelium-core/src/tls.rs:916`). **Exit needed:** after re-issue, for
  every pair of a node's keys, the later-issued key still outranks the earlier. An old key's revocation of the current
  key does not count. No two re-issued keys are siblings unless they were before.
- **R3 (medium-high): two sources go missing.**
  - The off-node issuer has no source for which keys are revoked, so it cannot tell which to re-issue.
  - A revocation record's trailer carries the revoker's and the revoked key's old-CA certificates, which stop
    verifying after narrowing (`revocation.rs:36-38`).

  **Exit needed:** the issuer's source of revoked keys named and tested: a revoked key is not re-issued, and an
  unrevoked one is. A revocation counted before narrowing is still counted after it, on a long-running node and a
  restarted one, and one first seen after narrowing is judged by a stated rule.
- **R4 (medium): pruning `anchors_seen` by what the store now holds reopens demotion by hiding.** `anchors_seen` exists
  so that hiding a node's certificate record does not demote it (`revocation.rs:263-266`). Pruning an anchor because its
  record is no longer readable lets a member that hides the re-issued record demote the node. So an anchor must keep its
  certificate or the certificate's digest. **Exit needed:** after narrowing, hiding or overwriting a node's re-issued
  record does not remove its anchor, and an anchor is removed only when its kept certificate fails the new roots.

**Depends on:** S-succeed (done); the gateway door after I1's.

**Enumerate:**
- `grep -n "cert_verifier\|fleet_server_verifier\|fn activate\|fn build_rustls_configs\|fn pem_cert_to_der\|fn load_ca_cert_only\|fn generate_ca\|fn generate_node_cert\|fn verify_member_cert" mycelium-core/src/tls.rs`;
- every reader of a certificate record and its caches: `grep -rn "verify_member_cert\|fn cert_facts\|cert_cache\|merge_anchors_seen\|anchors_seen" --include='*.rs' src mycelium-core/src`, covering the KV arm in `connection.rs` and the folds, the cache and the anchors in `revocation.rs`, **and I1's client-certificate cache** (`cert_cache` in `src/agent/client_cert.rs`, keyed by digest, holding the chain verdict) — a root change must clear it, or a certificate the old root issued stays admitted until the cache churns;
- `fleet_server_verifier()` (the bulk fetch, `src/agent/bulk.rs`);
- the accept side (`tls_accept`, `src/agent/tasks.rs`) and the dial side (`mycelium-core/src/writer.rs`);
- the guarantee report's `id.*` descriptors;
- `mycelium tls` in `src/main.rs`.

**Exit:**
1. **Live, no restart.** A live node on `cert_pem` with no `ca-key.pem` widens, cuts over and narrows, with a KV write
   crossing the mesh as proof of each session (the spike's steps, as a CI test). At each phase, a handshake is shown in
   each direction. Two refusals are included:
   - after narrowing, a peer that **trusts both roots but holds an old-root certificate** is refused by the narrowed
     node's accept side, and that node, dialling, refuses it (spike M2);
   - the narrowed node presents its own successor certificate on a socket to a peer that trusts only the successor.
2. **Restart in each phase.** A node restarted in each phase comes back in it, from `succession/`. The operator's
   `cert_pem` and `ca_cert_pem` are unwritten: on a read-only mount the start still succeeds, and a mismatch is warned
   by name.
3. **Widening reported.** A node reports widening only after persisting it. A fault injected between the persist and
   the report restarts the node widened, and it then reports. A fault injected before the persist reports nothing,
   and the node restarts unwidened.
4. **The cutover proceeds** at the window start with a member unreported, which is counted and named. A node with no
   successor certificate at the window start stays widened, presenting its old-root certificate, is counted
   (`ca_succession_uncut`), and is cut off when its peers narrow.
5. **Offline nodes.** A node offline through widening that returns inside the window widens. One that returns after it
   is cut off and counted, and the operator's recovery (re-issue under the successor, the files pointed at the
   successor, `succession/` removed, a restart) brings it back.
6. **The record.** A succession record not signed by the committed successor is refused. A stale or replayed record is
   refused by act-once. The gateway door publishes the record, refuses a malformed one, and accepts no certificate.
7. **Distinct subject.** Two CAs minted by `tls ca` have distinct subjects. A node certificate's issuer name is its CA's,
   and a refusal across the two CAs reads `UnknownIssuer`.
8. **The re-issued certificates.** The node's current key is re-issued after its newest issuance, every unrevoked
   earlier key is re-issued, and no revoked key is. After the
   overwrite of `cert_record_key(node, key)`, the identity history, the revocation standing and the `anchors_seen` rank
   are as before, which is measured. `rotate_identity` is still refused on the off-node posture.
9. **Caches re-judged.** After each change of roots, a long-running node and a freshly restarted one agree on every
   node's anchors and revocation count, and the KV arm agrees with the fold.
10. **Old-CA records lose standing.** Before narrowing, an old-CA record verifies. After it, the record no longer
    verifies and its anchor is gone, on a long-running node and a restarted one alike. A revoked old key is refused at
    the handshake and at a signature check. A signature by an unrevoked earlier key still verifies, through its
    re-issued record.
11. **The other verifiers.** The bulk fetch and the node-certificate gateway work under the successor after cutover.
12. **Removal and revocation.** A removed member and a revoked key are refused under either root during the window.
13. **The profile.** Under the secure profile, `id.ca_succession_committed` is reported, with rev 6 and rev 2. A
    node under `secure-single-domain` rev 6 with no commitment (`ca-commit.pem`) refuses to start, naming
    `id.ca_succession_committed`.
14. **No silent drop.** A multi-certificate `ca_cert_pem` is refused by name, with its upgrade note; a chain in
    `cert_pem` is unaffected.

**Not built, stated:**
- a successor issued as an intermediate under the old root;
- a mixed-version fleet mid-succession: every member is upgraded before `tls commit`, because an old binary cannot widen
  live;
- a `[gateway_tls]` gateway on its own certificate, which is outside the fleet CA and unchanged;
- rotation in place on the off-node posture.

#### I7 · The developer's identity — *written after S-presence* (D7)

Decided: the key never touches the worktree; `signed_with` graded on the record; the prompt renders from the fleet's
record by id; a bounded request queue (`busy`). **From the spike:** whether the signer is a P-256 fleet certificate
(a second algorithm through every verifier, gated) or a locally-trusted signer with its own verifier; whether `dev` can
ship as a signed bundle; Windows's grade. N4's identity half and N7 wait on this row; N4's fleet and wiring halves do
not.

### Wave 2 — the developer surface (one MINOR)

#### N3 · `--format json` for `explain`, `declare`, `tls`

`mycelium.<cmd>/1` *(new)* for the three; `explain --closure`'s `mycelium.closure/1` kept; goldens under
`tests/fixtures/cli/` *(new)*. **Exit:** every subcommand's JSON parsed against its golden.

#### N5 · The agent guide the binary serves

`mycelium skills get fleet` *(new)*; a root `AGENTS.md` *(new, ≤ 2 KB; distinct from `docs/wiki/AGENTS.md`)* for Codex,
Cursor and Orca's workers; **the `CLAUDE.md` line is load-bearing** (Claude Code reads a root `AGENTS.md` only without a
`CLAUDE.md`). **Exit:** the guide's commands are the `one-hour` job's, by test; the size gated.

#### N4 · `mycelium dev --isolate <name>` — the fleet and the worktree's wiring now; the identity after I7

**Fleet:** N nodes from a declaration directory, kept up, re-declaring on change; an injective isolate encoding;
the banner names the `CARGO_TARGET_DIR`; `[authority]` built for the fleet with `gateway_identity_issuer = "dev-<isolate>"`.
**Wiring:** `dev` writes `.mcp.json` (Claude Code; project-scoped approval remembered and resettable), `.cursor/mcp.json`
(Cursor; Auto-review by default) and `.codex/config.toml` `[mcp_servers.mycelium]` (Codex; trusted projects only), each
with a **separate low-scope bearer** (`mcp:invoke`, `grant:read`, `grant:relay`); an existing file merged, an existing
`mycelium` entry refused by name. Orca writes a starter `.mcp.json` only on a click in its settings and never
overwrites; it writes no project Codex or Cursor file. **Identity:** after I7 — until then `dev` issues no developer
certificate, and a decision in a dev fleet is made by a service principal (I1) the developer runs. **Exit (now):** two
fleets on one host isolated; colliding names refused; re-declare without restart; a Claude Code worker lists the tools
after its approval; the low-scope bearer signs nothing. **Not built, stated:** a fleet across hosts; the developer
identity (I7).

#### N1 · `mycelium mcp`

A stdio MCP server over the gateway routes (`doctor`, `explain`, `wire_check`, `declare`, `grants_pending`,
`grant_decide`, `grant_revoke`, and `cancel_call` after I4) with the low-scope bearer; decision tools relay a signed
body. **Depends on:** N3, I1, I3. **Exit:** a decision relayed with the supervisor's signed body, the journal naming the
supervisor; a fabricated body refused; CI against a node.

#### N6 · `mycelium watch --format json`

One stream: grants, `declare --watch` findings, `doctor` changes. **Depends on:** N3, N4. **Exit:** the three kinds
in a node-free test.

#### N7 · The Orca plugin — *after I7*

A view over routes, holding no key: `workspace.created`/`removed` (the current names) to offer `dev` via
`terminal:send` — a command channel, which is why consent is not a terminal; a card per `grant_request` from `watch`;
the decision requested from `dev`, consent at the OS (I7), the record's `signed_with` grade shown; `explain` in the
panel. Orca facts at `981ad63b`: capabilities `workspace:read`, `agent:read`, `terminal:send`, `notifications:show`,
`storage:own`, `secrets:own`, `settings:own` (`storage`, `secrets`, `events:subscribe` are aliases); **no network
capability**, so the plugin's HTTP is ungated and stated; **plugin notifications reach phones through the push
gateway**; a manifest declares `engines.orca` as a **floor**, not a pin, so the plugin names the floor it was tested at
and the exact release of its recorded run; the catalog label for a non-official plugin is "Unverified" or
"`<catalog> · <owner>`". **Exit:** on the recorded release, a workspace gets a dev fleet from the offer; a card within
the stream's latency; approval only after the OS prompt with `os_presence(macos)` on the record, `tty` elsewhere and
said so on the card; a plugin typing into `dev`'s terminal produces no signature on macOS; the Orca half *run when
supplied*.

### Examples and documentation (no new mechanism)

| Id | Item | Depends on | Exit |
|---|---|---|---|
| **E1** | **The Eve supervisor** — an Eve agent on Slack/Teams cards consuming N1's tools, signing with its own client certificate (I1; `signed_with = service`) | I1, I3, N1, Y6 | the flow against a local Eve channel in CI; the card *run when supplied* |
| **E2** | Re-pin Y6 to Eve 1.0.0 | Eve 1.0.0 | `eve-fleet` green |
| **E3** | Y12's *not built* names Eve's position | — | the sentence |
| **E4** | The wiki comparison record and the plan's five reviews — recorded in #715's thread, this plan's status paragraph and the *Revised from* column, nowhere else today — as a dated example of rule 4 applied to a plan, with D8 as the lesson | — | the dated entry |
| **E5** | An Orca plugin — N7 | — | — |
| **E6** | `examples/orca_worktree/` — the walk in a `--features sim` test build with the consent recorded (after I7; before it, with a service principal deciding), the Orca run recorded | N1, N4, N7 | the `sim` walk green in CI |

## 4 · Sequence and release shape

```
Wave 1a (identity for services, one MINOR):  I0 ──> I1 ──> I3 ──> I5
Wave 1b (spikes, in parallel with 1a's PRs):  S-cancel ──> I4 · S-succeed ──> I2 · S-presence ──> I7   (one MINOR)
Wave 2 (developer surface, one MINOR):        N3, N5 ──> N4 (fleet, wiring) ──> N6 ──> N1 ──> E1
                                               I7 ──> N4 (identity) ──> N7 ──> E6
Any time:                                      E2, E3, E4
```

## 5 · Upgrade windows and behaviour changes

- **I0:** none without `[authority]`; with it, doors that enforced nothing now refuse; a build without `compliance`
  refuses the start by name; **an older binary ignores the table silently** — upgrade before relying on it.
- **I1:** `cs`/`cn` optional under `v: 1`, safe both ways; a certificate-holding principal's unsigned action counted then
  refused (counted, as built: at the `/mcp` `tools/call` door, both `/a2a` doors and the provider of an `mcp.invoke` or
  `skill.invoke`; the KV routes, the raw RPC routes and `ServiceHandle::gateway_rpc_call` count nothing); **key-less principals lose action scopes at the refusal release** (`oidc:`, `anonymous`, the legacy token;
  `/mcp`, KV, `/a2a`); `identity:write` is an existing scope, so a token holding it for `/gateway/identity/revoke` gains the client-cert
  door at the upgrade — reissue narrower tokens first; `ExecutionAuthority::new` takes the shared `TrustedExternalIssuers`
  handle (a caller breaks); `sys/client-cert/` ignored by an un-upgraded node (shown); a
  `mycelium-client://` certificate refused at the mesh from this release; pre-row-Q node certificates unaffected.
- **I3:** an unsigned decision body counted then refused; `grant:relay` inert at an un-upgraded gateway.
- **I5:** `ot` optional; the provider's egress list must admit the issuer; a provider without `compliance` refuses a call
  carrying `ot` as `not_in_build`; an envelope past 8 KiB is refused `caller_context_too_large`, as today.
- **I2:** a `ca_cert_pem` holding more than one certificate is refused by name at start. **Before upgrading, an operator
  whose `ca_cert_pem` holds an extra certificate removes it**, keeping only the fleet CA's: the extra certificate was
  ignored before, so removing it changes nothing. Only `ca_cert_pem` is affected, not `cert_pem` or the on-node CA, and
  the upgrade note says so (`deprecations.md`). The successor-issued certificate and the phase live under
  `auto_cert_dir/succession/`, and the operator's files are never written. Every member is upgraded before `tls commit`.
- **I4, I7:** written with their rows.
- **N4:** three configuration files written into the worktree, merged.
- Wire **v12** unchanged throughout: every field is payload (`cs`, `cn`, `ot` park in `x` at an older provider).

## 6 · Verification

1. **Wave 1a goes to the independent review as a buildable document**; the reviewer is asked whether any premise of
   I0, I1, I3 or I5 fails against the code, and whether the one-of-two scope shape, the client-cert fold's
   judgment by signature and issuance, and the signature vector are specified enough to build.
2. **Wave 1b's rows are not reviewed until their spike's note exists**; the note is reviewed with the row.
3. Every row: the fail-first test, the enumeration, the review before merge, the feature page and registry entry, the
   CHANGELOG and upgrade note.
4. Measurements that are run, not asserted, dated on [`what-is-proven.md`](../operations/what-is-proven.md): I1's
   handshake refusal; S-cancel's latency; S-succeed's live re-root; S-presence's probe.
5. The private Cedar adapter re-pins on I3 and, later, I4.

## 7 · Not in scope, by decision

- A model-in-the-loop approval (Eve's `auto()`); Orca as a runtime dependency; Eve as a compatibility promise (D24).
- M-of-N approvals; keyed federation principals; Linux and Windows user presence beyond `tty`; a fleet-wide rule set.
- A task API in the substrate (I4 is narrower, §2).
- A decision signed by an OIDC assertion; a forwarded OIDC bearer.

## 8 · Evidence

Filled as rows close: each promise quoted beside the code and the test that deliver it, or *not built*.

| Row | Promises, and what delivers each | PR |
|---|---|---|
| I0 | *"`[authority]` (new config table): the declaration directory, `unit` … the evidence journal's path, the tier, the freshness bound and the entitlement rule"*: `AuthorityConfig` in [`mycelium-core/src/config.rs`](../../mycelium-core/src/config.rs) — `declarations`, `unit`, `journal`, `tier`, `freshness_ms`, `entitle_grants` (`node` · `fleet`), `scope`, and **`skew_ms`**, which the row did not name: the strict gate's clock model needs a skew, and the node's `max_clock_drift_ms` (300 s) made every freshness bound under 20 minutes unbuildable; `deny_unknown_fields` (`an_unknown_key_in_the_table_is_refused_at_load`). *"builds the reference evaluator from that unit's `[[rule]]`s only … keeps the bare form for this unit and loads no other unit's rules"*: `plan` in [`src/agent/declared_authority.rs`](../../src/agent/declared_authority.rs) — each rule as written, a bare `tool:x` also at `tool:x@<node>`, a grant-requiring rule on the unit's own principal also for `node:<id>`; a second unit's rule admitting the refused call admits nothing (`the_gateway_door_decides_by_this_units_rules_and_records_each_decision`). *"the operation map covering every door the binary serves (an unmapped door refuses the start by name)"*: `evaluated_doors` ([`src/agent/http/mod.rs`](../../src/agent/http/mod.rs)) and the provider's protected kinds (`doors`), checked by `check_doors` (`a_door_the_evaluator_leaves_unmapped_is_named`); the take, claim and emit doors are not listed — they ask only when an exposure rule selects, and the table attaches no exposure policy. *"constructs the authority and attaches all three with `with_action_evaluator`, `with_execution_authority`, `with_provider_enforcement` and `with_evidence_journal`"*: `attach`, called by `start()` after `preflight()` and before the lifecycle boundary ([`src/agent/lifecycle.rs`](../../src/agent/lifecycle.rs)), the journal first. *"A build that cannot enforce the table (no `compliance`) refuses the start by name"*: `preflight()`; `an_authority_table_without_compliance_refuses_the_start_by_name` (the default-feature lib run). *"an older binary ignores the table silently and the upgrade note says so"*: `deprecations.md` §83. *"`doctor` reports `ae.*`, `prov.enforcement` and the journal"*: the doctor attaches the plan without opening the journal, and `ae.recorded_before_dispatch` reads the declared journal as enforced on a node never started (`the_doctor_reports_what_authority_would_attach_and_opens_no_journal`, which also shows no journal file created). **Exit:** *a call no rule permits refused at its gateway door … journaled* — the capital-ledger call, refused before the tool runs and recorded *indeterminate* (no allowance matches; the reference evaluator denies only under a prohibition, and indeterminate is never a permit); *a call under a revoked grant at its provider door, journaled* — `the_provider_door_refuses_a_call_under_a_revoked_grant`, the refusal recorded at `provider` with the grant's hash; *`no_evaluator` never appears* — asserted on every answer and record of both; *a second unit's rule … does not admit* — above; *an unmapped door, an empty directory, an undeclared unit, and a build without `compliance` each refuse the start by name* — `a_door_the_evaluator_leaves_unmapped_is_named`, `each_unbuildable_declaration_refuses_the_start_by_name` (also: a missing directory, two units with one principal, an exposure rule, a gate the strict profile refuses, an evaluator already attached in code, a journal that does not open, the advisory tier), and the no-`compliance` test. Seen failing first: with `attach` planted to attach nothing, the capital-ledger call ran, the grant door answered `no_evaluator`, the doctor read `ae.*` as not configured, and an unopenable journal started. **Found on the way:** the reference evaluator selected no arguments, so a `requires_values` rule never decided at a live door (CHANGELOG *Fixed*). **The scaffold:** `governed-call`'s `node.toml` now fixes its gateway issuer (`gateway_identity_issuer = "governed-call"`) and its unit names the treasurer `token:governed-call/treasurer` with a `grant.revoke` rule — the revoked-grant test runs over the scaffold's unit unchanged (seen failing on the scaffold as it was: the issue refused at the grant door), and `the_scaffolds_node_toml_with_an_authority_table_loads_and_decides_through_the_config_path` loads its `node.toml` through `GossipConfig::load_from_file` and `validate()`. *"the binary over the `governed-call` scaffold"*: CI's scaffold job builds the binary with `cli,tls,compliance,llm`, runs `mycelium doctor` over the scaffold's `node.toml` with `[persistence]` and `[authority]` appended, and starts the binary over it, checking it logs the attach and opens the journal; the doors' decisions over the binary are **not built** — the bare binary serves no tool of its own, so a call reaches no handler, and the door refusals are shown by the lib tests over the same unit and configuration path. **#733's review, round 1 (each seen failing first under a plant, one build, one variable per plant):** one source of authority on every `preflight` — an evaluator, an execution authority or a journal attached in code refuses (`an_authority_attached_in_code_beside_the_table_refuses_the_start`; a second `preflight` reads the table's own as its own, `a_second_preflight_reads_the_tables_attachments_as_its_own`); durable epochs under `<persistence>/<node>/authority/`, and `secure-single-domain` met from the table (`the_table_meets_the_secure_profiles_authority_requirements`); `[persistence]` required; the doctor checks the journal's path and sets the plan in memory without opening, spawning or `mark_started`; a bare `skill:` qualified to this node (`a_bare_skill_resource_is_qualified_to_this_node`) and the node scope stated; the second-unit rule on `*`, so the test fails if another unit's rules load; `skew_ms` = 0 refused and the bound *s* ≤ *F*/6 stated with its assumption; a prohibition's named value decided as written — present and equal denies, absent is `Indeterminate`, present and different leaves the allowances to decide (the round's first reading, a different value refusing, made every `over_ceiling = true` deny rule refuse every order — the co-op's `procurement_authority` demo in CI — and was corrected; `a_prohibition_whose_value_is_present_and_equal_denies`, `…_is_absent_is_indeterminate`, `…_is_present_and_different_does_not_apply`). **Not built, stated:** the doors' decisions over the binary itself (above); the epoch floor on a table-only node — no route or configuration installs a newer epoch, so its gate stays at epoch 1 and the durable epochs hold only that floor (an embedder installs one with `install_epoch_durably`); other units' rules; the Cedar evaluator from configuration (private); a gate spanning more than one `ResourceAuthority`; exposure rules (refused at start); mandate authorities' keys (`TrustedExternalIssuers` empty until I1); tools and skills on another node. | #733 |
| I1 (first half) | *"`mycelium tls issue --client <name>`: an Ed25519 certificate with SAN `mycelium-client://<issuer>/<name>`"*: `issue_client_cert` / `generate_client_cert` in [`mycelium-core/src/tls.rs`](../../mycelium-core/src/tls.rs), reached by `mycelium tls issue --ca-dir <dir> --client <name> --issuer <issuer> --out <dir> [--force]` ([`src/main.rs`](../../src/main.rs) `tls_issue_client`) — **a departure:** the row's flag alone cannot write the SAN, which names the issuer, so `--issuer` is required beside it; the serial is the issuance in ms (`issuance_after`), the SAN holds the client URI only, and a segment is 1–128 of `A-Za-z0-9-_.` (`valid_client_segment`; `a_client_identity_is_read_exactly_and_bad_segments_are_refused_at_issue`); replacing a key file needs `--force`. *"Both mTLS verifiers refuse the scheme (seen failing first)"*: `refuse_if_client` after the chain and removal checks in `RemovalAwareClientVerifier::verify_client_cert` and `RemovalAwareServerVerifier::verify_server_cert`; `a_client_certificate_is_refused_at_the_mesh_both_ways` — seen failing first with both calls planted out (*"ADMITTED: a client certificate dialled the mesh: Ok(())"*); the serving case uses a CA-minted client certificate that also names an IP, because an issued one already fails the name check. The bulk verifier inherits the refusal: a `General` error is final there, never re-tried against `bulk_ca_pem_path`. *"a pre-row-Q node certificate naming no URI stays admitted"*: the same test, both a node certificate and `legacy_cert` admitted. *"`verify_member_cert` … must not apply the scheme refusal"*: unchanged, asserted in the same test. Measured against the binary: [`what-is-proven.md`](../operations/what-is-proven.md#client-certificate-refused-at-mesh). *"`POST /gateway/identity/client-cert` … under `identity:write` … `compliance`-gated"*: `gw_identity_client_cert` ([`src/agent/http/compliance.rs`](../../src/agent/http/compliance.rs)), the scope map in `authz.rs`, audited by `audit_govern`; `the_client_cert_door_is_identity_write_and_publishes_what_it_judged` (403 without the scope, 422 `not_a_client_certificate` for a node certificate, 200 and sealed). The door refuses a certificate from another CA (`not_issued_by_the_fleet_ca`), one issued past the clock plus the skew bound, and one older than the principal's newest already read (`409 superseded`) — `the_publish_refuses_a_node_certificate_a_foreign_ca_and_a_superseded_key`. *"`sys/client-cert/{principal-hex}/{key-hex}` (new namespace, absent from `SELF_OWNED_SYS_PREFIXES`)"*: `CLIENT_CERT_PREFIX` / `client_cert_key` ([`src/agent/client_cert.rs`](../../src/agent/client_cert.rs)); the raw KV routes already refuse it under `sys/` and now name this door. *"the fold judges by … the certificate's CA issuance — `judge_cert`'s shape — never by KV LWW time"*: `judge` — `client_identity` for exactly the path's principal and key, `verify_member_cert`, issuance no later than wall clock + `max_clock_drift_ms` (300 s when 0), counted in `future`. *"a high-water mark of the newest verified issuance … in memory and in the node-local journal … a certificate at or below the mark is refused for good"*: `ClientCertLedger` (papaya `compute`, retry-safe, the mark only rises; ties on the greater key, `Mark`'s order); the journal `identity/client-certs.journal` opened at start under `[persistence]` (a journal that does not open refuses the start), budget `JOURNAL_PER_PRINCIPAL` 64 / `JOURNAL_TOTAL` 65 536 records — 64 supersessions at a reading node, about 32 at the publishing gateway, which journals `Raised` and `Published` for each — a saturated principal recorded and warned. The fold runs on arrival (`kv_subscribe_prefix` on the prefix, `refresh`), at start, at publish and at every query. *"a superseded client key refused once the newer record is read, and still refused after that record is overwritten and, under `[persistence]`, after a restart"*: `a_newer_certificate_supersedes_the_older_key_and_an_overwrite_cannot_lower_it` (overwrite with garbage, then tombstone), `a_record_written_by_a_member_supersedes_on_arrival_before_anyone_asks`, `a_supersession_survives_a_restart_under_persistence_with_the_newer_record_gone` (plant inside: the journal removed brings the older key back). Seen failing first: with the mark allowed to fall, all three failed (*"SUPERSESSION LOWERED"*, *"RESTORED"*); with the journal not opened, the restart test failed. A tombstone of the newest record does not retire its key — only a newer certificate does (stated). Every record of a principal is judged, uncapped: `junk_records_sorted_ahead_cannot_hide_a_newer_certificate` (seen failing first with a 64-record cap, which let a member's junk sorted ahead of the newer record keep it from ever being judged); junk is refused before the chain check, and a genuine record exists once per issued certificate. **Not built in this PR, the row's second half:** `TrustedExternalIssuers` shared at the three holders and `ExecutionAuthority::new`'s new signature, with its lock-order row; `gateway_actions_unsigned{kind}` and the refusal of a certificate-holding principal's unsigned action; refusing a client's call at a gateway without `gateway_identity_issuer`; `cs`/`cn` (D1′) in `frame_with_context_at` and `verify`, and the `tests/fixtures/cs/` vector in Rust, jest and pytest; the `grant/` key change for client holders and every reader of it, `revoke_grant`'s `tool:{tool}@{provider}`, `POST /gateway/grants/request`; the SDK `tools/call` clients and `cs`/`cn` in both A2A clients; the exit's three-node fleet of published binaries; and the exit's *an un-upgraded node's reading of `sys/client-cert/` (shown against the previous binary)* — not shown, and #741's review found that a 2.39 node admits a client certificate at the mesh as a URI-less certificate, so `deprecations.md` §86 says to upgrade every node before issuing one. **#741's review, round 1:** a certificate with no readable issuance (a serial over 8 bytes, as `openssl` writes; read before as issuance 0) is refused by name at the door (`no_readable_issuance`) and ignored as a record — `a_client_certificate_with_no_readable_issuance_is_refused_by_name`, seen failing first; the issuing host's clock orders certificates, so a host whose clock runs behind can issue one that orders below its predecessor (stated). `refresh` makes one store scan grouped by principal and checks each distinct certificate's chain once, cached by digest (`cert_facts`' shape, bounded, cleared when full), single-flight as the revocation fold is — `many_junk_principals_cost_one_scan_and_one_chain_check_per_certificate`, seen failing first (151 scans; 150 chain checks again). The publishing gateway republishes its records (in memory, and journaled under `[persistence]`), as `revocation.rs` republishes a node's own — `a_node_joining_later_reads_a_tombstoned_newer_record_back`, seen failing first; a record whose publisher is gone for good and which every node has lost is not restored (stated). `IssuedInTheFuture` is covered by `a_certificate_issued_in_the_future_is_refused_at_the_door_and_ignored_as_a_record`. The page says the issuer must be set explicitly (the default node id holds a `:`). **Follow-up, recorded:** the revocation fold's `nodes_with_records` / `fold_node` rescans the store per node, the same cost shape as `refresh` had; not changed here. **Residual, stated:** the cache holds 4 096 certificates, so past that many crafted certificates it churns and each `refresh` re-checks every chain — the cost is bounded by the store, not by the cache; and `refresh` still runs inside the identity watcher's loop, so a large client-cert fold delays the revocation fold behind it — moving it to its own task is a follow-up. A cached admit outlives the certificate's `notAfter` until the cache churns, the same gap as `revocation::cert_facts` (this tree issues to 2099). **#741's confirm round:** a `Published` journal entry is appended only when the held record changes, inside the same 64 / 65 536 budget as `Raised` (`repeated_identical_publishes_leave_the_journal_bounded`, seen failing first: 557 → 24 507 bytes); the publisher leaves alone a stored certificate for the same key that the CA issued at or after its own, so two gateways holding the key's two certificates do not rewrite each other (`a_later_certificate_for_the_same_key_is_not_rewritten_by_the_publisher`, seen failing first); on load the journal keeps the newest published record by (issuance, key), not the last appended (`the_journal_keeps_the_newest_published_record_whatever_the_append_order`, seen failing first). **#741's final confirm:** the publisher's skip test is the fold's `judge`, skew bound included, so a far-future same-key certificate planted at the path is rewritten (`a_far_future_certificate_for_the_same_key_is_rewritten_by_the_publisher`, seen failing first). **Questions for the second half (#741's review, item 7):** the door does not check the certificate's issuer against this gateway's `gateway_identity_issuer`; any `identity:write` bearer can publish for any principal; the audit record names the principal, not the bearer who published; a client named `legacy` would collide with the legacy token's principal; journal appends are fire-and-forget, so a crash right after a supersession can lose its record. The fold answers *current*/*superseded*/*unknown*; nothing consults it at a call yet, because no call carries a client key until `cs` exists. | this PR |
| I1 (second half, PR 1 of 3) | **The split, stated:** PR 1 (this) is the client's own signature end to end; PR 2 is the SDK `tools/call` clients and `cs`/`cn` in both A2A clients, grants held by a client principal (the `grant/` key and every reader, `revoke_grant` evaluating `tool:{tool}@{provider}`, `POST /gateway/grants/request`), and `TrustedExternalIssuers` shared at the three holders with `ExecutionAuthority::new`'s signature and its lock-order row; PR 3 is the three-node published-binary exit run. *"`cs`/`cn` (D1′): `frame_with_context_at` writes them"*: the envelope gained `cs` and `cn` beside `m`, outside the gateway's signature (they authenticate themselves), written by `frame_with_client_signature_at` ([`src/agent/gateway_caller.rs`](../../src/agent/gateway_caller.rs)), which `frame_with_context_at` now calls; the `/mcp` `tools/call` door and both `/a2a` doors (send and stream) read `params._meta.cs`/`cn` (`client_sig::at_door`) and carry them; the LLM doors bind none. *"`cs` covers the operation, the resource, the arguments' digest over the canonical form … the client's `issued_at` and `cn`"*: `client_signature_message` ([`src/agent/client_sig.rs`](../../src/agent/client_sig.rs)) — the tag `mycelium.gateway/client-signature/1`, then the **principal** (not in the row: without it a signature could be presented under another bearer), the operation, the resource before `@`, the digest, `t`, `instance`, `counter`; a `tools/call` signs `("tools/call", "tool:{name}", digest(arguments))`, an A2A send `("skill.invoke", "skill:{id}", digest({"text": …}))` — the operations the preflight already uses. *"one new vector pins the signature"*: `tests/fixtures/cs/vector.json`, checked by `the_client_signature_vector_is_the_same_in_every_language` (Rust), `client_sig.test.ts` (jest) and `test_client_sig.py` (pytest, with the new optional `cryptography` extra `[sign]`, installed in CI's pytest jobs); Ed25519 is deterministic, so the signature is pinned as well as the message. *"`verify` checks `cs` before the cache"*: in `verify`, after the gateway's signature and freshness and before `verify_received`'s windows — the binding read from the request (`binding_of`: a `tools/call` payload's name and arguments, checked against the envelope's resource claim; a skill's text and the claim's skill; any other kind refuses a signature, `client_signature_unbound`), the signature (`client_signature_invalid`), the key the principal's **current** client key by `client_key_standing` (`client_key_unknown`, retriable, for a key with no certificate here; `client_key_superseded`, final), then the client's issue time on the envelope's window (`stale`). *"`instance` a random 64-bit id the process draws once … the Rust client draws `instance` through the seam, the SDKs from `os.urandom` / `crypto.randomBytes`"*: `ClientSigner::new` draws through `sim_seam::rng_u64_from`; `mycelium.ClientSigner` and the TypeScript `ClientSigner` from those two. *"a new window type keyed per (principal, certificate key) … eviction-only floor per instance"*: `ClientReplayWindows` — keyed principal → key → instance, each instance its own `(t, c)` set and floor, 65 536 per instance; a new instance past 1 024 per key is refused, answered and counted (`client_instances_exhausted`, round 1) — **a bound the row did not state**, since only the key's holder can mint instances; lock-order row 76. *Exit:* *"a foreign-key `cs` refused"*, superseded and tampered too — `the_provider_checks_the_client_signature_binding_signature_and_key_standing`; *"a replayed `cn` refused and a reordered honest pair admitted; two processes under one certificate not colliding"* — `a_replayed_cn_is_dropped_and_reordered_and_concurrent_honest_calls_are_admitted` (a replay dropped unanswered, as an envelope replay is); *"the cross-language vector green in all three suites"* — above. *"A `token:` principal with a certificate on record and no `cs`: counted (`gateway_actions_unsigned{kind}`), then refused"*: counted at the `/mcp` and `/a2a` doors (`UnsignedActions`, `mycelium_gateway_actions_unsigned_total{kind}` under `metrics`); **the refusal is not built** — §5 says counted, then refused, so it waits for the refusal release (`deprecations.md` §87 announces it). *"A client's call at a gateway without `gateway_identity_issuer` is refused by name at call time"*: `at_door`, `-32602` `gateway_identity_issuer_unset`, before dispatch. The `/mcp` door end to end, a signed call reaching the tool with its verified signature (`GatewayCaller::client_signed`): `the_mcp_door_carries_a_client_signature_counts_an_unsigned_one_and_needs_an_issuer`. Seen failing first, one build per plant: the standing check out (*"FORGED"*), the signature check out (*"TAMPERED"*), a generic binding for every kind (*"UNBOUND"*), the window keyed across instances (*"COLLIDED"*), the window skipped (*"REPLAYED"*), the door's issuer check out — the call was dispatched and refused **at the provider** as `client_signature_invalid`, since the principal no longer matched the signed one (*"DISPATCHED"*), the count out (*"UNCOUNTED"*). **#741's questions for this half:** (1) the door now publishes only its own issuer's certificates — `409 gateway_identity_issuer_unset`, `422 issuer_mismatch` (`the_client_cert_door_checks_its_issuer_and_audits_the_bearer`, seen failing first: the foreign certificate was published); (2) any `identity:write` bearer may still publish for any principal of its issuer — stated, by design: the record's authority is the fleet CA's signature, and `identity:write` is an operator's scope; a per-principal publishing scope is not built; (3) the audit record now names the bearer (`by`) — the same test, seen failing with the field planted out; (4) a client named `legacy` is refused at issue and read as no client (`RESERVED_CLIENT_NAMES`, `a_client_named_legacy_is_refused_at_issue_and_read_as_no_client`, seen failing first) — a **named token** called `legacy` collides with the legacy token's principal the same way, which predates I1 and is not changed here (stated); (5) journal appends stay fire-and-forget, as the revocation ledger's are — a crash between a supersession and its append loses the journal record, which matters only if the newer certificate's record is also gone from the store by then (the store's own WAL replays it otherwise); stated, not changed. **#742's review, round 1:** (1) the arguments' digest a client signs is **RFC 8785 (JCS)** — `canonical_json` in Rust, `canonical_json` in Python, the existing `canonicalJson` in TypeScript (already JCS: ECMAScript numbers, UTF-16 key order) — because `serde_json` writes `1e-5`/`1e16`, Python `1e-05`/`1e+16`, JavaScript `1e+21`, so an honest Python call with a float was refused; the vector gained seven `canonical_cases` (exponent-form floats `0.00001`, `1e16`, `1e21`, `1e-7`, the largest double, non-ASCII and control text, keys sorted by UTF-16 code units at depth), checked by `the_canonical_arguments_are_the_same_in_every_language` in Rust, jest and pytest — seen failing first in Python (`1e-05`) and with Rust's digest planted back to `serde_json`. **The client digest is not shared** with the mandate/grant `arguments_digest`, which is unchanged; its readers, enumerated (`grep -rn "arguments_digest\|argumentsDigest\|args_digest"`): `gateway_authority::arguments_digest` (the possession request and a grant's binding check), `provider_enforcement` (the provider's possession check), `state_machine` and `http/grants.rs` / `http/authz.rs` (a grant's bound digest), `action_evaluator` (the envelope's digest, recorded as evidence), and the SDKs' `a2a.arguments_digest` / `agent.py` (Python's `json.dumps`) and `argumentsDigest` / `agent.ts` (JCS). **That digest has the same cross-language float defect** — Python and TypeScript disagree with the gateway on exponent-form floats — **recorded, not changed here**: fixing it changes what a grant's bound digest means on the wire, so it needs its own row and window. (2) The 1 025th instance is refused **with an answer**, `client_instances_exhausted`, and counted; `client_signatures_replayed` and `client_instances_refused` are on `/stats` (`the_instance_cap_is_answered_and_the_eviction_floor_refuses_at_or_below_it`, seen failing first with the cap mapped to a replay; the floor tested in the same test). (3) **One signature at N providers, stated as a limit:** the signature names the resource before `@` and each provider keeps its own window, so a signed call presented to every provider of a tool runs once at each — binding the provider would need the client to know which one the gateway resolves, which it does not; on the page. (4) An unsigned call for a certificate holder is also counted **at the provider** that runs an `mcp.invoke` or `skill.invoke` (`provider_actions_unsigned`, `an_unsigned_call_for_a_certificate_holder_is_counted_at_the_provider`, seen failing first); the LLM doors **refuse** a signature (`400` / an error event `client_signature_unbound`, `the_llm_doors_refuse_a_client_signature_rather_than_drop_it`, seen failing first: the call went on to `404 no_provider`); §87 now names the doors as built — the KV routes, the raw RPC routes and `ServiceHandle::gateway_rpc_call` count nothing. (5) Both `/a2a` doors with `cs`, the `skill.invoke` binding and the stream's refusal: `both_a2a_doors_carry_a_client_signature_and_the_skill_sees_it`, seen failing first with `None` planted at each door. (6) `_meta.cs`/`cn` are reserved, a client using them otherwise refused `-32602` — in §87. (7) The `cn` window is **memory only** (a capture replays once across a provider restart, inside the freshness window — stated); `client_key_not_current` is split into `client_key_unknown` (retriable while the record travels) and `client_key_superseded` (final); the window is keyed by principal, and an admission drops the principal's keys that are neither the signer nor current, so it holds one key's windows and a racing predecessor's (`a_principals_superseded_key_windows_are_dropped_when_its_successor_signs`, seen failing first); lock-order row 76 updated. **#742's confirm round:** (1) an integer outside ±(2^53 − 1) is refused at both ends — RFC 8785 writes it as the nearest double, so `9007199254740993` and `9007199254740992` digested alike while the tool received the exact integer, and a relay could swap a large id for its neighbour: `door_binding_mcp` / `mcp_binding` / `mcpBinding` and the three signers' `sign_tools_call` refuse it (`UnsafeInteger`, `ValueError`, a thrown `Error`), and the provider's `binding_of` refuses the call `client_signature_unsafe_integer`; any number whose magnitude is 2^53 or more counts, float or not (`1e16`, `6.022e23`), because every double of that magnitude is an integer and JavaScript cannot tell `1e16` from `10000000000000000`; the vector's `unsafe_integer_cases` / `safe_integer_cases` are raw JSON each language parses itself; `an_integer_beyond_2_53_is_refused_at_the_signer_and_at_the_provider` (Rust), `test_an_integer_beyond_2_53_is_refused_by_the_signer`, the jest case — each seen failing first; large ids go as strings (page, §87); (2) the window's wording above corrected; (3) the mandate/grant digest's defect has a tracked home, §9 **O1**. **Not built in this PR:** PR 2's and PR 3's rows above; the refusal of an unsigned call (the refusal release). | this PR |
| I1 (second half, PR 2 of 4) | **The split, restated:** PR 1 (merged, #742) was the client's own signature; this PR is client-held grants and the shared issuer set; **PR 3** is the SDK `tools/call` clients, `cs`/`cn` in both A2A clients and the SDKs' grant-request verbs with a possession-proof vector; **PR 4** is the three-node published-binary exit run. The SDK half moved out of this PR because its possession proof needs the mandate's canonical bytes in Python and TypeScript, with a vector of its own. *"`TrustedExternalIssuers` shared at `ExecutionAuthority`, `ProviderBudget`, `MembershipAuthorities` … the shared handle changes `ExecutionAuthority::new`'s signature (§5); a lock-order row"*: `SharedExternalIssuers` ([`src/knowledge/issuer.rs`](../../src/knowledge/issuer.rs)), a clone the same set; the three take `impl Into<SharedExternalIssuers>`, so a caller passing a `TrustedExternalIssuers` still compiles (the signature changed, the call sites did not); every reader snapshots the set before its own lock — lock-order row 77. *"(the last never entitles a client)"*: no removal verifies under a client key (`verify_signed_by` reads configured keys only, since review item 4 below) — `a_client_key_in_a_shared_set_never_entitles_a_removal`, seen failing first with the full set read. **Client keys reach a holder** by the client-certificate fold: `TrustedExternalIssuers` gained a per-principal current client key (`set_client_key`, replaced only by a newer issuance), and `feed_client_keys` writes each mark into every attached execution authority's set (`client_key_sinks`, registered by `with_execution_authority`) — after every fold and at attach. **A departure stated:** the provider's cohort budget shares the type but is not fed client keys — its authorities are the operator's. *"`grant/` keys: `node:` literal, client principals hex, readers decode by prefix"*: `grant_holder_segment` / `grant_holder_from_segment` ([`src/agent/gateway_authority.rs`](../../src/agent/gateway_authority.rs)); the readers, enumerated (`grep -rn '"grant/\|grant_checkpoint_key\|is_member_agent\|unreadable_grant'`): `grant_checkpoint_key` and the `…/used/{door}` marker in `authz.rs` now write the segment (both wrote the raw holder, which for a client would have split the key); `decide_grant` decodes the path segment, and a client holder needs a certificate on record (`not_a_member` otherwise); `revoke_grant` as below; `pending()`, the stream's `rsplit`, the state machine's `grant_key` and the faults reader see a segment and are unchanged (a node segment is unchanged, a hex one holds no `/`); `is_member_agent` is reached only for a node segment. *"a pre-upgrade `grant/node:…` record read unchanged"*: a node segment is written and read as before — `a_grant_holder_segment_round_trips_and_is_unambiguous`, and every existing grant test passes unchanged. *"`revoke_grant` on a client holder evaluating `tool:{tool}@{provider}` … resolved from the `tools/` scan as `/mcp` does"*: the provider is resolved from `tools/` (`tool_provider`) **when the request is written**, and the issue signs the resource it evaluated into the grant (`bound:resource:`, `Mandate::grant_resource`); the revocation evaluates that, never a live re-scan — a departure from the row's wording, made in review (below). *"A client request door `POST /gateway/grants/request` under `mcp:invoke`"*: `gw_grant_request` ([`src/agent/http/grants.rs`](../../src/agent/http/grants.rs)) — the caller must hold a client certificate on record (`409 not_a_client_holder`), the tool's provider is resolved, the record is the state machine's shape, written on disk or `503 not_durable`; **added beside it**, `GET /gateway/grants/request/{request}` (`mcp:invoke`), the caller's own request and answer, so a client need not read the `grant/` namespace. **Review round 1 (#743, independent):** (1) the revocation re-scanned `tools/`, so a departed provider left the grant standing and a member's `tools/` record could change the resource — it evaluates the signed resource, and refuses an `…/issued` this node did not sign (`a_client_grant_is_revoked_against_its_issued_resource_with_the_provider_gone`, two nodes, the provider gone; seen failing with the re-scan restored: `409 no_provider`); (2) the SDKs' `grant_decide(row.agent, …)` sent the principal, which the doors refused — the doors accept the principal as well as the segment (`grant_holder_from_path`; chosen over a `holder` field in the record because it changes two door functions and no reader, where the field changes both SDKs and the record writer), exercised through the SDKs' percent-encoded URL shape in the end-to-end test (seen failing segment-only: `400`); (3) concurrent feeds could write a superseded key back — a client key carries its issuance and is replaced only by a newer one, decided under the write lock (`a_client_key_proves_possession_only_and_a_superseded_one_never_returns`, the out-of-order feed planted; `a_possession_proof_by_a_superseded_client_key_is_refused` end to end); (4) the cohort budget read client keys, and the question whether a client key should verify a mandate authority's checkpoint is answered **no**: a client key verifies a possession proof and nothing else — `verify_signed_by` reads configured keys only and `GrantVerifier::check`'s possession step alone reads `verify_client`, so the budget, checkpoints, mandates, heads, records and removals are closed by one rule rather than a `configured_only()` per reader (seen failing with `verify_signed_by` reading client keys: a client-signed checkpoint accepted, and the membership test's removal verified); (5) the request door reuses a standing request for the same tool and digest, refuses past `MAX_PENDING_GRANT_REQUESTS_PER_CLIENT` (16) with `429 too_many_pending`, and seals each request into the audit chain under `compliance` (`the_request_door_reuses_a_standing_request_bounds_them_and_withholds_a_classified_rendering`, reuse and bound each planted); (6) a request id names its writer (`{hlc}-{node}`, planted); (7) one record builder for both doors (`grant_request_record`, `count_grant_requested`): the data-class rendering, the 16 KiB cap, the withheld shape, the policy revision and the counter — a client declares `arguments_class` and, having no agent policy, renders no class (planted); (8) the evaluator sees the holder's principal, a client's as a node's — not separately tested, the arguments being the evaluator's input and no shipped rule reading `agent`; (9) a sink is registered only for the authority attached (`a_second_execution_authority_registers_no_client_key_sink`, planted). **Round 2:** the signature check item 1 added read only the current key, so a grant issued before a `rotate_identity` could not be revoked — it reads every key the node has held (current, its identity record's history, its anchored keys), still refusing bytes none of them signed (`a_grant_issued_before_a_rotation_is_revoked_by_its_issuer`; seen failing with the current key only: `409 unreadable_grant`); the per-principal bound of item 5 is **soft**, stated on the constant and the page (the scan and the write are separate, and two gateways meet only by gossip, so a lock would not close it). *Exit:* a grant held by `token:fleet/forecaster` requested, issued at the supervisor's door, read back, presented at `/mcp` with a possession proof signed by the client key, run once, and a second grant revoked before use refused when presented — `a_grant_held_by_a_client_principal_is_requested_issued_presented_and_revoked`, on a node whose authority is its declaration; a principal without a certificate neither asks nor is issued — `a_principal_without_a_client_certificate_neither_asks_for_nor_is_issued_a_grant`. Seen failing first, one build per plant: the client keys not fed (the presented grant was refused), the holder written raw (the request never reached the pending list), the old node-only revocation (`unreadable_grant`), the decision's certificate check removed (`unknown_request` instead of `not_a_member`), the request door's certificate check removed (the request was written), and membership reading the full set (a client-signed removal verified). **Not built in this PR:** PR 3's and PR 4's items above; the refusal of an unsigned call (the refusal release); the mandate digest's cross-language defect (§9 O1) — a client-held grant binds the same digest, so a Python or TypeScript client with an exponent-form float in its arguments is refused until O1 lands. | #743 |
| I1 (second half, PR 3 of 4) | The SDK half of the split PR 2 stated. **Enumerated** (`grep -rn -e '/mcp' -e '/a2a' -e 'tools/call' -e 'tasks/send' -e 'message/send'` over both SDKs and the two companions): the invoking doors are `A2aClient.send` / `stream` in each SDK (`tasks/send`, `tasks/sendSubscribe`, `params._meta`); `get_task` / `cancel` invoke nothing and bind no signature; neither SDK had a `tools/call` client (`eve.ts`'s `tools/call` is a server's); the companions call neither door. *"SDKs: a `tools/call` client (new in both) … carry `cs`/`cn`"*: `McpClient` ([`mycelium-py/src/mycelium/mcp.py`](../../mycelium-py/src/mycelium/mcp.py), [`mycelium-ts/src/mcp.ts`](../../mycelium-ts/src/mcp.ts)) — `call_tool` / `callTool`, `list_tools` / `listTools`; with a `signer` each call carries `params._meta.cs`/`cn` (`mcp_binding`, so an unsafe integer raises before anything is sent); the evaluator's three refusals raise `ActionRefusedError`, any other JSON-RPC error `McpError`. *"and the A2A client carry `cs`/`cn`"*: `A2aClient(…, signer=…)` / `new A2aClient(url, { signer })`, every `send` and `stream` signed over `a2a_binding(skill, message)`. **The grant verbs** (from PR 2's door): `request_grant` / `requestGrant`, `grant_status` / `grantStatus` (the request id one percent-encoded segment — it holds the writer's `:`), and `wait_for_grant` / `waitForGrant`, which raise `GrantNotIssuedError` naming the state. **The possession proof**: `mandate_canonical_bytes` / `mandateCanonicalBytes`, `mandate_digest`, `possession_message` and `ClientSigner.prove_possession` / `provePossession`, refusing a grant another principal holds; `call_tool(…, grant=…)` proves and presents. **The cross-language vector** `tests/fixtures/possession/vector.json` — a grant as the gateway serves it (a non-ASCII issuer in its purpose, four operations including `bound:resource:`) and a proof for one `tools/call` — was written by the Python implementation and is checked by the gateway's own code (`the_possession_vector_is_the_gateways`: canonical bytes, digest, request bytes over the gateway's arguments digest, possession message, both signatures, and `GrantVerifier::check` → `Valid` with the holder's key fed as a client key), by jest and by pytest. Tests: `mycelium-py/tests/test_possession.py` and `mycelium-ts/tests/possession.test.ts`, six each — the vector rebuilt; the proof the vector's and refused for another holder; a `tools/call` signed and its grant presented, each signature verified as the provider would; an unsigned client sends no `_meta`, an unsafe integer and a grant without a signer refused before sending; the grant verbs' bodies and paths; an A2A send and stream signed. Seen failing first, one run per plant, in each SDK: the canonical bytes without the operation count, the `tools/call` unsigned, the possession proof over other arguments, the A2A send unsigned, the wait returning a refusal. **Review round 1 (#744, independent; the reviewer ran both SDKs' clients against a live gateway, and the wire shape held):** (1) TypeScript's possession proof digested the arguments with the JCS form the client signature uses — members by UTF-16 code units — where the gateway's mandate digest is `serde_json`'s, by UTF-8 bytes, so a key outside the BMP beside one in U+E000–U+FFFF was refused; and an `undefined` member was digested though `JSON.stringify` drops it from the body. The proof now binds `mandateArgumentsDigest` (members by UTF-8 bytes, what the body omits omitted); `argumentsDigest` stays JCS for `cs`. Python's `arguments_digest` was checked and already matched: code-point order is UTF-8 byte order, and `json.dumps(ensure_ascii=False)` escapes the short forms and other controls as `\u00xx` lowercase, as `serde_json` does, leaving DEL and U+2028 raw. The vector gained `mandate_digest_cases` — non-BMP beside U+E000–U+FFFF keys, non-ASCII, control characters, nesting — checked by the gateway's own `serde_json::to_string` and `arguments_digest` in Rust, by jest and by pytest. Seen failing first: the proof over the JCS digest, members by UTF-16, `undefined` kept (jest), and members by UTF-16 in Python (pytest; Python needed no fix). (2) The Python counter is taken under a lock (`two_threads_never_sign_with_one_counter`; under the GIL the race does not show, so this test was not seen failing). (3) Both READMEs say how refusals reach the caller and which to retry. (4) **Stated, not changed:** a possession proof that does not verify is `WrongHolder` whether the key or the digest differs — the gateway sees only the proof, so naming the O1 case would need the client's arguments rendered both ways at the verifier. (5) `wait_for_grant` / `waitForGrant` bound each read and pause by the time left (`a wait ends within its timeout even when a read is slow`, seen failing with the read unbounded, in each SDK). (6) `a_signed_call_presenting_a_client_grant_runs_at_mcp`: `cs`/`cn` and a client grant in one `_meta`, as the SDKs send them, run once; a second grant with a signature over other arguments is refused `client_signature_invalid` and does not run (seen failing with a signed call refused at the door). **Found by that test, stated:** the gateway claims a grant's use at its preflight, before the provider checks `cs`, so a call refused for its signature has spent the grant at the gateway. **Review round 2 (#744):** live, `{"！":1,"😀":2}`, escapes and nesting are admitted; a TypeScript call whose arguments held an `undefined` member was refused `client_signature_invalid` (and spent its grant) because the `cs` digest's `canonicalJson` wrote `"gone":undefined` where the body drops it — both TypeScript digests now share one serialiser that sends only what `JSON.stringify` sends (`undefined`, function and symbol members dropped, `null` in an array, `toJSON` honoured), JCS order kept for `cs`; `canonicalJson`'s one caller is `argumentsDigest` (through `mcpBinding`, `a2aBinding` — a string — and the vector tests), none relying on the old form. Jest: the digest of the arguments equals the digest of the round-tripped body for a top-level, a nested and an in-array `undefined` and a `toJSON`, and the client's `cs` verifies over the body's arguments; seen failing first with the old `canonicalJson`. **Added to PR 4's list:** the gateway verifies `cs` before it claims a grant's use — the signature only, by `client_key_standing` and `door_binding_mcp`, leaving `cn` to the provider — with a test that a refused signature leaves the grant unspent. **Not built in this PR:** the SDK clients against a live node (a recording stub and the vector stand in; the live suites run a node with no client certificate or authority); PR 4's three-node exit run; §9 O1 — the mandate digest the proof binds is the gateway's `serde_json` form, so a float in exponent form is refused, stated on the signer and in both READMEs. | #744 |
| I1 (second half, PR 4 of 4) — **I1 closed** | **The door's own `cs` check** (from #744's review). **Enumerated** (`grep -rn "client_sig::at_door" src`): three doors read `cs` — `/mcp` (`http/mcp_bridge.rs`) and the A2A unary send and stream (`a2a.rs`); each ran `at_door` and then `ae_preflight`, which claims a grant's use. Each now runs `client_sig::check_at_door` between the two: the signature over the door's own binding (`door_binding_mcp`, `door_binding_a2a`) and `client_key_standing`, sharing `check`'s code (`check_bound`); `cn` is left to the provider, which checks everything again; an unauthenticated caller is left to the provider. `a_signed_call_presenting_a_client_grant_runs_at_mcp`: a call with a signature over other arguments is refused `client_signature_invalid` at the gateway, and the honest call under the **same** grant then runs — seen failing first with the `/mcp` check removed (the honest call refused: the grant already used at the gateway); `both_a2a_doors_carry_a_client_signature_and_the_skill_sees_it`: a tampered send and a tampered stream refused by name and never reaching the skill — seen failing first with each A2A door's check removed, one build each. **The exit run:** `examples/eve_weather/run_client_grant.mjs`, the release builds, three runs, dated on [`what-is-proven.md`](../operations/what-is-proven.md#client-grant-three-node-fleet); CI's `eve-fleet` job runs it on debug builds. **Closing I1, promise by promise, across #741–#744 and this PR:** *"`mycelium tls issue --client <name>`: an Ed25519 certificate with SAN `mycelium-client://<issuer>/<name>`"* — `issue_client_cert`, `tls_issue_client`; `a_client_identity_is_read_exactly_and_bad_segments_are_refused_at_issue` (#741). *"the key held by the service's process"* — the key file `mycelium tls issue` writes owner-only; the SDKs' `ClientSigner` reads its PEM (#741, #744). *"Both mTLS verifiers refuse the scheme (seen failing first)"* — `a_client_certificate_is_refused_at_the_mesh_both_ways`; the ledger's [mesh row](../operations/what-is-proven.md#client-certificate-refused-at-mesh) against the release binary (#741). *"Revocation of a client key is its re-issue, by supersession … a high-water mark … in memory and in the node-local journal … a certificate at or below the mark is refused for good … its own line in that budget … two certificates in one millisecond tie-break on the greater key"* — the fold's `Mark`; `a_newer_certificate_supersedes_the_older_key_and_an_overwrite_cannot_lower_it`, `a_record_written_by_a_member_supersedes_on_arrival_before_anyone_asks`, `a_supersession_survives_a_restart_under_persistence_with_the_newer_record_gone`, `repeated_identical_publishes_leave_the_journal_bounded` (#741); on the fleet, the older key refused `client_key_superseded` after a newer certificate (this PR's run). The `sys/revocation/` client arm the row offered as the maintainer's alternative was not taken. *"`POST /gateway/identity/client-cert` … under `identity:write` … `compliance`-gated … publishes `sys/client-cert/{principal-hex}/{key-hex}` … the fold judges by the record's signature and the certificate's CA issuance, never by KV LWW time"* — `gw_identity_client_cert`; `the_client_cert_door_is_identity_write_and_publishes_what_it_judged`, `the_client_cert_door_checks_its_issuer_and_audits_the_bearer`, `the_publish_refuses_a_node_certificate_a_foreign_ca_and_a_superseded_key`, `junk_records_sorted_ahead_cannot_hide_a_newer_certificate` (#741, #742). *"`TrustedExternalIssuers` shared at `ExecutionAuthority`, `ProviderBudget`, `MembershipAuthorities` (the last never entitles a client) … a lock-order row"* — `SharedExternalIssuers`, lock-order row 77; narrowed in #743's review: a client key verifies a possession proof and nothing else (`a_client_key_proves_possession_only_and_a_superseded_one_never_returns`, `a_client_key_in_a_shared_set_never_entitles_a_removal`) (#743). *"A `token:` principal with a certificate on record and no `cs`: counted (`gateway_actions_unsigned{kind}`), then refused"* — **counted**: `the_mcp_door_carries_a_client_signature_counts_an_unsigned_one_and_needs_an_issuer`, `an_unsigned_call_for_a_certificate_holder_is_counted_at_the_provider` (#742); **refused: not built** — it waits a release with the counter visible, so an operator sees who would be refused before anyone is; §9 **O3**. *"a key-less principal: D6's window"* — I5's, unchanged. *"A client's call at a gateway without `gateway_identity_issuer` is refused by name at call time"* — `gateway_identity_issuer_unset` at all three doors (#742). *"`cs`/`cn` (D1′): `frame_with_context_at` writes them; `verify` checks `cs` before the cache; the signature vector … checked by Rust, jest and pytest"* — `frame_with_client_signature_at`, `the_provider_checks_the_client_signature_binding_signature_and_key_standing`, `the_client_signature_vector_is_the_same_in_every_language`, `the_canonical_arguments_are_the_same_in_every_language` (#742); the gateway now also checks the signature before its preflight (this PR). *"`grant/` keys: `node:` literal, client principals hex, readers decode by prefix"* — `grant_holder_segment`, `grant_holder_from_segment`, `grant_holder_from_path` (#743, the SDKs' URL shape in #743's review). *"The resource a client-held grant's decisions evaluate against … the provider node of the tool, resolved from the `tools/` scan as `/mcp` does"* — resolved when the request is written, signed into the grant (`bound:resource:`), evaluated at revocation without a live provider — a departure made in #743's review, `a_client_grant_is_revoked_against_its_issued_resource_with_the_provider_gone`. *"A client request door `POST /gateway/grants/request` under `mcp:invoke`"* — `gw_grant_request`, with `GET /gateway/grants/request/{request}` beside it (#743). *"SDKs: a `tools/call` client (new in both) and the A2A client carry `cs`/`cn`; `mycelium-ts` via `node:crypto`, `mycelium-py` via `cryptography`"* — `McpClient`, `A2aClient(signer)`, the grant verbs and the possession proof, `mycelium-py[sign]` (#744). *Exit:* *"a client certificate at the mesh refused by both verifiers; a pre-row-Q node certificate still admitted"* — `a_client_certificate_is_refused_at_the_mesh_both_ways` and the mesh row (#741). *"Y6's weather tool called through `/mcp` under a grant held by the example's `token:fleet/forecaster` on a three-node fleet of published binaries (I0) with `gateway_identity_issuer = "fleet"`, refused without one, surviving a restart"* — this PR's run: the gateway and a member are the `mycelium` binary **built locally in release mode** with the release's `NODE_FEATURES` (not the published artifacts; CI runs debug builds of the same features), the gateway with `[authority]` (I0); refused without a grant, run under one, a grant issued before the gateway's graceful restart run after it. **A departure, stated:** the third node is `eve_fleet_node`, not the binary, because the binary hosts no tool of its own (I0's row says so) and Eve's tool is served by bridging the loader's MCP server; and the restart is the gateway's, graceful. *"a foreign-key `cs` refused"* — `the_provider_checks_the_client_signature_binding_signature_and_key_standing` (#742) and the fleet run (`client_key_unknown`, this PR). *"a replayed `cn` refused and a reordered honest pair admitted; two processes under one certificate not colliding"* — `a_replayed_cn_is_dropped_and_reordered_and_concurrent_honest_calls_are_admitted` (#742), in process; not on the fleet, where a replay of a granted call is refused first by the grant's use marker. *"the cross-language vector green in all three suites"* — the `cs` vector (#742) and the possession vector with its digest cases (#744). *"a superseded client key refused once the newer record is read, still refused after the record is overwritten and, under `[persistence]`, after a restart"* — the #741 tests above; on the fleet only the first of the three, the older key refused once the newer record is read. *"`revoke_grant` on a client holder evaluating `tool:{tool}@{provider}`"* — the two-node revoke test (#743) and the fleet run's revoked grant. *"a pre-upgrade `grant/node:…` record read unchanged"* — `a_grant_holder_segment_round_trips_and_is_unambiguous` and every grant test unchanged (#743). **Not built, stated (the row's own, unchanged):** OIDC (I5); the developer's keystore key (I7); M-of-N (beyond I3, stated there). *"an un-upgraded node's reading of `sys/client-cert/` (shown against the previous binary)"* — **shown** in #745's review round: the exit run with the v2.39.0 `mycelium` (the previous release, built locally from its tag) as the member node passed `CLIENT GRANT OK`, and the old node logged no warning about the records it carried. That shows it carries and relays them; what it does with them is nothing, since it has no fold. **Moved:** the refusal of an unsigned call → §9 O3; the mandate digest across languages → I3 (§9 O1, the maintainer's decision). **Found by the exit run:** §9 O2 — after the gateway's restart its first calls to the provider time out until the provider writes towards it; the run waits for a write each way. **Found by the A2A stream plant:** §9 O4 — a provider's refusal reached `/a2a` as a `completed` task (fixed in the review round below). **Review round 1 (#745, independent):** (1) the gateway's fold of client certificates leads the provider's, so a grant was spent on a provider's `client_key_unknown` — a provider's pre-execution refusal (`client_key_*`, `client_signature_*`, `stale`) now tombstones the gateway's `…/used/{door}` marker and releases its claim (`release_grant_claim_after`; the claim carried on the permit as `grant_claim`); the claim's readers enumerated (`grep -rn "claim_grant_use\|release_grant_use\|/used/" src`): the preflight's claim and R3's release, nothing else; sound against R2 since the marker guards work that may have run and none did, a crash before the tombstone leaving the grant spent; the provider's answer is trusted, stated; `a_grant_the_provider_refused_before_running_is_given_back`, seen failing first with the release doing nothing; the exit run's `sleep(2_000)`s are gone — it retries `client_key_unknown` under the same grant; (2) the door checks `cn.t` on the provider's window (`client_issue_time_fresh`, shared), `stale`; (3) the un-upgraded node shown (above); (4) O2 on the feature page; (5) O2 measured and rewritten (§9); (6) O4 fixed at both A2A doors, the journal recording `failed`; a tool's `-32022` refusal now carries `data.reason` (`a_providers_refusal_reaches_an_mcp_client_as_json_rpc_with_its_reason`); (7) a name holding `@` binds — the claim read up to its last `@` (`a_tool_whose_name_holds_an_at_sign_binds_its_signature`), the signed message's first-`@` cut stated on the page; (8) the exit driver asserts each refusal's reason; (9) CI's `eve-fleet` builds the release's `NODE_FEATURES`; (10) the closure's wording (local release builds; the fleet shows the first supersession refusal only); (11) a gateway with `tls` and no `[tls]` answers `client_signature_unverifiable`, and the door's `unsafe_integer` is tested (`the_door_refuses_a_stale_an_unsafe_and_an_unverifiable_signature_by_name`). Each fix seen failing first, one build per plant. | this PR |

## 9 · Open items, tracked

Items found while building a row that the row does not own. Each names its home, what it changes, and why it waits.

- **O1 · The mandate/grant `arguments_digest` across languages** *(placed in I3 by the maintainer, 2026-11-09; found by
  #742's review, 2026-11-08)*. The digest a
  holder's possession proof and a grant's bound digest use is `serde_json`'s encoding in Rust
  (`gateway_authority::arguments_digest`), `json.dumps(sort_keys=True)` in Python (`a2a.arguments_digest`, `agent.py`) and
  JCS in TypeScript (`argumentsDigest`, `agent.ts`; since #744 `mandateArgumentsDigest`, the gateway's form but for
  floats). They disagree on exponent-form floats (`1e-5` / `1e-05` /
  `0.00001`, `1e16`, `1e21`). The failure is **closed**: a Python or TypeScript holder whose arguments carry such a float
  is refused (`BoundToOtherArguments` or a possession that does not verify); nothing is admitted that should not be.
  **Readers that change when it moves to RFC 8785**: the possession request and a grant's binding check
  (`gateway_authority`), the provider's possession check (`provider_enforcement`), a grant's bound digest where it is
  issued and listed (`state_machine`, `http/grants.rs`, `http/authz.rs`), the evidence envelope's digest
  (`action_evaluator`), and both SDKs. **Two points travel with it:** the large-integer rule I1's client signature
  adopted (an integer outside ±(2^53 − 1) refused, since JCS cannot bind it exactly), and a window — a grant issued
  before the change binds the old digest, so the change needs a release that accepts both or re-issues. **Home: I3**
  (decided by the maintainer, 2026-11-09), whose row lists it among its promises with the window and the large-integer
  rule. No code for it lands in I1.

- **O2 · A reply to a peer that restarted is lost until the sender notices** *(open, a transport defect; found by I1's
  exit run, 2026-11-09; measured the same day)*. After the gateway's graceful restart, the provider's first reply to it
  is lost, so the `tools/call` times out at the gateway's 30 s bound, although `/stats` reports both peers and a write
  at the gateway reaches the provider.
  - **The window:** with the run's back-probe dropped, a call made 1 s after the forward probe timed out, and calls
    made after 2, 3, 5 and 10 s ran (`CLIENT_GRANT_SETTLE_MS`, local release builds, one run each). That is about the
    provider noticing the dead connection on its health tick (1 s on `eve_fleet_node`; the default
    `health_check_interval_secs` is 10 s, so a default fleet's window is longer).
  - **Not the writer's idle bound** (the review's hypothesis): the provider's `writer_idle_timeout_secs = 5` changed
    nothing (two runs each way). Its health pings keep that writer busy, so the bound never fires.
  - **Consistent with:** the writer's own `Write to … failed; 1 frame(s) will be dropped during backoff`. A frame
    written into the connection to the peer's previous process is dropped, and an RPC reply is not resent.
  - **Reach:** this is the transport, not I1. Every reply a node sends to a peer that restarted, before it notices,
    is lost. The caller sees a timeout, which the substrate reads as delivery unknown. Whether the tool ran is not
    established here.
  - **Cost under a grant:** a timed-out call has spent the grant's use at the gateway, because a timeout is not one of
    the refusals that give it back.
  - **Home:** the transport, the maintainer's to place. Until then `run_client_grant.mjs` probes a write each way
    before calling; the probe from the provider surfaces the dead connection.
- **O3 · Refusing an unsigned call from a certificate holder** *(open; I1's "then refused")*. A `token:` principal with
  a client certificate on record calling without `cs` is counted (`gateway_actions_unsigned`, `provider_actions_unsigned`)
  and not refused (`deprecations.md` §87). The refusal waits a release in which the counters are visible, so an
  operator sees who would be refused before anyone is. **Home:** the release after I1's ships, as its own row; the
  maintainer's to schedule.
- **O4 · A provider's refusal reached `/a2a` as a completed task** *(fixed in I1's PR 4, #745's review; found
  2026-11-09)*. Both `tasks/send` and the stream answered a provider's refusal `completed`, with the refusal's body as
  the artifact. The evidence journal recorded `Execution::Completed`. Both doors now read the provider's refusal body
  (`provider_refusal` in `a2a.rs`): a JSON-RPC error and a failed task, each naming the reason, recorded `failed`
  (`a_providers_refusal_is_a_failed_task_at_both_a2a_doors_and_on_the_record`).