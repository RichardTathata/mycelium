# Identity and lifecycle — keyed principals, signed decisions, cancellation under a per-call mandate, and the developer surface

**Status:** **agreed with the maintainer on 2026-11-07**, rev 0.10 (row I4 is **not yet buildable: N1–N5 open**, the round-2 review's questions, which I4's build PR must settle; rev 0.10 takes #731's review and its confirm round: S-cancel re-measured, its reclamation fixed for the feeder, and I4's exit given an item for every D2 decision; rev 0.9 writes row I4 from spike S-cancel's note, `docs/plans/spikes/2026-11-07-s-cancel.md`; rev 0.8 records the agreement, the text then rev 0.7's); proposed 2026-10-31. Work starts in §4's order: wave 1a's I0 and wave 1b's spikes in parallel. Five independent adversarial reviews —
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
one-scope shape becomes one-of-two for these routes. SDK verbs sign. **Depends on:** I1. **Exit:** `not_the_decider`
*(new)* on a foreign signature; `not_addressed_here` on misdelivery; a relay with only `grant:relay` reaching the route
and succeeding with the signer as actor, refused unsigned; a re-targeted body refused; the journal names the signer; the
Cedar re-pin with a refusal case. **Not built, stated:** M-of-N; an `oidc:` decider; a relay at an un-upgraded gateway.

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

#### I2 · CA succession by a pre-committed successor — *written after S-succeed*

Decided (D4): `ca-commit.pem`, `tls commit`, `tls succeed`, the successor-signed record with its window end, act-once
and persist, a **gateway door** *(new)* that publishes the record (as I1's does), the shadow counted, the offline node
cut off and counted, `secure-single-domain` rev 6 / `secure-multi-domain` rev 2 requiring `id.ca_succession_committed`.
**From the spike:** whether a live node re-roots without a restart, and which key file rotation uses after succession.

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
  refused; **key-less principals lose action scopes at the refusal release** (`oidc:`, `anonymous`, the legacy token;
  `/mcp`, KV, `/a2a`); `identity:write` is an existing scope, so a token holding it for `/gateway/identity/revoke` gains the client-cert
  door at the upgrade — reissue narrower tokens first; `ExecutionAuthority::new` takes the shared `TrustedExternalIssuers`
  handle (a caller breaks); `sys/client-cert/` ignored by an un-upgraded node (shown); a
  `mycelium-client://` certificate refused at the mesh from this release; pre-row-Q node certificates unaffected.
- **I3:** an unsigned decision body counted then refused; `grant:relay` inert at an un-upgraded gateway.
- **I5:** `ot` optional; the provider's egress list must admit the issuer; a provider without `compliance` refuses a call
  carrying `ot` as `not_in_build`; an envelope past 8 KiB is refused `caller_context_too_large`, as today.
- **I2, I4, I7:** written with their rows.
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
