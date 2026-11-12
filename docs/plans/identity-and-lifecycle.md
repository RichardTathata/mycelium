# Identity and lifecycle — keyed principals, signed decisions, cancellation under a per-call mandate, and the developer surface

**Status:** **agreed with the maintainer on 2026-11-07**, rev 0.17 (rev 0.17, 2026-11-12, writes row I4 buildable: N1–N5 resolved, with `file:line` at `d974fc76`, and the maintainer's decisions D-1–D-6 of 2026-11-12 recorded in the row — a `call:` mandate rides a second slot, is a cancellation handle no rule's `requires_mandate` reads, is self-issued by the envelope's gateway and is assessed once at `admit_after`; `cancellable` requires it; the KV, not a window table, survives a restart, with `call:` exempt from the start rule; a retention floor for `call:` and `grant/`; N1's grant half recorded as fixed by #735; exits 1–3, 5, 6, 10–12, 14, 16 and 17 reworded; I2 dependency noted; rev 0.16, 2026-11-12, takes #764's review (M1–M6, L1–L9) and records the maintainer's decisions on I7 of the same day: P2 a fleet-certified P-256 consent key verified at `grant_decision::verify` only, issued over its public key with the CA off-node; P3 the CryptoKit blob path, no signed bundle; P1 `os_presence(macos, unbound)`, an enrolment claim no verifier can check; P4 the grade on the consent record, D7's three plus `service`; P5 a residual; P6 I1's supersession with every node upgraded first; then, after #764's confirm round, H1 a consent key under its own principal `consent:{issuer}/{name}` (nothing supersedes across kinds; never fed to possession) and H2 the grade written into the consent certificate by the fleet CA, the body's grade refused when it differs (P4 revised); after the second confirm round, A consent's own authority (`gateway_consent_principals`, `signed_with` at the evaluator), B the `tty` residual stated, the grade's OID on the `2.25` UUID arc, its message encoding and the P-256 key-hex pinned; the consent issue flow over an SPKI PEM with `--grade` and a `--force` guard — I7 **buildable on macOS**, its `tty` half then waiting on P7, decided the same day: a fleet-certified P-256 consent key in software on Linux and Windows, graded `tty`, so I7 is **buildable**; D7's *Revised from* and N7 follow; rev 0.15, 2026-11-12, writes row I7 from spike S-presence's note, `docs/plans/spikes/2026-11-07-s-presence.md`, and its maintainer's run of 2026-11-09: the four questions the stub left to the spike answered with what it settled — the fleet CA can certify a P-256 Enclave key and `ring` verifies it, the release pipeline cannot produce a signed bundle, Windows was not run so D7's `tty` stands, and presence is not attested to the verifier, so an `os_presence` grade is an enrolment claim — where the evidence and D7 disagree stated, every verifier a client key reaches enumerated, and I7 **not yet buildable: P1–P6 open**, the maintainer's; rev 0.14, 2026-11-12, records wave 1a — I0, I1, I3 and I5 — released in v2.40.0; rev 0.13, 2026-11-11, closes I5 on evidence across #757, #759 and its PR 2, with the decision that a gateway exchanges only for an action recorded on the row; rev 0.12, 2026-11-11, records #759's review on I5's provider half: the check's order restated and pinned, its clocks read at the receive boundary's instant, one OIDC verifier per node, the replay window keyed across gateways, `typ` and two start-time refusals, and the maintainer's decision that a hosted capability's invoke needs an `ot`; rev 0.11 records the maintainer's four I5 decisions of 2026-11-11 and spike S-exchange's note, `docs/plans/spikes/2026-11-11-s-exchange.md`: the gateway exchanges under its own client credential, `oidc.exchange_audience` is an explicit setting both ends read, the envelope cap stays at 8 KiB; after #757's review, the maintainer's decision to extend the spike to sender binding, and the row rewritten to it: the exchanged token is DPoP-bound to the gateway's Ed25519 node key (Keycloak 26.8.2, default features), the provider refuses a token not bound to the key that signed its envelope, and replay rests on that and the token's `jti` admitted once; rows I2 and I4 are **not yet buildable**: I2's **R1–R4 open**, the round-3 review's questions, and I4's **N1–N5 open**, the round-2 review's questions, which each row's build PR must settle; rev 0.10 writes row I4 from spike S-cancel's note, `docs/plans/spikes/2026-11-07-s-cancel.md`, and takes #731's review and its confirm round: S-cancel re-measured, its reclamation fixed for the feeder, and I4's exit given an item for every D2 decision; rev 0.9 writes row I2 from spike S-succeed's note, `spikes/2026-11-07-s-succeed.md`, and #732's review: a `cert_pem` node re-roots live, the trust set and the successor certificate persist under `succession/`, every cached judgment is re-judged when the roots change, and old-CA records lose standing (decided in the confirm round); rev 0.8 records the agreement, its text rev 0.7's); proposed 2026-10-31. Work starts in §4's order: wave 1a's I0 and wave 1b's spikes in parallel. Five independent adversarial reviews —
four on one day, the fifth on the merged document (#715's thread, 2026-11-05; the thread, this paragraph and the *Revised
from* column are the record of all five): rev 0.1 against the decisions, rev 0.3 against the mechanisms, rev 0.4 and rev 0.5 each against five
premises, both with the verdict *not yet buildable*. Every premise failure sat in wave 1 (I0–I4) or in the developer
identity (D7); none in I3, I5, N3, N5, N6 — the reviewer named those buildable. **Rev 0.6 is a split, not a rewrite
(D8):** wave 1a holds the rows a plan can close — I0 narrowed to this node's own unit, I1 narrowed to service principals
holding an Ed25519 key, I3, I5 — and wave 1b holds three **spikes** whose code, not this document, settles the three
premises the reviews could not: a `call:` mandate carried with its first checkpoint (I4), a certificate reload without a
restart (I2), and an OS-presence signer (the developer identity, now its own row I7; its spike found a signed bundle unneeded). Wave 1b's
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
| **D7** | **The consent primitive is an OS user-presence check whose result the repository verifies, graded on the record — the mechanism is settled by the spike S-presence (I7).** What the fourth review established from Apple's and Microsoft's documentation: a Secure Enclave key is **P-256**, reachable only from a **team-signed bundle with a provisioning profile** (a plain `cargo build` binary cannot use it, and the release pipeline builds plain binaries); the ACL cannot be read back, so the check is a negative probe (`interactionNotAllowed` must fail) plus `kSecAttrTokenIDSecureEnclave` and the enrolled public key compared on every use; `biometryCurrentSet` removes the typed-password leg; a fresh `LAContext` per approval. **Windows Hello keys are user+device scoped** — a same-user process that knows the credential name can request a signature — so Windows grades **no higher than `tty`** until shown otherwise. The grade on the record: `os_presence(macos)` · `tty` (Linux, Windows) · `recording` (a `sim` test build). | the rule "a check no co-resident process can satisfy, whose result we verify"; where none exists the record says so. | rev 0.5 assumed an Ed25519 key, a readable ACL and a Windows primitive. Rev 0.16 (the maintainer, 2026-11-12, from S-presence): the key is reached through a CryptoKit blob, not a team-signed bundle, and the macOS grade is `os_presence(macos, unbound)`, an enrolment claim no verifier can check — I7. |
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

*Delivered (#733); closed promise by promise in §8.*

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

*Delivered (#750, #754 and its PR 3); closed promise by promise in §8; the private Cedar adapter's re-pin is §9 O7, delivered 2026-11-12 (private #34).*

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

*Delivered (#757, #759 and #760); closed promise by promise in §8.*

*Settled by spike S-exchange ([note](spikes/2026-11-11-s-exchange.md), 2026-11-11; evidence at tag
`spike/s-exchange-2026-11-11-r2`) and the maintainer's decisions of the same day (rev 0.11).*

**What:** RFC 8693 exchange at the IdP **by the gateway, under its own client credential**, the exchanged token
**sender-bound to the gateway's node key by DPoP** (RFC 9449), and verified at the provider.

- **The exchange** *(decided 2026-11-11)*: the gateway exchanges the user's gateway-audience token at
  `[oidc] exchange_token_endpoint` *(new)* as `exchange_client_id` *(new)*, with the client secret read from the
  environment variable `exchange_client_secret_env` names *(new; default `MYCELIUM_OIDC_EXCHANGE_SECRET`)*, for
  `audience = exchange_audience` *(new)*. It sends a DPoP proof signed with its Ed25519 node identity key (`alg:
  EdDSA`). The spike ran with a generated Ed25519 key, as the node's is: Keycloak 26.8.2 accepted the EdDSA proof and
  issued `cnf.jkt` equal to that key's RFC 7638 thumbprint.
- **The node key signs a JWS** (#757's confirm round, N2). The DPoP proof is a JWS: the node's Ed25519 key signs
  `BASE64URL(header) "." BASE64URL(payload)`, which begins `eyJ` and is printable text with no zero byte. That cannot be
  any message the node key signs for Mycelium, because every such message has one of three shapes, none of them a JWS
  signing input (#757's second confirm round):
  - **a raw `mycelium` tag first:** `DOMAIN_SEP`, `DOMAIN_SEP_ID`, `ID_MARKER_SEP`, `GRANT_DECISION_TAG`,
    revocation's `V2_DOMAIN`, `possession_message`, and the identity-proof, consensus and KV tags;
  - **a zero byte within the first eight:** the length-prefixed forms put a `u32` little-endian length before their
    tag (grants and checkpoints, `Mandate::canonical_bytes` and `RevocationCheckpoint`, signed at `http/grants.rs`;
    knowledge records and heads; cohort declarations; `MemberRemoval`; the federation descriptor, policy, call and
    catalogue), so bytes 1–3 are zero. The untagged `serde_fixint` kinds whose first field is a `NodeId` — role
    claims (`rbac.rs`), audit records and checkpoints (`audit.rs`), v1 revocation events (`revocation.rs`) — write
    it as a `u64` little-endian length of at most about 47, so bytes 1–7 are zero. `RightsHead::canonical_bytes`
    (`control/ledger.rs`) is the same shape with a `PrincipalId` first; whether the private RA companion signs it with
    a node key is not visible from this repository;
  - **`{` first:** AgentFacts' canonical JSON, which companions sign through `sign_with_identity`.

  A JWS signing input is base64url text joined by `.`: it holds no zero byte and cannot begin with `{` or with a
  `mycelium` tag, so no DPoP proof verifies as a Mycelium message, and no Mycelium signature verifies as a DPoP proof.
  **The rule this rests on:** every message signed with the node key must keep one of those three shapes; a new signed
  form is checked against them before it ships. Clients do not
  change. A gateway-audience token is never forwarded. The exchanged assertion travels in `ot` *(new)*, once per call.
- **When a node exchanges, and when it verifies** (#757's review, M4; restated by the maintainer on 2026-11-11, #760's
  review M1). **A node exchanges iff `exchange_token_endpoint` is set, and verifies iff `exchange_audience` is set**;
  whether it runs an HTTP port decides neither. With the endpoint set, a missing audience, client or secret, or an
  endpoint neither https nor loopback, is refused at start by name (`oidc_exchange_audience_unset`,
  `oidc_exchange_client_unset`, `oidc_exchange_secret_unset`, `oidc_exchange_endpoint_insecure`), identically in
  `validate()`, `preflight()` and so `mycelium doctor`. A provider with only a health port needs no secret, and a
  gateway that also provides may verify without exchanging: its OIDC users' actions toward a verifying provider are then
  refused `ot_required`. Under `gateway_caller_profile = legacy` nothing is exchanged and the settings are not demanded.
  - A gateway with an `[oidc]` table and neither setting behaves exactly as today: no exchange, no `ot`.
  - **A gateway exchanges only for an action** *(decided by the maintainer, 2026-11-11)*: the kinds a verifying
    provider requires an `ot` for (`needs_exchanged_token`, below — the protected kinds, the companions' write kinds, and
    since #759's Q1 a hosted capability's invoke). Every other call by an OIDC user is unchanged: no exchange, no `ot`.
    A non-`oidc:` principal is never exchanged for. **"Action" is read per node** (#760's review, L4): a kind one node
    lists in `protected_rpc_kinds` and another does not is exchanged for by neither or refused by one, so a protected
    kind is listed on every node that dispatches or serves it.
  - A node with `exchange_token_endpoint` (the exchange trigger, decided 2026-11-11) refuses to start, by name, when
    `exchange_audience` or `exchange_client_id` is unset, when the endpoint is neither https nor loopback, or when the
    variable `exchange_client_secret_env` names is unset or empty (`oidc_exchange_audience_unset`,
    `oidc_exchange_client_unset`, `oidc_exchange_endpoint_insecure`, `oidc_exchange_secret_unset`). **An exchanging node
    always verifies too**, since the endpoint requires `exchange_audience`, the verify trigger.
  - A provider with `exchange_audience` verifies `ot`, and needs `exchange_client_id` too: the `azp` it accepts. It
    also needs `[tls]` (`oidc_exchange_needs_tls`: the token binds to the node key that signs the envelope), and an
    `exchange_audience` other than `audience` (`oidc_exchange_audience_is_gateway_audience`) *(#759's review, L1 and Q3,
    2026-11-11)*.
  - A provider-only `[oidc]` table still fills today's required `audience`. It is read only to validate bearers at a
    gateway, so a provider-only node sets the value its gateways use. Nothing in it is new.
- **The provider's check, in order.** The verifier is `#[cfg(feature = "compliance")]`: a provider built without it
  refuses a call carrying `ot` at call time as `not_in_build`, naming `compliance` (the response error `govern.rs`
  already answers for a feature the build lacks), rather than running it unverified. The checks, the first failing one
  naming the refusal *(the order restated by #759's review, L3, 2026-11-11, and pinned by
  `the_first_failing_check_names_the_refusal`)*:
  1. **The header:** an allowed asymmetric `alg` (`none` and `HS*` are `ot_malformed`), a `typ`, if any, of an access
     token — `JWT`, which Keycloak 26.8.2 emits, or RFC 9068's `at+jwt` (`ot_wrong_type`; Q3, 2026-11-11: requiring
     `at+jwt` would refuse Keycloak's tokens), and a `kid` (`ot_malformed`).
  2. **The keys:** the issuer's JWKS from the provider's **own** `[oidc]` table (`issuer`, `jwks_uri` or discovery)
     (`ot_keys_unavailable`).
  3. **The signature.** A member's forged `ot` is refused here (`ot_signature_invalid`).
  4. `iss` the provider's issuer, absent included (`ot_wrong_issuer`).
  5. `aud` contains `exchange_audience`, as a string or in an array (`ot_wrong_audience`).
  6. `azp` equals `exchange_client_id` (`ot_wrong_client`).
  7. **Sender binding** (decided with the spike, 2026-11-11): `cnf.jkt` is present (`ot_unbound`), and equals the RFC
     7638 thumbprint of the Ed25519 key that verified **this envelope's** gateway signature (`ot_bound_to_another_key`).
     The envelope's signature is the proof of possession, made per call over the principal, `o`, `n`, the time and the
     payload's digest. A member that lifts an `ot` signs its own envelope with its own key, and is refused (#757's
     review, H1, H2).
  8. **The subject** (M2): the envelope's `p` equals `oidc:{iss}/{sub}` of the token (`ot_subject_mismatch`).
  9. **Lifetime and clocks** (M1): read against **the instant the receive boundary admits at** — the HLC's
     `decision_now_ms`, read once per received request — never the system clock (#759's review, M1, 2026-11-11), under
     the leeway of `max_clock_drift_ms`. `iat` present (`ot_without_iat`) and `exp` present (`ot_without_exp`); neither
     `iat` nor `nbf` in the future (`ot_not_yet_valid`); `exp` not passed (`ot_expired`). A token whose `exp − iat`
     exceeds `exchange_max_lifetime_secs` *(new, default 300, at most 3 600)* is refused (`ot_lifetime_too_long`).
  10. A token without `jti` is refused (`ot_without_jti`); Entra's `uti` is not read.
  11. **Replay** (M1): the token's `(iss, jti)` is admitted **once at this provider**, whichever gateway carries it,
      while it is unexpired.
- **The replay cache** (M1):
  - **Per provider**, in memory, keyed `(iss, jti)` **across gateways**, each entry held until its `exp` plus the
    leeway *(#759's review, Q2, decided 2026-11-11: the window was keyed per gateway, so one key under two node ids got
    two admissions; one map across gateways closes that, and frees a departed gateway's entries as they expire — L2)*.
    Expired entries leave in expiry order from a queue, so an admission removes only what has expired.
  - **At most 262 144 entries in all, 65 536 per gateway, and 16 384 per principal** (#757's confirm round, N1; the
    total #759's review, L2, `ot_provider_window_full`). Unlike the envelope's replay window, which evicts and keeps a floor, this one **refuses**: an entry is
    held until its `exp` plus the leeway, and a new token arriving while the provider's, its gateway's or its
    principal's share is full is refused, failing closed (`ot_provider_window_full`, `ot_window_full`,
    `ot_principal_window_full`, each counted).
  - **Three hard ceilings, each at the provider** *(restated after #759's confirm round, N1, 2026-11-11 — stated, not
    scaled)*: per gateway, 65 536 / (lifetime + drift) tokens a second; **provider-wide, 262 144 / (lifetime + drift)**;
    per principal, 16 384 / (lifetime + drift), **counted across every gateway** the principal calls through. At the
    defaults (300 s lifetime, 300 s drift) that is about **109**, **436** and **27 calls a second**; at the 3 600 s
    lifetime cap, about **17**, **67** and **4**.
  - **A cross-gateway denial, stated:** the provider-wide bound is four gateways' shares, so **four gateways at their
    share fill it**, and every other gateway's OIDC users are then refused `ot_provider_window_full` until entries
    expire — the per-gateway design did not have this. It bounds the provider's memory whatever the number of gateways,
    which the per-gateway windows did not (#759's review, L2); a total that scaled with the gateways seen would give
    that bound back. Like the per-gateway case below, the hard bound against it is the gateways' rate limits; **not
    built**.
  - **Why a per-principal bound, not eviction:** evicting by `iat` would raise a floor that refuses an honest token
    issued in the same second as an evicted one. A refusal per principal keeps one user, or one compromised account,
    from filling a gateway's share and denying that gateway's other users. **Four principals at their bound still
    fill it**, and sixteen fill the provider's; that denial of service is **not built** (the gateway's rate limits are the hard bound).
  - **After a restart the cache is empty.** A token presented before the restart can be presented once more after it,
    while unexpired, **only in an envelope signed by the bound gateway's key**: the sender binding holds across the
    restart. That leaves the gateway-as-attacker case below.
- **Keys before the first fetch** *(decided by the maintainer, 2026-11-11)*. The provider fetches the issuer's JWKS at
  `start()` and refreshes it in the background, through **the gateway's own verifier and its cache**
  (`OidcVerifier::keys_now`, a non-waiting read of the same plan; no second fetcher — one `Arc<OidcVerifier>` per
  node, `TaskCtx::oidc_verifier`, set at `start()` and reused by the gateway, #759's review, M2). Off a tokio runtime a
  verify starts no fetch and refuses with the keys it holds (L4). A key the issuer rotated in is
  picked up by a refresh started on an **unknown `kid`**, single-flight and at most once per 30 s cooldown, as well as
  by the 1 h TTL refresh on use. Keys more than 24 h past their last successful fetch are not used (R4's ceiling). A
  token carrying `ot` while no usable key is held, or naming a `kid` the held keys lack, is refused
  **`ot_keys_unavailable`**, retriable, and never admitted.
- **The refusal names** *(PR 1, 2026-11-11)*, in the check's order:
  - `ot_unsigned` (no `so`), `ot_signature_malformed` (a `so` that is not a 64-byte signature; #759's review, L3),
    `bad_signature` (`so` does not verify), and `ot_unverifiable` (on a mesh without `[tls]`, no key binds);
  - `ot_malformed`, `ot_wrong_type`;
  - `ot_keys_unavailable`;
  - `ot_signature_invalid`;
  - `ot_wrong_issuer`, **`ot_wrong_audience`** (a gateway-audience token forwarded rather than exchanged; the name
    agreed 2026-11-11), `ot_wrong_client`;
  - `ot_unbound`, `ot_bound_to_another_key`, `ot_subject_mismatch`;
  - `ot_without_iat`, `ot_without_exp`, `ot_not_yet_valid`, `ot_expired`, `ot_lifetime_too_long`, `ot_without_jti`;
  - at the receive boundary, `ot_replayed`, `ot_provider_window_full`, `ot_window_full`, `ot_principal_window_full`;
  - `ot_required`, and `not_in_build`.
- **An `oidc:` principal's action needs an `ot`** (M2). At a provider that verifies (`exchange_audience` set), an
  envelope whose `p` is `oidc:…` is refused without one (`ot_required`) when its kind is (#757's confirm round, N4):
  - a kind `is_protected_kind` names: `mcp.invoke`, `skill.invoke`, `llm.invoke` (`BUILTIN_PROTECTED_RPC_KINDS`), and
    each kind in the operator's `protected_rpc_kinds`;
  - a companion kind `companion_kind_scope` gives a `:write` scope: every `blackboard.*`, `tuple.*` and `wiki.*` kind
    except the read operations — `read`, `depth` and `snapshot` on a board, `depth` and `wal_replay` on a tuple space,
    `access` on a wiki — an unknown verb included, as that table treats it;
  - a hosted capability's invoke, the `cap.invoke/{namespace}/{name}` family (`mycelium_wasm_host::cap_invoke_kind`),
    *decided by the maintainer on 2026-11-11 (#759's Q1): it runs code*. The artifact kinds (`artifact.fetch`,
    `artifact.fetch_range`, `artifact.size`) and `reason.blob.fetch` are not actions.

  Every other kind is not an action here, and needs no `ot`. A provider that does not verify behaves as today.
- **The scopes** (M2) come from the envelope's `s`, as today: the gateway's mapping of the user's groups through its
  `group_scopes`. The `ot` establishes **who** the user is, not what they may do. The provider does not re-derive scopes
  from the token.
- **The gateway as attacker** (M3). A gateway holding the client secret receives users' gateway-audience tokens on
  every request. With them it can obtain an `ot` for any user who calls through it, at will, bound to its own key,
  until that user's token expires.
  - The binding stops a **member** acting as the user. It does not stop the **gateway** the user called through.
  - `azp` names the client, which every gateway of the fleet may share, not a node or a call.
  - A compromised gateway acts as every user who calls through it, as a compromised gateway does today.
- **The envelope cap stays at `MAX_ENVELOPE_BYTES` (8 KiB under a `u16` prefix)** *(decided 2026-11-11)*.
  - The spike measured an envelope with a presented grant, a `cs` and Keycloak's 1 309 B DPoP-bound token at **2 945 B**,
    5 247 B under the cap. With `so` (below) it is about 3 041 B.
  - The cap holds a token of up to **6.4 KiB** beside a grant and `cs`, about **6.3 KiB** with `so`.
  - Past it, the producer's existing refusal answers: `GatewayDispatchError::ContextTooLarge`,
    `caller_context_too_large`, counted in `mycelium_gateway_caller_refusals_total`.
- **Signed with the envelope** (#757's confirm round, N3). When `ot` is present the gateway **adds** a signature,
  `so` *(new)*, under the domain tag `mycelium:gateway-caller-ot:v1\n`, over the envelope's id `n`, its target `o` and
  the `ot`'s SHA-256. A relay cannot swap one user's token into another's envelope. `sn`, the existing signature, is
  unchanged, so a provider that predates I5 still verifies the envelope and ignores `ot` and `so`.
  - `so` adds about 96 B (64 B of signature in base64, and its field). The spike's DPoP-bound envelope is then about
    **3 041 B**, and the cap holds a token of up to about **6 460 B (6.3 KiB)** beside a grant, a `cs` and `so`.
- `doctor` reports `id.oidc_exchange` *(new)* — and, apart, `id.oidc_exchanged_token`, the verify half (#760's review,
  M1, 2026-11-11).

**Depends on:** I1.

**Exit:**
- an OIDC user's call verified at the provider;
- the same exchanged token presented twice refused, by its `jti` (the spike's reading of "a replay refused");
- **a token lifted into another node's envelope refused `ot_bound_to_another_key`**, and a token without `cnf`
  refused `ot_unbound`;
- a token whose subject is not the envelope's principal refused;
- a token whose `azp` is not the gateway's client refused;
- a gateway-audience token refused by name;
- a provider without `compliance` refusing `ot` as `not_in_build`;
- an oversized envelope refused `caller_context_too_large`;
- a member's forged `ot` refused by the JWKS;
- the exchange's settings refused by name at start when unset, and an existing `[oidc]` gateway without
  `exchange_audience` unchanged;
- an `oidc:` action without an `ot` refused at a verifying provider.

**Not built, stated:**
- an IdP without exchange (the client certificate is the path);
- a decision signed by an assertion;
- a token bound to its **call**: the spike found no supported mechanism, and sender binding is what is built;
- the exchanging gateway reusing its own bound token in several envelopes until `exp` (refused at each provider by
  `jti`, once per provider);
- the gateway as attacker (M3);
- the mTLS route (RFC 8705), shown viable and not chosen;
- an IdP other than Keycloak, and Entra's `uti`;
- the provider deriving scopes from the token.

### Wave 1b — three spikes, then the rows they settle (D8)

A spike is one branch, at most two days, read-only on `main`, whose report is a dated note in `docs/plans/spikes/`
*(new directory)* quoting what ran, what it measured and what it could not do. Its row is written afterwards from that
note and goes through the standing review as any row does. No spike ships. The table below also holds **S-exchange**, a spike for wave 1a's row I5, added on the maintainer's instruction of 2026-11-11.

| Spike | Question only code can answer | Done when |
|---|---|---|
| **S-cancel** | A `call:` mandate minted at admission reaches the provider before its checkpoint gossips. Carry the first checkpoint **inside the envelope** beside the mandate, through `assess` and `admit_after` to `begin`; measure the added bytes and the admission latency against today's grant path; show a revoked term read `Refused`; and two bounds the row must state: a term revoked between the gateway's framing and the provider's `admit_after` carries a checkpoint that predates the revocation, so the provider registers the work and stops it only when the revoking checkpoint arrives — the cancel's effect is gossip latency plus the drain bound; and `drain_bound` is `Unbounded` for the unenforced and unconfirmable classes ([`authority.rs`](../../src/mandate/authority.rs)), where a cancel writes `Refused` and stops nothing — the cancel receipt names the drain class it hit, or answers `DeliveryUnknown`. | a call registered with `begin` without a KV round-trip, or the bounded wait's cost measured; the memory-reclamation hook shown on 10⁵ calls |
| **S-succeed** | A node on `cert_pem` (secure profile, no CA key) takes a certificate issued under a successor CA **without a restart**: a reload path through `activate`, a two-root `RootCertStore`, `verify_member_cert` across the window. | the handshake succeeds under the new root on a live node, or the spike records that a restart is required and why |
| **S-exchange** *(wave 1a, added 2026-11-11)* | Can a per-call value be bound into a token Keycloak exchanges (RFC 8693), and failing that, can the token be **bound to the exchanging gateway** (DPoP, RFC 8705 mTLS) on a supported release; the exchanged token's size against the 8 KiB envelope cap. | a token carrying the binding on a pinned image, or the route shown not viable; the envelope measured with a grant and a `cs` — **done** ([note](spikes/2026-11-11-s-exchange.md)): no per-call binding in a supported configuration; DPoP and mTLS sender binding both viable on 26.8.2 with default features; 2 945 B |
| **S-presence** | `dev` signs with a Secure Enclave P-256 key from a **team-signed bundle**, verified by one P-256-aware verifier; the negative probe (`interactionNotAllowed` fails), `kSecAttrTokenIDSecureEnclave`, `biometryCurrentSet`, a fresh context per approval; and whether the release pipeline can produce such a bundle at all. | a signature produced only after the prompt and verified; the pipeline question answered yes or no; if no, I7 falls back to `tty` everywhere, graded — **done** ([note](spikes/2026-11-07-s-presence.md); the maintainer's run of 2026-11-09): a signature after a Touch ID prompt, verified; the pipeline answer no; the key reached from an ad-hoc binary through a CryptoKit blob, not the keychain |

#### I4 · Cancellation under a `call:`-scoped mandate — *written from S-cancel ([note](spikes/2026-11-07-s-cancel.md), 2026-11-07; evidence at tag `spike/s-cancel-2026-11-07-r2`) — **buildable: N1–N5 resolved and D-1–D-6 decided, 2026-11-12***

Decided (D2): scope prefix; holder = the gateway; second mandate slot; `Refused` on revoke; `is_entitled`'s `call:`
arm; incremental feeder; in-memory reclamation with a lock-order row; a cancel-forward RPC kind *(new)*; A2A
`tasks/cancel` revokes before it removes (`handle_tasks_cancel` is sync — the revocation is spawned, stated); MCP
`notifications/cancelled` *(new handler)* with a bounded id → term table; retention `max_drain_secs` *(new)* + the sweep
ceiling for `call:`, `grant_retention_secs` *(new, default 7 days)* for `grant/`; a `cancellable` key is **refused by
name by a 2.38 reader** (`RuleDecl` and `MandateDecl` deny unknown keys, `deprecations.md` §58–§65) and read as a plain
allowance by a ≤ 2.37 one, so it is declared only on an upgraded fleet.

**Decided 2026-11-12 (D-1–D-6, the maintainer, from the N1–N5 resolutions below).**
- **D-1. A `call:` mandate is a cancellation handle, not authority a rule can require.** It travels in the envelope's
  second slot and never enters `ActionEnvelope.mandate` (`src/agent/action_evaluator.rs:147`). A rule's
  `requires_mandate` (`:2144–2155`) is satisfied only by the caller's own mandate in slot 1. Otherwise a gateway could
  mint a mandate for itself and meet any rule that requires one.
- **D-2. `cancellable` on a rule means the provider requires an established `call:` mandate.** Without one the call is
  refused `authority_not_established` (-32031), detail `call_mandate_not_established`, with `Execution::None` on the
  record, as #735's refusal does. Stripping slot 2 cannot make a cancellable call run uncancellable.
- **D-3. Slot 2 is assessed once, at `admit_after`, immediately before `begin`.** It is not assessed at the preflight,
  so there is no check-then-act window for it (N1). The `call:` term is recorded on the **execution** record, not on the
  decision's envelope.
- **D-4. No durable window table.** The KV is the restart record: the provider re-reads its copy of the call's
  checkpoint at every admission (N2), which re-creates the window. A provider that crashes before the revoking
  checkpoint reached its WAL admits a re-presentation on the carried checkpoint and stops it when anti-entropy
  re-delivers the revocation. That is bound 1, stated (N5).
- **D-5. A retention floor for both key families.** A `call:` record is kept until at least `valid_until_ms` + 2·skew +
  `max_drain_secs`, and a `grant/` record until at least `valid_until_ms` + 2·skew, whatever the setting.
  `validate()` refuses a `grant_retention_secs` below the longest grant window the node issues.
- **D-6. A `call:` mandate is self-issued by the gateway that sends it.** Its `established_by` equals its `holder`,
  which equals `node:{via}` of the envelope carrying it. Minting is turned on by a gateway setting *(new)*. This
  replaces a separate entitlement for the `call:` prefix (exit 3).

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
mandate, the `Unknown` binding is refused (`src/agent/action_evaluator.rs:2144–2155`). That was the spike's
reading. The row replaces it with D-1 and D-2: a `call:` mandate never meets `requires_mandate`, and it is a cancellable rule
that refuses a call without one.

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
   because its carried checkpoint predates the revocation. It stops when the revoking checkpoint arrives. A
   revocation already in the provider's KV when `admit_after` runs is read there and refuses the call (N1, N2,
   resolved below): the spike's code ran such a call unregistered, and skipped a revocation written between its KV read
   and its window's insert.
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
  admission. The window is inserted before that read, inside the same hold of row 43 (N2), so the race of bound 1 closes
  for any revocation already written.
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
- the window table across a restart: **not built, by decision** (D-4). The KV re-read at admission
  replaces it (N5).

**Rev 0.5's exit list.** The row said *"the exit list of rev 0.5 then stands, with the soak"*. That list is not in the
repository: the plan entered the tree at rev 0.6 (#715, one commit), and a `git log --all -S` search confirms it is not
in #715's thread. The exit below is written from D2 and the note. **The soak is stated here for the first time**, for
the review to confirm.

**N1–N5, resolved (2026-11-12).** These were the round-2 review's questions, and every one is now settled. Each
resolution names the place it is built and the test that shows it, seen failing first. The citations are at `main`
`d974fc76` (v2.40.0). They were found with `git grep -n "admit_after\|authority.begin\|mandate_established\|established_at_preflight\|feed_grant_checkpoint\|is_entitled\|RevocationView::started_at\|grant_scope\|kv_delete_held\|MAX_ENVELOPE_BYTES"`
over `src/` and `mycelium-core/src/`, and from the spike's diff, `git diff 6a64e8bc spike/s-cancel-2026-11-07-r2 -- src/`.

- **N1: a revocation between the preflight and `admit_after`.**
  - **The grant half was fixed by #735** (`85cea86b`, its review rounds `18473186` and `7c158d9a`). Rev 0.10 said
    *"`main` has the same shape for grants today"*, and that is no longer true:
    - `Preflight::Proceed` carries `mandate_established` (`src/agent/http/authz.rs:652`, set at `:1544`), and it
      becomes `AdmittedCall.established_at_preflight` (`src/agent/provider_enforcement.rs:332`, set at `:295`).
    - When the re-assessment no longer finds the mandate established, `admit_after` (`:353–410`) refuses the call
      `authority_not_established` (-32031), detail `mandate_lapsed_before_run`, and records `Execution::None`.
    - The test is `a_grant_revoked_between_the_preflight_and_the_registration_refuses_the_call`
      (`src/agent/http/grant_tests.rs:465`), held at the `test_hooks::PAUSE` hook (`provider_enforcement.rs:302–322`).
  - **The `call:` half is resolved by D-3.** Slot 2 is assessed once, in `admit_after`, immediately before `begin`, so
    it has no second assessment that could disagree with a first.
    - A cancellable call whose `call:` mandate is not established there is refused through #735's path (D-2).
    - A revocation landing after that read is the sweep's to catch (bound 1).
  - **Test:** the `PAUSE` test for a `call:` call. It is framed, held at `before_admit_after`, the revoking checkpoint is
    written, and it is resumed.
    - Expected: -32031 `call_mandate_not_established`, the tool not run, and `Execution::None` on the record. The same
      holds with both slots present, under a rule that requires the grant.
    - Seen failing first: with the refusal taken out, the call runs unregistered, which is the spike's shape.
- **N2: the window before the KV read.**
  - **Confirmed at the spike tag.** `admit_after` read `local_call_checkpoint` (a `kv_get`) before `assess_with_local`
    inserted the window (`keep_call_window`).
    - The feeder offers a `call:` checkpoint only for a scope with a window (`admits_call_scope`). It is woken by a
      coalescing `watch::Receiver<u64>` (`mycelium-core/src/kv_handle.rs:425`) and rescans on each wake
      (`src/agent/mod.rs:2020–2032`).
    - So a revocation written between the read and the insert was never offered.
  - **Resolution:** `ExecutionAuthority::assess_call(…, read_local)` in `src/agent/gateway_authority.rs` takes row 43
    **once**. Under that one hold it does five things in order:
    1. it verifies the mandate: its signature, D-6's issuer rule, N3's holder and its possession;
    2. it inserts the window;
    3. it calls `read_local`, which is `ops::kv_get`, a papaya read with no lock (`mycelium-core/src/ops.rs:210`);
    4. it offers the carried checkpoint and the local copy;
    5. it judges.
  - **Why that closes the race:**
    - A write applied before the read is seen by the read.
    - A write applied after it wakes the feeder after the window exists, because the feeder's `admits_call_scope` takes
      row 43 too.
    - The window is inserted only once the mandate has verified, so an unverified mandate plants nothing.
    - At the gateway, the mint inserts its own window before the first checkpoint's durable write.
  - **No new lock.** Row 43's entry gains *one KV read under the hold* when the build lands.
  - **A constraint on exit 16:** an incremental feeder may not rely on the coalescing watch to carry keys. On each wake
    it reads the checkpoint key of each held window, or it takes keys from a channel that falls back to a rescan when it
    lags.
  - **Test:** a hook inside `assess_call`, between the window's insert and `read_local`, writes the revoking checkpoint
    at that point. A second case writes it just after the read, with the feeder running.
    - Expected: in both cases the work is cancelled within one sweep, and a re-presentation reads `Refused(Revoked)`.
    - Seen failing first, in the spike's order: the work runs to completion, and `view_sizes` holds the scope with
      nothing revoked.
- **N3: the holder.**
  - **Confirmed.** `check()` keeps `(c.principal, c.scopes, c.mandate, c.resource)` (`provider_enforcement.rs:258–261`)
    and drops `GatewayCaller.via` (`src/agent/gateway_caller.rs:214`, checked equal to the frame's sender).
    - `principal` goes to `admit_after` (`:263`, `:297`), then to `reassess_arguments` (`:372`).
    - `assess_digests` refuses `m.holder != caller` (`src/agent/gateway_authority.rs:705`).
  - **Resolution: the slot decides the holder rule.**
    - Slot 2 is a new envelope field, `cm`: a `PresentedMandate` with its checkpoint. It is read into
      `GatewayCaller.call_mandate`, an additive field, since the struct is `#[non_exhaustive]`.
    - In `assess_call` the holder must be `node:{via}`, or `node:{sender}` on an unframed member's call, and
      `established_by` must equal it (D-6).
    - `assess_digests` reads a `call:` scope in slot 1 as `Unknown("a call: mandate is not a caller's")`. A client
      replaying a gateway's `call:` mandate as its own gains nothing.
    - A `call:` possession proof is accepted over the RFC 8785 digest only: the kind is new, so it needs no I3 window.
  - **Tests:**
    - (a) A minted `call:` mandate in slot 2 is `Established` and registered.
    - (b) The same mandate in an envelope from another gateway is `Unknown`.
    - (c) The same mandate in slot 1 through its own gateway is `Unknown`.
    - (d) An `established_by` other than the holder is `Unknown`.
    - Seen failing first: on the spike, which compared the holder with the caller in one slot.
- **N4: both slots registered.**
  - **Confirmed.** These are all single today:
    - `AuthorizedWork.mandate` is one `Option<Mandate>` (`src/mandate/authority.rs:422–430`). It is public and not
      `#[non_exhaustive]`.
    - `Admission.work` is one `Option<WorkGuard>` (`provider_enforcement.rs:64–76`), and `run` selects on one guard
      (`:80–92`).
  - **Resolution: two `begin`s.**
    - `Admission` gains `call_work: Option<WorkGuard>`, and `run` selects on the handler and on both guards.
    - The sweep's grant arm (`gateway_authority.rs:459`) becomes `m.is_grant() || m.is_call()`.
    - Each slot that is `Established` is registered, and under a cancellable rule slot 2 must be (D-2).
    - When the handler future drops, both guards drop. Only a cancelled guard writes a stop record, so the record names
      the mandate whose lapse stopped the work.
    - The public API is unchanged and no lock is added. Row 48 is taken twice, flat, and its entry says so when the
      build lands.
    - `running_count` counts registrations, so a call under both slots counts 2. Exits 1 and 14 assert that.
  - **Test:** a call under a client grant and a `call:` mandate is run twice.
    - Revoking the grant stops it within bound 1, with a stop record naming the grant.
    - Cancelling the `call:` term stops it, with a stop record naming the term.
    - Seen failing first: with one registration, the unregistered mandate's revocation stops nothing.
- **N5: the restart, and retention.**
  - **Nothing collects these records today.** `grant_retention_secs` and `max_drain_secs` do not exist, and no code
    tombstones `grant/`. *"Kept at least until"* therefore holds trivially until a collector is built. The work is the
    collector and its floor (D-5).
  - **A premise the row had not named: the start rule.**
    - `mark_started` makes the view `RevocationView::started_at` (`gateway_authority.rs:505–507`).
    - Only `grant:` is exempt from `IssuedBeforeStart` (`src/mandate/authority.rs:281–287`).
    - A revocation still counts after a restart, because the revoked set is extended before that check (`:271`).
    - But a non-revoked `call:` checkpoint issued before the restart would never become the view's newest, and its call
      would read `Unknown`.
    - **The exemption extends to `call:`**, for a grant's reason: the standing reads no freshness, and the window bounds
      its life.
  - **Resolution:**
    - The window table is not persisted (D-4). N2's read re-creates the window at the next admission, and running work
      does not survive a restart.
    - `call:` gets its own owned key family, `call/{gateway segment}/{term}/checkpoint`, protected at the KV routes. That
      check is at the routes, not a Layer-I guard.
    - The term is derived from the envelope's HLC id. It is not an RNG draw, so it needs no new `sim_seam` row.
    - The first and the revoking checkpoint are written with `write_record_durable` (`src/agent/http/grants.rs:909`).
    - **The collector** runs on `sim_seam::sleep_ms` beside the `authority/sweep` loop (`src/agent/mod.rs:2005–2014`),
      reading `hlc.decision_now_ms()`.
      - Past D-5's floor, the issuing gateway tombstones its own key (`kv_delete`).
      - Any other node collects with `kv_delete_held` at the stamp it observed (`mycelium-core/src/ops.rs:521`, K2's
        residue precedent), so a departed issuer's records are still collected.
      - A record resurrected after the tombstone's GC (`propagation_window_secs`) is harmless: past its window the
        mandate is refused `OutOfWindow`, whatever its checkpoint says.
  - **Tests:**
    - (a) Revoke, flush the WAL, restart WAL-only with no snapshot, and re-present inside the window:
      `Refused(Revoked)`.
    - (b) The same with a non-revoked checkpoint issued before the restart: `Established`. Seen failing first: under
      the `grant:`-only exemption it is `Unknown`.
    - (c) Under the sim clock, a `call:` key is present at the floor − ε and tombstoned at + ε. This holds at the issuer
      and, with the issuer stopped, at a non-issuer.
    - (d) The same pair for `grant_retention_secs`.

**A dependency on I2 (CA succession).**
- **The constraint.** A `call:` mandate and its checkpoints are verified against the gateway's node key
  (`gateway_member_keys`). Once the providers narrow to the successor root, a cancel signed by a gateway still anchored
  only under the old CA cannot be verified. The work then runs to its window: open, but bounded by the window.
- **What I2 must state** (R1–R4): every gateway is anchored under the successor before any provider narrows, and a
  `call:` window is shorter than the succession window.
- **I2's re-judging** of cached judgments drops the `call:` checkpoints held from an old-root signer. The sweep stops
  work only on `Revoked` or past the window, never on `Unknown`, so this re-judging stops nothing by itself.

**Wave 1a's released behaviour.**
- **I3 grants:** slot 1 is unchanged, except that it now refuses `call:`. D-5's floor keeps a `grant/` record, and so
  the grant's use marker, as long as the grant is inside its window.
- **I5 `ot`:** the `call:` holder is the gateway's node key, the key the exchanged token is DPoP-bound to. The envelope
  cap stays at 8 KiB (`gateway_caller.rs:175`), and an OIDC call carries:
  - `ot` with its DPoP proof;
  - the client's grant with `cs`/`cn`;
  - the `call:` mandate with its checkpoint.

  That whole envelope has never been measured, and a producer past the cap returns `None`, so the call fails (exit 17).
  The checkpoint's signature is carried as base64.

**Exit.** Each item is a test or a run. Every D2 decision has an item or is marked *not built*.
1. **Establishment.** A `call:` mandate minted at the gateway's admission is established at a provider that holds no
   checkpoint for it, and the work is registered (`running_count`, which counts registrations: N4). The same call with
   the checkpoint, or the whole of slot 2, stripped reads `Unknown`. It is not registered under a rule that is not
   cancellable. Under a cancellable rule it is refused -32031 `call_mandate_not_established` (D-2). Seen failing first:
   before the row, the stripped form is the only form.
2. **The holder and the second slot** (N3, D-1). A client's call carries the `call:` mandate in slot 2 (`cm`), beside
   the client's own grant in slot 1, and both are assessed: slot 1 against the client principal, slot 2 against
   `node:{via}`. A `call:` mandate from another gateway's envelope is `Unknown`, and so is one in slot 1. A `call:`
   mandate alone does not meet a rule's `requires_mandate`, and its binding is not in `ActionEnvelope.mandate`.
3. **Entitlement, negatively** (D-6). A `call:` mandate is established only when its `established_by` and its `holder`
   are both `node:{via}` of the envelope carrying it, and only a gateway with minting turned on mints one. A `call:`
   mandate signed by another member node is not established, with `entitle_grants_from_fleet` on.
4. **A cancel stops admitted work within bound 1.** Revoking at the gateway ends a running cancellable call at the
   provider within gossip + *F* / 20 + the class's `drain_bound`, asserted against the configured *F*. The stop record
   is in `stop_records`.
5. **The race** (bound 1; N1, N2):
   - A call framed before the revocation and admitted before the revocation reaches the provider is stopped when the
     revoking checkpoint arrives.
   - One whose revocation is already in the provider's KV at `admit_after` is refused there.
   - A revocation written between the window's insert and the KV read, or just after the read, is offered and stops the
     work within one sweep.
   - Re-presenting the call afterwards is refused `Revoked`.
6. **Refused before the handler** (D-3). A revoked term reads `Refused(Revoked)` at `admit_after`, the one point slot
   2 is assessed. The handler does not run, and the record says `Execution::None`. Rev 0.10 said *"at the gateway's
   preflight and at the provider"*. Slot 2 is no longer assessed at the preflight.
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
    [`lock-order.md`](../wiki/dev/concurrency/lock-order.md) states the `call:` windows it now holds, the one KV read
    `assess_call` makes under the hold (N2), and the hold bound, from a measurement on the shipped code. Row 48's entry
    states that a call under both slots takes it twice, flat (N4). D2's *"a lock-order row"* is met by those entries: the
    windows live inside `AuthorityState`, so no new lock is added.
11. **Durability** (N5, D-4).
    - The first and the revoking checkpoint are on disk (`write_record_durable`) before the call is admitted or the
      cancel acknowledged, and the fsync's added admission latency is measured.
    - After a WAL-only restart, a revoked `call:` re-presented inside its window is refused.
    - A non-revoked `call:` whose checkpoint was issued before the restart is established, because `call:` is exempt
      from the start rule as `grant:` is (`src/mandate/authority.rs:281–287`).
    - No window table is persisted.
12. **Retention.**
    - `max_drain_secs` caps how long a cancelled `call:`'s records are kept past its stop, and the sweep ceiling applies
      to `call:` work.
    - `grant_retention_secs` (default 7 days) collects `grant/` records past it.
    - Neither collects below D-5's floor: `valid_until_ms` + 2·skew (+ `max_drain_secs` for `call:`). `validate()`
      refuses a `grant_retention_secs` below the longest grant window the node issues.
    - The issuer collects with `kv_delete`, and any other node with `kv_delete_held`.
    - A test for each shows the records gone after the setting, and kept before it. One case runs with the issuer
      stopped.
13. **The issuer down.** With the admitting gateway stopped, a running `call:`'s work ends at its mandate's window. The
    sweep ends it past `not_after_ms`, and the term then reads expired.
14. **The soak.** 10⁵ cancellable calls over a two-node fleet, half of them cancelled while running.
    - Every cancelled call ends inside bound 1, and `running_count` is asserted as registrations (two per call under
      both slots).
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
    Today's grant feeder rescans `grant/` on every write (`feed_grant_checkpoints`), and the spike kept that shape. The
    wake is a coalescing watch (`mycelium-core/src/kv_handle.rs:425`), so the feeder does not take the written key from
    it. It reads each held window's key, or it falls back to a rescan when a keyed channel lags (N2).
17. **The measurement.** S-cancel's figures are re-run on I4's shipped code and dated on
    [`what-is-proven.md`](../operations/what-is-proven.md) (§6.4). That covers the provider's `assess`, the gateway's
    mint with its durable write, the bytes, and bound 1 with its n. The bytes include the **worst-case envelope**:
    `ot` with its DPoP proof, a client grant with `cs`/`cn`, and the `call:` mandate with its base64 checkpoint, against
    the 8 KiB cap (`gateway_caller.rs:175`). A producer past the cap is shown refusing by name. The spike's row there
    is replaced by the shipped one.

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

#### I7 · The developer's identity — *written from S-presence ([note](spikes/2026-11-07-s-presence.md), 2026-11-07; its recorded run at tag `spike/s-presence-2026-11-07`, the positive Touch ID path in the note's maintainer's run of 2026-11-09) — **buildable (the maintainer's decisions of 2026-11-12: P1–P7, H1, H2, A)*** (D7)

**Decided before the spike** (D7 and the stub this row replaces):
- The consent primitive is **an OS user-presence check whose result the repository verifies, graded on the record**.
- D7's mechanics: a Secure Enclave key (`kSecAttrTokenIDSecureEnclave`), a negative probe (`interactionNotAllowed` must
  fail), the enrolled public key compared on every use, `biometryCurrentSet`, and a fresh `LAContext` per approval.
- D7's grades: `os_presence(macos)` · `tty` (Linux, Windows) · `recording` (a `sim` test build). Windows grades no
  higher than `tty` until shown otherwise.
- From the stub: the key never touches the worktree; `signed_with` *(new)* graded on the record; the prompt renders
  from the fleet's record by id; a bounded request queue (`busy`).
- D1 left the developer's key out of the fleet-certificate decision: "it is I7's question, settled by spike".

N4's identity half and N7 wait on this row; N4's fleet and wiring halves do not.

**From the spike** ([S-presence's note](spikes/2026-11-07-s-presence.md)). **The evidence is split.**
- **The tag `spike/s-presence-2026-11-07`** (`0aaa51f6`, never merged) holds `probe.swift`, `blob.swift`, the
  `verify/` crate, `keys.sh` and `run.sh.out`, the recorded run. That run raised no prompt.
- **The positive path is in the note only.** The maintainer ran it on 2026-11-09 from a worktree of the tag, and the
  note quotes its output. Nothing at the tag records it.

The machine: macOS 26.6.2, Apple M2 Max, Touch ID enrolled. It has no code-signing identity and no provisioning
profile (`0 valid identities found`), and both probes are `adhoc,linker-signed`. "No `cargo build` binary touched the
Secure Enclave."

The stub left the spike **three questions**: the signer, the signed bundle and Windows's grade. `signed_with` was
already decided; the spike bears on what a grade can rest on.

1. **The signer.**
   - **The fleet CA can certify an Enclave key, with nothing new in the tree.** "`rcgen` 0.13's public
     `PublicKeyData` trait, implemented over the bare point with `PKCS_ECDSA_P256_SHA256`, lets an **Ed25519** CA issue
     a client-auth leaf over the Enclave key without its private half" (row 6). The maintainer's run shows the same:
     `webpki.chain_ed25519_ca_to_p256_leaf Ok("ok")` and `webpki.verify_se_signature Ok("ok")`.
   - **`ring` can verify it.** `ring::signature::ECDSA_P256_SHA256_ASN1` is already a direct dependency of
     `mycelium-core` under `tls` (row 5).
   - **It can never be a mesh identity.** Row 7 reads this from the code; it was not run.
     - Both mesh verifiers call `refuse_if_removed` **before** the client-scheme check (`refuse_if_client`), at
       `mycelium-core/src/tls.rs:1357–1358` (client side) and `:1402–1403` (server side).
     - `refuse_if_removed` fails closed on a certificate without an Ed25519 key: "a mesh certificate must carry an
       Ed25519 identity key" (`:1264`).
     - So a P-256 client certificate is refused for its key, and never reaches the scheme refusal.
   - **What a second algorithm through every verifier would cost.** Row 8 counted tree `64fb5096`, before I1. The same
     `keys.sh` at `d974fc76` gives:
     - 73 fn parameters typed `&[u8; 32]` or `&[[u8; 32]]`, 37 of them a parameter whose **name** is `key`, `keys`,
       `verifying_key(s)` or `pub_key_bytes`;
     - 1 parameter typed `[u8; 64]` and 22 `[u8; 64]` mentions;
     - 11 `VerifyingKey::from_bytes`, and 11 `Signature::from_bytes` or `from_slice`.

     These count `file:line`s, not uses: a line holding two counts once, and a parameter list spanning lines is missed.
2. **A signed bundle: no, as the pipeline stands.** "The `macos` job runs `cargo build --release` and tars the
   binaries. It has no `codesign`, no notarization, no `.app` wrapper … and no signing secret" (row 9).
   - **D7's premise that a bundle is needed holds only for a key stored in the keychain.** Both keychains refused the
     store with `-34018` (`errSecMissingEntitlement`, row 3).
   - **An ad-hoc binary reached the Enclave regardless.** Rows 1 and 2 used an **ephemeral** key, never stored:
     created, signed, then refused `-1004` without interaction.
   - **Only row 4 is the blob.** CryptoKit's `dataRepresentation` was written to a file. It restored in a new process,
     where signing still needed presence.
   - **The bridge is missing.** "`dataRepresentation` exists only in CryptoKit, which is Swift-only." A Rust `dev`
     therefore needs a Swift shim, which is not built.
3. **Windows's grade: the spike did not run Windows.** D7's `tty` stands.

**What a grade can rest on.** "A verifier holding only the public key cannot tell what kind of key signed." A
`biometryCurrentSet` Enclave key, an Enclave key with no presence flag and a software P-256 key verify alike. So "an
`os_presence(...)` grade on a record is **self-asserted by whatever enrolled the key**". No attestation of a key's
access control is open to an unmanaged ad-hoc binary.

**The positive path** (the note's maintainer's run of 2026-11-09, not at the tag):
- `blob sign-interactive` raised the Touch ID prompt, and "only then was `out.txt` written"; `ring.verify` returned
  `Ok("ok")` and the tampered message `Err(Unspecified)`.
- A second approval in a **second process** signed and verified.
- A dismissed prompt yielded no signature (`Code=-2 "Canceled by user."`).
- The prompt offered no "Use Password…".
- "The binary has no way to record that a prompt was shown."

**Where the evidence and D7 disagree, and what the maintainer decided (2026-11-12).** D7's text in §2 is unchanged.
This row records the decisions that depart from it, and §2's D7 *Revised from* cell points here.
- **D7's bundle premise.** The evidence: true of keychain keys only. **Decided (P3): the CryptoKit blob path**, with
  no Apple Developer team. The signed bundle is not built.
- **D7's rule, "whose result the repository verifies".** The evidence: the presence flag cannot be verified.
  **Decided (P1): the macOS grade is `os_presence(macos, unbound)`, an enrolment claim**, documented as one that no
  verifier can check. The rule is met for what is verified: the signature, the consent key's fleet certificate and its
  standing. It is not met for presence, and the record and the feature page say so.
- **D7's mechanics on the blob path.**
  - `kSecAttrTokenIDSecureEnclave` and its read-back are the Security framework's. The blob path creates the key
    through `SecureEnclave.P256.Signing.PrivateKey(accessControl:)`, which is Enclave-only.
  - The negative probe ran on the ephemeral key (row 2) and on the restored blob (row 4). The keychain-side probe
    "never ran", and it is moot on this path.
  - "No probe compared a stored public key on use": the row builds the comparison (exit 3).
  - A fresh `LAContext` per approval was "exercised for **one** approval" only: exit 4 runs two in one process.

**Decided by the maintainer (2026-11-12):**
- **P1:** `os_presence(macos, unbound)`, recorded as an enrolment claim and stated as unverifiable by a verifier.
- **P2 = (a):** a **fleet-certified P-256 consent key**, read by **one P-256 verifier where a consent record is
  checked** (`grant_decision::verify`). Its reach is under *What*.
- **P3:** the CryptoKit blob path. No Apple team, no signed bundle.
- **P4, revised by H2 (#764's confirm round):** the grade goes **on the consent record**, and **comes from the consent
  certificate**.
  - The fleet CA writes the grade into the certificate at enrolment. The signed consent body carries the same grade,
    or it is refused.
  - **The grade set** is `os_presence(macos, unbound)` · `tty` · `recording` · `service`. That is D7's three, with P1's
    revision of the first, plus `service` (E1's), which is this row's addition and not a D7 grade.
  - **Why it was revised:** P4 as first recorded let the key's kind separate the grades. P7 makes software and Enclave
    consent keys one kind, so only the enrolment can tell them apart.
- **P5:** a **residual**, stated. A same-user process can read the blob and ask the Enclave for a signature. "It
  cannot produce one without the user's finger", but "it chooses the prompt text (`localizedReason`, under its own
  process name), so the presence proves a person was there, not what they were shown." The blob is written 0600 in a
  0700 directory, which narrows the readers to the user's own processes and does not close the gap. User binding
  across macOS users was not tested.
- **P6:** re-enrolment is **I1's supersession within the consent principal**: a newer consent certificate for
  `consent:{issuer}/{name}` raises that principal's mark, and nothing else (H1).
  - **The mixed-version caveat:** an un-upgraded node's fold reads Ed25519 keys only, so it never sees a consent
    certificate. It cannot judge a consent decision, and its publication door refuses the certificate.
  - So **every node is upgraded before a consent certificate is published**, as I2's line requires every member
    upgraded before `tls commit`.
- **P7:** on Linux and Windows a `tty` consent is signed by a **fleet-certified P-256 consent key held in software**,
  graded `tty`. So there is one consent-key kind and one verifier on every platform, as P2 requires.
  - **The `tty` residual (#764's second confirm round, B), stated.** The key is a same-user file, 0600 in a 0700
    directory, so any process running as the user can sign with it and no terminal is needed at all.
  - That includes the coding agent N4 runs, and an Orca plugin through `terminal:send`.
  - **A `tty` decision may therefore have been made by the agent whose request it approves.** The grade says where
    the key lives, not that a person typed.
  - This is why consent has its own authority (A, below): an operator can grant `grant:issue` to a person's consent
    and not to the service, or the reverse, and a policy can refuse `tty` where it needs more.
  - The residual is stated in the row, on the feature page (beside P1's) and on N7's card.
- **A (the maintainer, 2026-11-12): consent has its own authority.** A consent principal's scopes come from their
  own table, `gateway_consent_principals` *(new)*, never from a named token's (below).
- **H1 (#764's confirm round): a consent key lives under its own principal.**
  - **The defect:** the design before it put a P-256 consent certificate under the service's `token:{issuer}/{name}`.
    It superseded the service's Ed25519 key there, but `feed_client_keys` feeds no P-256 mark and withdraws nothing.
    So `TrustedExternalIssuers.clients` kept the old Ed25519 key, and `verify_client` (`knowledge/issuer.rs:275`) went
    on accepting a superseded key's possession proof at `GrantVerifier::check`, the preflight and
    `reassess_arguments`.
  - **The fix:** a consent key lives under its **own principal**, `consent:{issuer}/{name}`, beside the service's
    `token:{issuer}/{name}`. Nothing supersedes across the two, and a developer may hold both.
  - **The service key is not a consent key.** An Ed25519 key presented as a consent key is refused (exit 5).

**Nothing is left open.** What remains is build work, not questions:
- the Swift shim;
- the software consent key on Linux and Windows;
- the consent issue flow and its grade extension;
- the consent principal's fold and verifier;
- the release's `macos` job building the shim.

**What:**
- **The consent key, by platform.** One kind everywhere: P-256, certified by the fleet CA, never in the worktree.
  The directories, `generate_pkcs8` and the Windows ACL are **this row's choices**, and the build PR may change them in
  the open.
  - **macOS (P3):** the shim creates the key in the Secure Enclave and stores its CryptoKit blob under
    `~/Library/Application Support/mycelium/consent/`. Its certificate carries `os_presence(macos, unbound)`.
  - **Linux (P7):** `dev` generates the key in software with `ring` (`EcdsaKeyPair::generate_pkcs8` with
    `ECDSA_P256_SHA256_ASN1_SIGNING`) and writes it as PKCS#8 under `$XDG_DATA_HOME/mycelium/consent/`, which
    defaults to `~/.local/share/mycelium/consent/`. Its certificate carries `tty`, and the approval is typed at `dev`'s
    terminal.
  - **Windows (P7):** the same generation, stored under `%LOCALAPPDATA%\mycelium\consent\`. Windows has no POSIX
    modes, so an ACL granting only the user stands for 0600/0700, and the build PR states the ACL it sets. Its
    certificate carries `tty`.

  On macOS and Linux the file is written 0600 in a 0700 directory, as v2.38.2 and #699 require of private keys.
  `dev` refuses a consent key file that anyone other than the user can read.
- **The consent principal (H1).** A consent certificate's SAN is `mycelium-consent://<issuer>/<name>` *(new scheme)*,
  and its principal is `consent:{issuer}/{name}` *(new)*.
  - **The fold.** I1's client-certificate fold reads it under its own principal, at
    `sys/client-cert/{principal-hex}/{key-hex}`, since the principal differs. It keeps its own mark, so no consent
    certificate supersedes a `token:` key and no `token:` certificate supersedes a consent key.
  - **The scheme is bound to the key kind.** These have no facts and never count:
    - a `mycelium-consent://` certificate over an Ed25519 key;
    - a `mycelium-client://` one over a P-256 key;
    - a certificate naming **both** `mycelium-client://` and `mycelium-consent://`, which counts as neither, as
      `client_identity` already refuses a certificate naming both a client and a node URI.
  - **The key's hex in the record path.** A P-256 key's `{key-hex}` in `sys/client-cert/{principal-hex}/{key-hex}` is
    the **uncompressed X9.63 point**, 65 bytes and so 130 hex digits. That is the form the spike printed
    (`pub_x963_hex`), and its length tells it from an Ed25519 key's 64 digits. `parse_hex32` is widened by kind, and
    the encoding is pinned in a golden (exit 1).
  - **The mesh.** Both mesh verifiers refuse it in `refuse_if_removed` (no Ed25519 key). `refuse_if_client` is widened
    to the new scheme too, so the scheme refusal does not rest on the key alone.
  - **Who may decide (A).** `decider_holds_grant_issue` (`http/grants.rs:200`) today matches a `token:` decider
    against the named-token table. A `consent:` decider is matched against a **consent table**,
    `gateway_consent_principals` *(new `GossipConfig` field)*. Each entry is `{ name, scopes }`, with the scopes
    limited to `grant:issue` (`validate()` refuses any other by name).
    - **Why a table rather than a `consent_scopes` field on the named-token entry:** a named-token entry requires a
      bearer `token`, and a consent principal never presents a bearer. A field there would mint a bearer secret for a
      person who only decides.
    - With the table, an operator grants `grant:issue` to a person's consent without the service, or to the service
      without the consent, and revokes either alone by editing one entry.
    - `GOSSIP_GATEWAY_NAMED_TOKENS`'s format is unchanged.
  - **The grade at the evaluator (included).** The `ActionEnvelope` the decision routes evaluate
    (`ae_preflight` at `ENFORCEMENT_POINT_GRANT_ISSUE`, `http/grants.rs:350`) gains `signed_with` *(new field;
    additive, the type is `#[non_exhaustive]`)*.
    - A policy can then require a grade for an action, as **a set of accepted grades** rather than a minimum. The
      grades are not ordered: `tty` and `service` are both a same-user file, and `recording` exists only in `sim`.
    - An unsigned decision's envelope carries none.
  - **The upgrade note.** An existing evaluator policy keyed on `token:` principals does **not** match a `consent:`
    decider, so a deployment that admits developers' decisions by their `token:` principal adds rules for their
    `consent:` principals. The reference evaluator, the Cedar adapter's re-pin and `deprecations.md` say so.
    - An older binary ignores `gateway_consent_principals` silently: `GossipConfig` refuses no unknown key. It admits
      no consent decider either way, since its fold has no consent principal.
  - **The record.** `grant_decision::verify` names the consent principal as the decider. I3's decision record and its
    evidence name it as the actor (the `DecisionActor`'s principal), with `signed_with` beside it.
  - **Never possession.** `feed_client_keys` feeds `token:` principals only, so a consent principal's key never reaches
    `TrustedExternalIssuers.clients`, and the possession proof never sees it.
- **The grade in the certificate (H2).**
  - **The extension.** The fleet CA writes the grade as a non-critical X.509 extension under a **`2.25` UUID OID**
    (ITU-T X.667: `2.25.` followed by a UUID's decimal value). That needs no IANA PEN.
    - The build PR draws the UUID once and pins the OID in a golden, with the grade's DER encoding: an `UTF8String`
      holding the grade's exact text (exit 1).
    - It is written with `rcgen`'s `CustomExtension`.
    - Reading it needs a **new DER extension reader in `cert_identity`**: webpki exposes no extension accessor, and
      `cert_identity` already walks the `TBSCertificate` by hand for the serial and the SANs.
  - **Who decides the grade.** The issuer's host decides it at issue (`--grade`, below); the developer does not choose
    it per decision.
  - **No grade, no consent.** A consent certificate with no grade extension, or with one outside the set, never
    counts.
- **The consent issue flow, over a public key alone, with the CA off-node** (L7, M-new-1):
  1. **The developer's side.** `dev` creates the key (the shim's Enclave key on macOS, `ring`'s elsewhere) and exports
     the public key as **SPKI PEM** (`-----BEGIN PUBLIC KEY-----`, the P-256 `id-ecPublicKey` form). CryptoKit's
     `pemRepresentation` gives it, and so does `ring`'s uncompressed point under the fixed SPKI header. Raw X9.63 is
     not accepted, so the input names its own algorithm.
  2. **The issuer's side.** `mycelium tls issue --client <name> --consent --public-key <spki.pem> --grade <grade>
     --ca-dir <dir> --issuer <issuer> --out <dir> [--force]` *(new flags: `--consent`, `--public-key`, `--grade`)*.
     - `--ca-dir`, `--issuer` and `--out` are required as for today's `--client`.
     - `--public-key` and `--grade` are required with `--consent`.
     - It refuses an SPKI that is not P-256, and a grade outside `os_presence(macos, unbound)`, `tty` or `recording`.
       `service` is never a consent grade.
     - It writes `consent-{issuer}-{name}.cert.pem` under `--out`, and **no key file**.
     - **The `--force` guard is keyed on that `.cert.pem`.** If the file exists, the issue is refused unless `--force`
       is passed, because once published the new certificate supersedes the old consent key. This mirrors
       `tls_issue_client`'s guard on the service key (`src/main.rs:815`).
     - It issues over the public key without its private half, as row 6 did with `rcgen`'s `PublicKeyData`.
  3. **Back to the developer.** The certificate is copied back, and `dev` publishes it through I1's door.

  No private key ever reaches the issuer's host, and the CA key never reaches the developer's machine.
- **(a)'s reach, exactly.** The P-256 key widens these:
  - **I1's door and fold.** The key read (`cert_identity` and `client_identity` read Ed25519 only, through
    `ed25519_key_from_cert_der`), `client_cert::facts`/`judge`, `publish`, and the record path `client_cert_key`. The
    fold also gains the consent scheme and the grade.
  - **Supersession.** The fold's `Mark` and the journal's entries, within the consent principal.
  - **`client_key_standing`**, including the public `GossipAgent::client_key_standing`.
  - **One P-256 verifier at `grant_decision::verify`** (and `SignedDecision::verify_signature`). The key is decoded
    by its kind and verified with `ring`.
  - **`issue_client_cert`**, through the consent flow.

  It does **not** widen these, by decision:
  - `client_sig` (`check_bound`, the call's `cs`, and its replay window): a consent key decides; it does not call.
  - The possession proof (`verify_client` at `GrantVerifier::check`, fed by `feed_client_keys`): a consent principal is
    never fed (H1).
  - The provider's checks, which see `cs` and possession only.

  A consent key presented at any of them is refused or ignored: exit 8.
- **`signed_with` on the consent record.** The signed decision body carries the grade inside the signature, so a relay
  cannot alter it. Today the body carries the key `k` but neither its kind nor a grade (`SignedDecision`,
  `src/agent/grant_decision.rs`).
  - **A consent decider's grade.** `grant_decision::verify` refuses a body whose grade differs from its certificate's.
    The gateway records the certificate's grade beside the decider on I3's decision record.
  - **A service decider's grade.** A `token:` decider signing with its Ed25519 key records `service`. A body from it
    naming no grade (today's body, from a pre-I7 SDK) is recorded `service`, and one claiming any other grade is
    refused.
  - **A consent decider's body with no grade is refused**, by name, before the signature is checked against a
    certificate grade.
  - **The signed message.** The grade is appended to `grant_decision_message` **after `t`** (the `u64` LE
    `issued_at_ms`), only when present: a `u32` little-endian length, then the grade's bytes.
    - An empty grade, a length of 0, is refused.
    - So today's message, and every signature over it, is unchanged.
    - `tests/fixtures/decision/vector.json` pins both forms, with a grade and without, in all three languages.
  - **`recording`** is accepted only in a `sim` build.
- **The residual (H2, consistent with P1).** The grade is what enrolment attested: the issuer's host was told which
  kind of key it certified, and no verifier can check that.
  - A software key enrolled as `os_presence(macos, unbound)` would carry that grade.
  - What the certificate adds is that the grade is the **issuer's** statement, signed by the fleet CA, and not the
    signer's per-decision claim.
- **The shim.** A Swift helper, built by the release's `macos` job, does the following:
  - creates the key with `[.privateKeyUsage, .biometryCurrentSet]` and no `.devicePasscode`;
  - writes its `dataRepresentation` 0600 in a 0700 directory under the user's home, never the worktree;
  - exports the SPKI PEM;
  - signs under a fresh `LAContext` per approval;
  - compares the restored key's public point with the enrolled certificate's before signing.
- **The prompt.** `dev`'s prompt renders from the fleet's record, looked up by request id (the request, the tool and
  the arguments' digest), never from text the caller passes. A bounded request queue answers `busy` when full.
- **`recording`.** A `sim` build signs under the `recording` grade, so E6's walk runs in CI without a person.

**API changes** (each with a `deprecations.md` entry, or stated unchanged):
- **Widened under (a):**
  - `mycelium_core::tls::CertIdentity.key` and `ClientIdentity.key` (`[u8; 32]`), which also gain the scheme and the
    grade;
  - `issue_client_cert(ca_dir, issuer, name, out_dir)` (`tls.rs:1061`), which today generates an Ed25519 key and
    writes it. It gains a sibling that takes a public key and a grade and writes no key, or a variant argument; the
    build PR chooses which, with its entry;
  - `client_cert_key(principal, &[u8; 32])`;
  - `ClientCertPublished.key` (`client_cert::Published`);
  - `ClientKeyStanding`'s argument at `GossipAgent::client_key_standing(&str, &[u8; 32])`;
  - `SignedDecision`, which gains the grade, and `grant_decision_message`, which appends it (a published format);
  - `GossipConfig`, which gains `gateway_consent_principals` (an exhaustive literal breaks);
  - `ActionEnvelope`, which gains `signed_with` (additive, `#[non_exhaustive]`).
- **Unchanged by decision:**
  - `ClientSigned.key`, and `ClientSigner::new(ed25519_dalek::SigningKey, …)` (`client_sig.rs:304`): a consent key
    never signs a `cs`;
  - `TrustedExternalIssuers::set_client_key`: no consent principal is fed;
  - lock-order row 76 (`ClientReplayWindows`, keyed by `[u8; 32]`).

  The build PR keeps each one unchanged, or adds its entry.

**Depends on:** S-presence (done); I1 and I3 (released in v2.40.0), for the client-certificate door, fold and
supersession, and the signed decision this row grades; the maintainer's decisions of 2026-11-12 (P1–P7, H1, H2, A).

**Enumerate:** every place a client key is read, verified, fed or issued today, found by these greps:
- `grep -n "fn verify_bytes\|fn ed25519_key_from_cert_der\|fn cert_identity\|fn client_identity\|fn issue_client_cert\|fn refuse_if_removed\|refuse_if_client(" mycelium-core/src/tls.rs`;
- `grep -rn "client_key_standing\|fn check_bound\|fn check_at_door\|client_sig::check\|grant_decision::verify\|gd::verify\|fn verify_signature\|verify_client\|set_client_key\|fn feed_client_keys\|fn judge\|fn facts\|type PrincipalWindow\|fn client_cert_key\|fn parse_hex32\|fn decider_holds_grant_issue\|ENFORCEMENT_POINT_GRANT_ISSUE" --include='*.rs' src | grep -v _tests.rs`;
- `grep -n "class ClientSigner" mycelium-py/src/mycelium/client_sig.py mycelium-ts/src/client_sig.ts`.

The signature checks (`verify_bytes`, `check_bound`, `grant_decision::verify`, `verify_client`) take an Ed25519 key
of 32 bytes and a signature of 64. The rest handle the key by its 32-byte type without verifying: standing, the record
path, feeding, issuing. Each entry is marked **reached** (widened under (a)) or **not reached** (refuses or ignores a
consent key):
- **The primitive.** `mycelium_core::tls::verify_bytes(&[u8; 32], …)` (`tls.rs:1480`): Ed25519 `verify_strict`,
  called by every signature check below. It is **unchanged**, and the P-256 verifier sits beside it.
- **The key read from a certificate** (**reached**). `cert_identity` (`tls.rs:252`) reads its key through
  `ed25519_key_from_cert_der` (`tls.rs:1104`), which returns `None` for any other SPKI. It reads the serial and the
  SANs by its own DER walk, and reading the grade needs a **new extension reader** there, since webpki exposes none. So `client_identity`
  (`tls.rs:229`) returns `None` for a P-256 leaf, and `client_cert::facts` (`client_cert.rs:331`) and `judge` (`:308`)
  never count it. Today a P-256 client certificate is **ignored by the fold and refused by the publication door**
  (`client_cert::publish`, `POST /gateway/identity/client-cert`), and silently so at an older node (§5).
- **The key's standing** (**reached**, no signature). `client_cert::client_key_standing` (`client_cert.rs:376`) over
  the fold's `Mark { key: [u8; 32] }`; the record path `client_cert_key` (`:99`) and `parse_hex32` (`:87`); the
  journal's entries; and `GossipAgent::client_key_standing` (`src/agent/mod.rs:2434`).
- **`grant_decision::verify`** (**reached**: the one P-256 verifier). `grant_decision.rs:184` decodes a 32-byte key
  and a 64-byte signature, then checks `client_key_standing` at `:206`. It is called from the decision routes
  (`http/grants.rs:155`), and `decider_holds_grant_issue` (`:200`) follows it. Beside it is the public
  `SignedDecision::verify_signature` (`:71`).
- **`client_sig`** (**not reached**). `check_bound` (`client_sig.rs:478`) is reached through `check` at the provider's
  receive boundary (`gateway_caller.rs:1298`), and through `check_at_door` at `/mcp` (`http/mcp_bridge.rs:146`) and at
  both A2A doors (`a2a.rs:864`, `a2a.rs:1255`). Its replay window, `PrincipalWindow` (`:534`), is lock-order row 76.
- **The possession proof** (**not reached**). `TrustedExternalIssuers::set_client_key`, `client_key` and
  `verify_client` (`knowledge/issuer.rs:255`, `:269`, `:275`; `clients`, `:220`), with `SharedExternalIssuers`'s setter
  (`:312`). They are fed by `client_cert::feed_client_keys` (`client_cert.rs:560`, which feeds without verifying, and
  under H1 feeds `token:` principals only) and read at `GrantVerifier::check` (`mandate/grant.rs:312`).
- **The provider's checks** (**not reached**). The possession proof is checked again at the gateway's preflight
  (`gateway_authority.rs:742`) and at the provider's re-assessment in `admit_after` (`provider_enforcement.rs:353`,
  `reassess_arguments`); the provider's `cs` check is at `gateway_caller.rs:1298`.
- **The signers, the issuer and the vectors.**
  - `ClientSigner` in Rust (`client_sig.rs:286`; `new` at `:304`, taking an `ed25519_dalek::SigningKey`): unchanged.
  - Python (`client_sig.py:219`) and TypeScript (`client_sig.ts:193`), each refusing a non-Ed25519 key: unchanged.
  - `issue_client_cert` (`tls.rs:1061`, which issues and does not verify): **reached**, through the consent flow.
  - The `cs` vectors under `tests/fixtures/cs/`: unchanged. A decision vector carrying the grade is new.
- **Never reached.**
  - The mesh: `refuse_if_removed` (`tls.rs:1255`), which refuses for the key before `refuse_if_client` runs.
  - `verify_signed_by`'s member and configured-issuer arms (`knowledge/issuer.rs:349`: "a client key is never an
    issuer's").
  - The removal authority (`src/membership.rs`: "never entitles a client").
  - `ProviderBudget`'s set (`provider_enforcement.rs:98`, its `external` at `:104`: "Not fed client keys").

**Exit** (each item a test or a run; the ones a person runs are dated on
[`what-is-proven.md`](../operations/what-is-proven.md)):
1. **The issue flow.** On a host holding the CA key and no private consent key, a consent certificate is issued over a
   P-256 SPKI PEM alone, with its grade.
   - `webpki` verifies the chain. The certificate carries the `mycelium-consent://` SAN and the grade extension, and
     the door publishes it. The fold reads it under `consent:{issuer}/{name}`, and `client_key_standing` answers
     `Current`.
   - The flow refuses a non-P-256 SPKI, raw X9.63, a missing `--grade`, a grade of `service`, and an existing
     `.cert.pem` without `--force`. Each is refused by name.
   - **Goldens:**
     - the grade extension's OID (the chosen `2.25` UUID OID) and each grade's DER encoding, read back by the new
       extension reader in `cert_identity`;
     - the record path's `{key-hex}` for a P-256 key (the 130-digit X9.63 point).
   - A certificate naming both `mycelium-client://` and `mycelium-consent://` counts as neither, for either
     principal.
2. **A signature only after the prompt** (a person's run). The shim is the one the release's `macos` job built,
   obtained by downloading the release tarball through a browser, as an adopter would.
   - `dev` signs a grant decision only after Touch ID is approved, and a dismissed prompt yields no signature and no
     decision.
   - The decision verifies at `grant_decision::verify` and is recorded with the consent principal as decider and
     `signed_with = os_presence(macos, unbound)`.
   - **Gatekeeper on the quarantined, ad-hoc tarball** is run here and recorded: does the shim launch, and with what
     prompt or refusal? If the run cannot be made, it is stated *not run* on `what-is-proven.md` with the reason, and
     the row says how an adopter clears quarantine.
3. **The negative probe and the key on use.** Signing without interaction fails (`-1004`) on the blob as `dev` stores
   it. A blob whose restored public point differs from the enrolled certificate's is refused by name before any prompt.
4. **A fresh `LAContext` per approval** (a person's run). Two approvals in one `dev` process each raise their own
   prompt, and the second is not satisfied by the first. If the run cannot be made, it is stated *not run* on
   `what-is-proven.md`, and the row does not close.
5. **The grade.**
   - **A body whose grade differs from its certificate's is refused**: each grade against each other grade.
   - A relay altering the grade breaks the signature.
   - A `token:` decider's body claiming any grade but `service` is refused, and one naming no grade is recorded
     `service` (a pre-I7 vector verifies unchanged).
   - A `sim` build records `recording`, and a non-`sim` build refuses it.
   - **An Ed25519 service key presented as a consent key is refused** at every point it could be presented:
     - as a consent decider: its `mycelium-client://` certificate gives no consent principal;
     - under a consent principal: a `mycelium-consent://` certificate over an Ed25519 key never counts.
6. **A `tty` consent (P7).** This runs on Linux, and on Windows where CI has a runner (otherwise *not run*, stated).
   - `dev` generates the software consent key and stores it as *What* says, and its certificate is issued with
     `--grade tty` (exit 1).
   - A grant decision approved at the terminal and signed by that key verifies at `grant_decision::verify`, and is
     recorded with its decider and `signed_with = tty`.
7. **Separate principals (H1).** One person holds an Ed25519 service certificate (`token:{issuer}/{name}`) and a
   P-256 consent certificate (`consent:{issuer}/{name}`). Each issue order is tested: both keys stay `Current`, and
   neither supersedes the other.
   - The service key still proves possession at `GrantVerifier::check`, the preflight and `reassess_arguments`.
   - The consent key is never fed to `TrustedExternalIssuers`.
   - Re-issuing the service key supersedes the old service key and only it: the old key's possession proof is refused
     at all three. The consent key is unaffected.
   - Seen failing first, with the consent certificate published under the `token:` principal.
8. **Not reached, by test.** A consent key is refused or ignored as a `cs` signer, as a possession proof, as a
   provider's caller, as a configured issuer, as a removal's authority, and by `ProviderBudget`. At the mesh it is
   refused with the reason **"a mesh certificate must carry an Ed25519 identity key"** (`refuse_if_removed`), and an
   Ed25519 certificate carrying the consent scheme is refused by the widened `refuse_if_client`. Each is seen failing
   first where the row widens a type.
9. **The prompt from the record.** `dev`'s prompt names the request, the tool and the arguments' digest read from the
   fleet's record by id. A request whose record is absent prompts nothing, and the queue answers `busy` past its bound.
10. **The key off the worktree.** The blob (macOS) and the software key (Linux) are outside the worktree, written
    0600 in a 0700 directory. The Windows file carries the user-only ACL. A wider file is refused by name.
11. **Re-enrolment (P6).**
    - A newer consent certificate supersedes the older consent key, which is then refused at
      `grant_decision::verify`, including after a restart under `[persistence]`.
    - **Mixed versions**, shown against the previous binary: an un-upgraded node ignores the consent certificate, and
      its door refuses it. That is why every node is upgraded first (§5).
12. **A blob across shim builds.** A blob created by one shim build signs under an upgraded build of the shim. If the
    build PR cannot produce two release builds, this is stated *not run*, and a re-enrolment (exit 11) is the
    documented recovery.
13. **The measurement.** The probe's results are re-run on the shipped shim and dated on `what-is-proven.md` (§6.4).
14. **Consent's own authority (A).**
    - A `consent:` decider with `grant:issue` in `gateway_consent_principals` decides, while the same person's
      `token:` principal, holding no `grant:issue`, is refused `decider_lacks_scope`.
    - The reverse holds too: the `token:` decider has it and the consent principal has none.
    - Removing either entry revokes that principal alone.
    - `validate()` refuses a consent entry with any scope but `grant:issue`.
    - A policy keyed on `token:{issuer}/{name}` does not admit the `consent:` decider: the upgrade note, shown.
15. **The grade at the evaluator.** A policy accepting only `os_presence(macos, unbound)` for an action refuses the
    same decision signed at `tty` and at `service`, and admits it at `os_presence(macos, unbound)`. The evidence record
    names the grade it decided with.

**Not built, stated:**
- A team-signed bundle, a keychain-stored key, notarization (P3).
- Attestation of a key's access control: none is open to an unmanaged binary, so the grade is the enrolment's (H2's
  residual).
- A Windows presence grade, and Linux presence beyond `tty`: both out of scope by §7.
- A consent key as a `cs` signer, a grant holder or a mesh identity.
- M-of-N.
- **The `tty` residual (B):** a same-user process can sign a `tty` consent with no terminal. It is stated, not closed.
- Not run, and kept so unless a build run makes it: the added-fingerprint leg; a passcode reaching the key by another
  route; user binding across macOS users; device binding.

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

#### N7 · The Orca plugin — *after I7 (its P1, P3 and P7)*

A view over routes, holding no key: `workspace.created`/`removed` (the current names) to offer `dev` via
`terminal:send` — a command channel, which is why consent is not a terminal; a card per `grant_request` from `watch`;
the decision requested from `dev`, consent at the OS (I7), the record's `signed_with` grade shown; `explain` in the
panel. Orca facts at `981ad63b`: capabilities `workspace:read`, `agent:read`, `terminal:send`, `notifications:show`,
`storage:own`, `secrets:own`, `settings:own` (`storage`, `secrets`, `events:subscribe` are aliases); **no network
capability**, so the plugin's HTTP is ungated and stated; **plugin notifications reach phones through the push
gateway**; a manifest declares `engines.orca` as a **floor**, not a pin, so the plugin names the floor it was tested at
and the exact release of its recorded run; the catalog label for a non-official plugin is "Unverified" or
"`<catalog> · <owner>`". **Exit:** on the recorded release, a workspace gets a dev fleet from the offer; a card within
the stream's latency; approval only after the OS prompt with `os_presence(macos, unbound)` on the record (I7's P1 and P3: the blob
path, so the terminal cannot satisfy it on macOS), `tty` elsewhere and said so on the card, with what `unbound` means and the `tty` residual's text (I7, B): "on this platform the consent key is a file any process running as you can use; this decision may have been made without you"; a plugin typing into `dev`'s terminal produces no signature on macOS; the Orca half *run when
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
Wave 1a (identity for services, one MINOR):  I0 ──> I1 ──> I3 ──> S-exchange ──> I5
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
  carrying `ot` as `not_in_build`; an envelope past 8 KiB is refused `caller_context_too_large`, as today. A gateway
  that exchanges needs the token endpoint, its client id and the secret's environment variable, each refused by name at
  start when unset; a provider verifying `ot` needs its own `[oidc]` table with `exchange_audience` (rev 0.11) and
  `exchange_client_id` (the `azp` it accepts), and still fills today's required `audience`. Nothing changes for an
  `[oidc]` table without `exchange_audience`. At a verifying provider an `oidc:` action without an `ot` is refused, so
  upgrade the gateways and set their exchange before the providers set `exchange_audience`.
  - **An older binary ignores `exchange_audience` silently** (#757's confirm round): `OidcConfig` accepts unknown keys
    (it has no `deny_unknown_fields`). So a provider configured to verify before it is upgraded **enforces nothing**:
    it admits an `oidc:` action as today, without an `ot`. Upgrade the provider's binary before relying on its setting.
  - **A provider-only `[oidc]` table opens that node's own gateway door to the IdP's users.** Any `[oidc]` table counts
    as a credential model (`gateway_credential_model`, `src/agent/guarantee.rs`), so a provider node with an HTTP gateway
    then admits the IdP's users there with the table's `group_scopes`. Its issuer and JWKS hosts are checked against
    `egress.allow_hosts` at start (`src/agent/lifecycle.rs`). Give a provider-only table no `group_scopes`, or no gateway
    port, if that door should stay shut.
- **I2:** a `ca_cert_pem` holding more than one certificate is refused by name at start. **Before upgrading, an operator
  whose `ca_cert_pem` holds an extra certificate removes it**, keeping only the fleet CA's: the extra certificate was
  ignored before, so removing it changes nothing. Only `ca_cert_pem` is affected, not `cert_pem` or the on-node CA, and
  the upgrade note says so (`deprecations.md`). The successor-issued certificate and the phase live under
  `auto_cert_dir/succession/`, and the operator's files are never written. Every member is upgraded before `tls commit`.
- **I4:** written with its row.
- **I7:** **every node is upgraded before a P-256 consent certificate is published** (P6).
  - An un-upgraded node's client-certificate fold reads an Ed25519 key only (`ed25519_key_from_cert_der`), so it
    ignores a consent certificate silently, cannot judge a consent decision, and its publication door refuses one.
  - A consent key lives under its own principal, `consent:{issuer}/{name}` (H1), so no node, upgraded or not, sees it
    supersede a service's `token:` key.
  - At the mesh, a consent certificate is refused by `refuse_if_removed` before the client-scheme check runs, because
    it carries no Ed25519 key. That holds on every version I1 runs on.
  - A pre-I7 SDK's signed decision, naming no grade, verifies unchanged and records `service`.
  - **An evaluator policy keyed on `token:` principals does not match a `consent:` decider**: add rules for the
    `consent:` principals, and their entries in `gateway_consent_principals`, before developers decide.
  - `SignedDecision` gains the grade, and `issue_client_cert` a public-key form. These and the other API types (a)'s
    reach widens are listed in I7's row, each with a `deprecations.md` entry.
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
5. The private Cedar adapter re-pins on I3 (§9 O7, delivered at v2.40.0, private #34) and, later, I4.

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
| I1 (second half, PR 4 of 4) — **I1 closed** | **The door's own `cs` check** (from #744's review). **Enumerated** (`grep -rn "client_sig::at_door" src`): three doors read `cs` — `/mcp` (`http/mcp_bridge.rs`) and the A2A unary send and stream (`a2a.rs`); each ran `at_door` and then `ae_preflight`, which claims a grant's use. Each now runs `client_sig::check_at_door` between the two: the signature over the door's own binding (`door_binding_mcp`, `door_binding_a2a`) and `client_key_standing`, sharing `check`'s code (`check_bound`); `cn` is left to the provider, which checks everything again; an unauthenticated caller is left to the provider. `a_signed_call_presenting_a_client_grant_runs_at_mcp`: a call with a signature over other arguments is refused `client_signature_invalid` at the gateway, and the honest call under the **same** grant then runs — seen failing first with the `/mcp` check removed (the honest call refused: the grant already used at the gateway); `both_a2a_doors_carry_a_client_signature_and_the_skill_sees_it`: a tampered send and a tampered stream refused by name and never reaching the skill — seen failing first with each A2A door's check removed, one build each. **The exit run:** `examples/eve_weather/run_client_grant.mjs`, the release builds, three runs, dated on [`what-is-proven.md`](../operations/what-is-proven.md#client-grant-three-node-fleet); CI's `eve-fleet` job runs it on debug builds. **Closing I1, promise by promise, across #741–#744 and this PR:** *"`mycelium tls issue --client <name>`: an Ed25519 certificate with SAN `mycelium-client://<issuer>/<name>`"* — `issue_client_cert`, `tls_issue_client`; `a_client_identity_is_read_exactly_and_bad_segments_are_refused_at_issue` (#741). *"the key held by the service's process"* — the key file `mycelium tls issue` writes owner-only; the SDKs' `ClientSigner` reads its PEM (#741, #744). *"Both mTLS verifiers refuse the scheme (seen failing first)"* — `a_client_certificate_is_refused_at_the_mesh_both_ways`; the ledger's [mesh row](../operations/what-is-proven.md#client-certificate-refused-at-mesh) against the release binary (#741). *"Revocation of a client key is its re-issue, by supersession … a high-water mark … in memory and in the node-local journal … a certificate at or below the mark is refused for good … its own line in that budget … two certificates in one millisecond tie-break on the greater key"* — the fold's `Mark`; `a_newer_certificate_supersedes_the_older_key_and_an_overwrite_cannot_lower_it`, `a_record_written_by_a_member_supersedes_on_arrival_before_anyone_asks`, `a_supersession_survives_a_restart_under_persistence_with_the_newer_record_gone`, `repeated_identical_publishes_leave_the_journal_bounded` (#741); on the fleet, the older key refused `client_key_superseded` after a newer certificate (this PR's run). The `sys/revocation/` client arm the row offered as the maintainer's alternative was not taken. *"`POST /gateway/identity/client-cert` … under `identity:write` … `compliance`-gated … publishes `sys/client-cert/{principal-hex}/{key-hex}` … the fold judges by the record's signature and the certificate's CA issuance, never by KV LWW time"* — `gw_identity_client_cert`; `the_client_cert_door_is_identity_write_and_publishes_what_it_judged`, `the_client_cert_door_checks_its_issuer_and_audits_the_bearer`, `the_publish_refuses_a_node_certificate_a_foreign_ca_and_a_superseded_key`, `junk_records_sorted_ahead_cannot_hide_a_newer_certificate` (#741, #742). *"`TrustedExternalIssuers` shared at `ExecutionAuthority`, `ProviderBudget`, `MembershipAuthorities` (the last never entitles a client) … a lock-order row"* — `SharedExternalIssuers`, lock-order row 77; narrowed in #743's review: a client key verifies a possession proof and nothing else (`a_client_key_proves_possession_only_and_a_superseded_one_never_returns`, `a_client_key_in_a_shared_set_never_entitles_a_removal`) (#743). *"A `token:` principal with a certificate on record and no `cs`: counted (`gateway_actions_unsigned{kind}`), then refused"* — **counted**: `the_mcp_door_carries_a_client_signature_counts_an_unsigned_one_and_needs_an_issuer`, `an_unsigned_call_for_a_certificate_holder_is_counted_at_the_provider` (#742); **refused: not built** — it waits a release with the counter visible, so an operator sees who would be refused before anyone is; §9 **O3**. *"a key-less principal: D6's window"* — I5's, unchanged. *"A client's call at a gateway without `gateway_identity_issuer` is refused by name at call time"* — `gateway_identity_issuer_unset` at all three doors (#742). *"`cs`/`cn` (D1′): `frame_with_context_at` writes them; `verify` checks `cs` before the cache; the signature vector … checked by Rust, jest and pytest"* — `frame_with_client_signature_at`, `the_provider_checks_the_client_signature_binding_signature_and_key_standing`, `the_client_signature_vector_is_the_same_in_every_language`, `the_canonical_arguments_are_the_same_in_every_language` (#742); the gateway now also checks the signature before its preflight (this PR). *"`grant/` keys: `node:` literal, client principals hex, readers decode by prefix"* — `grant_holder_segment`, `grant_holder_from_segment`, `grant_holder_from_path` (#743, the SDKs' URL shape in #743's review). *"The resource a client-held grant's decisions evaluate against … the provider node of the tool, resolved from the `tools/` scan as `/mcp` does"* — resolved when the request is written, signed into the grant (`bound:resource:`), evaluated at revocation without a live provider — a departure made in #743's review, `a_client_grant_is_revoked_against_its_issued_resource_with_the_provider_gone`. *"A client request door `POST /gateway/grants/request` under `mcp:invoke`"* — `gw_grant_request`, with `GET /gateway/grants/request/{request}` beside it (#743). *"SDKs: a `tools/call` client (new in both) and the A2A client carry `cs`/`cn`; `mycelium-ts` via `node:crypto`, `mycelium-py` via `cryptography`"* — `McpClient`, `A2aClient(signer)`, the grant verbs and the possession proof, `mycelium-py[sign]` (#744). *Exit:* *"a client certificate at the mesh refused by both verifiers; a pre-row-Q node certificate still admitted"* — `a_client_certificate_is_refused_at_the_mesh_both_ways` and the mesh row (#741). *"Y6's weather tool called through `/mcp` under a grant held by the example's `token:fleet/forecaster` on a three-node fleet of published binaries (I0) with `gateway_identity_issuer = "fleet"`, refused without one, surviving a restart"* — this PR's run: the gateway and a member are the `mycelium` binary **built locally in release mode** with the release's `NODE_FEATURES` (not the published artifacts; CI runs debug builds of the same features), the gateway with `[authority]` (I0); refused without a grant, run under one, a grant issued before the gateway's graceful restart run after it. **A departure, stated:** the third node is `eve_fleet_node`, not the binary, because the binary hosts no tool of its own (I0's row says so) and Eve's tool is served by bridging the loader's MCP server; and the restart is the gateway's, graceful. *"a foreign-key `cs` refused"* — `the_provider_checks_the_client_signature_binding_signature_and_key_standing` (#742) and the fleet run (`client_key_unknown`, this PR). *"a replayed `cn` refused and a reordered honest pair admitted; two processes under one certificate not colliding"* — `a_replayed_cn_is_dropped_and_reordered_and_concurrent_honest_calls_are_admitted` (#742), in process; not on the fleet, where a replay of a granted call is refused first by the grant's use marker. *"the cross-language vector green in all three suites"* — the `cs` vector (#742) and the possession vector with its digest cases (#744). *"a superseded client key refused once the newer record is read, still refused after the record is overwritten and, under `[persistence]`, after a restart"* — the #741 tests above; on the fleet only the first of the three, the older key refused once the newer record is read. *"`revoke_grant` on a client holder evaluating `tool:{tool}@{provider}`"* — the two-node revoke test (#743) and the fleet run's revoked grant. *"a pre-upgrade `grant/node:…` record read unchanged"* — `a_grant_holder_segment_round_trips_and_is_unambiguous` and every grant test unchanged (#743). **Not built, stated (the row's own, unchanged):** OIDC (I5); the developer's keystore key (I7); M-of-N (beyond I3, stated there). *"an un-upgraded node's reading of `sys/client-cert/` (shown against the previous binary)"* — **shown** in #745's review round: the exit run with the v2.39.0 `mycelium` (the previous release, built locally from its tag) as the member node passed `CLIENT GRANT OK`, and the old node logged no warning about the records it carried. That shows it carries and relays them; what it does with them is nothing, since it has no fold. **Moved:** the refusal of an unsigned call → §9 O3; the mandate digest across languages → I3 (§9 O1, the maintainer's decision). **Found by the exit run:** §9 O2 — after the gateway's restart its first calls to the provider time out until the provider writes towards it; the run waits for a write each way. **Found by the A2A stream plant:** §9 O4 — a provider's refusal reached `/a2a` as a `completed` task (fixed in the review round below). **Review round 1 (#745, independent):** (1) the gateway's fold of client certificates leads the provider's, so a grant was spent on a provider's `client_key_unknown` — a provider's pre-execution refusal (`client_key_*`, `client_signature_*`, `stale`) now tombstones the gateway's `…/used/{door}` marker and releases its claim (`release_grant_claim_after`; the claim carried on the permit as `grant_claim`); the claim's readers enumerated (`grep -rn "claim_grant_use\|release_grant_use\|/used/" src`): the preflight's claim and R3's release, nothing else; sound against R2 since the marker guards work that may have run and none did, a crash before the tombstone leaving the grant spent; the provider's answer is trusted, stated; `a_grant_the_provider_refused_before_running_is_given_back`, seen failing first with the release doing nothing; the exit run's `sleep(2_000)`s are gone — it retries `client_key_unknown` under the same grant; (2) the door checks `cn.t` on the provider's window (`client_issue_time_fresh`, shared), `stale`; (3) the un-upgraded node shown (above); (4) O2 on the feature page; (5) O2 measured and rewritten (§9); (6) O4 fixed at both A2A doors, the journal recording `failed`; a tool's `-32022` refusal now carries `data.reason` (`a_providers_refusal_reaches_an_mcp_client_as_json_rpc_with_its_reason`); (7) a name holding `@` binds — the claim read up to its last `@` (`a_tool_whose_name_holds_an_at_sign_binds_its_signature`), the signed message's first-`@` cut stated on the page; (8) the exit driver asserts each refusal's reason; (9) CI's `eve-fleet` builds the release's `NODE_FEATURES`; (10) the closure's wording (local release builds; the fleet shows the first supersession refusal only); (11) a gateway with `tls` and no `[tls]` answers `client_signature_unverifiable`, and the door's `unsafe_integer` is tested (`the_door_refuses_a_stale_an_unsafe_and_an_unverifiable_signature_by_name`). Each fix seen failing first, one build per plant. **Confirm round (#745):** (1) **only the node authors a releasable refusal** — the paths by which tool- or skill-authored bytes reach a reply, enumerated: a native tool's answer (`Ok` wrapped in `result`, `Err` answered `-32603`: neither is the node's shape); a bridged MCP server's reply (forwarded verbatim — the hole: a tool that ran and answered `-32022`/`stale` had its grant given back) now rewritten into a tool error (`mcp::as_tool_answer`), `a_bridged_tool_answering_like_a_refusal_after_running_leaves_the_grant_spent`, seen failing first with the reply forwarded verbatim; the SDK serve stream refuses `mcp.invoke` and `skill.invoke` (protected kinds), so no SDK provider answers `/mcp`; skills answer raw output, and the A2A doors give nothing back; an embedder's own `rpc_rx("mcp.invoke")` is the node's code; and the release is taken off the embedder door (`gateway_record_execution`), whose kinds can be any provider's. `pre_execution_refusal` accepts the serve loop's `-32022` with the `caller context refused` message only; (2) the client signature binds the bare resource whole in all three languages — the first-`@` cut removed, the cs vector regenerated with an `at_case`, seen failing first in each suite with the cut restored; (3) the page scopes the release to `/mcp` and says A2A spends the grant; (4) a released grant stays counted against history limits, stated; (5) `provider_refusal` reads the node's two refusal bodies exactly (`only_the_nodes_refusal_bodies_read_as_refusals`, seen failing first with the loose reading); (6) O2's default-fleet window marked inferred. | this PR |
| I3 (PR 1 of 3) — §9 O1 | **The split, stated:** PR 1 (this) is §9 O1, the mandate/grant arguments digest across languages, which the maintainer placed in I3 and which shares no code with the signed decisions; **PR 2** is the signed decision routes (D3, D5) — a body signed by a certificate-holding principal, `not_addressed_here`, `not_the_decider`, `grant:relay` beside `grant:issue`, the journal naming the signer, the SDK verbs signing; **PR 3** is the private Cedar re-pin with a refusal case and I3's closure. **Enumerated** (`git grep -n "arguments_digest\|possession_request\|mandateArgumentsDigest\|mandate_request_bytes"` over `src`, the companions, both SDKs and the examples — §9 O1's reader list, confirmed): `gateway_authority::arguments_digest` (public; the request door in `http/grants.rs`, the agent's `await_grant` in `state_machine.rs`, three examples), the preflight's envelope digest (`http/authz.rs`), the provider's re-assessment (`provider_enforcement.rs`), `ExecutionAuthority::assess`'s binding and possession checks, the faults class (through `arguments_digest`), Python's `a2a.arguments_digest` (`prove_possession`, the Strands example) and TypeScript's `mandateArgumentsDigest` (`provePossession`); the companions compute none. *"the mandate/grant `arguments_digest` moves to RFC 8785 (JCS) in Rust, Python and TypeScript — every reader §9 O1 lists"*: `arguments_digest` is the client signature's `client_arguments_digest` in Rust; Python's `arguments_digest` is `client_arguments_digest` (`canonical_json`); TypeScript's `mandateArgumentsDigest` is `argumentsDigest` — the possession request, a grant's `bound:args:` at the request door and the agent's door, the preflight's evidence digest and both assessments now read it. *"a window for grants already issued … the row chooses and states which"*: **chosen: accept either for one release, and say which matched** — `ExecutionAuthority::assess_arguments` tries the RFC 8785 digest, then the pre-I3 `serde_json` digest (`legacy_arguments_digest`) where it differs, against the grant's binding and the possession proof; a legacy match is counted (`legacy_digest_matches`), the binding marked (`MandateBinding::legacy_digest`) and the evidence's `checked` names it (`LEGACY_DIGEST_CHECKED`); the release after refuses the older form (`deprecations.md` §93) — `a_grant_binds_the_jcs_digest_and_a_pre_i3_grant_is_honoured_for_one_release` (two grants, one per form, on keys the two forms order differently), `a_mandate_on_the_legacy_digest_is_named_on_its_record`; the provider's re-assessment does not count again (`reassess_arguments`). *"the large-integer rule (an integer outside ±(2^53 − 1) refused where the digest is taken)"*: any number of magnitude 2^53 or more, as I1's signature rule reads it, refused where a mandate or grant binds it — `MandateRefusal::UnsafeInteger` (new) at the gateway's and the provider's assessment, `400 unsafe_integer` at the request door (`the_request_door_refuses_an_unsafe_integer_and_accepts_an_exponent_form_float`), `GrantRefused` at the agent's door, the SDK digests raising; **a departure, stated:** a call presenting no mandate is not refused — its evidence records the RFC 8785 digest, as the policy may evaluate it without a binding. *Exit (O1):* *"an exponent-form float (`1e-5`, `1e21`) in a grant's arguments admitted from Python and TypeScript"* — `1e-5` and `1e-7`: the possession vector's `float_grant`, a grant bound to them with the holder's proof, made by the Python implementation and reproduced by jest and pytest, accepted by the gateway's `GrantVerifier`; **`1e21` is refused, not admitted, by decision** (the maintainer accepted the departure, 2026-11-09). The large-integer rule this row adopts counts it: a double of magnitude 2^53 or more is an integer, and JavaScript cannot tell `1e21` from its integer form. So the two halves of the exit cannot both hold, and the rule wins. **How this differs from plain RFC 8785:** RFC 8785 serialises any finite double, `1e21` included, writing it `1e+21`. This digest refuses such a number before serialising, wherever a mandate binds it. It binds only numbers every language reads back to the same value, so a large number travels as a string; *"the cross-language vector carrying such cases in all three suites"* — `digest_cases` (floats in exponent form, a key outside the BMP beside U+FF01, integers to 2^53 − 1), `unsafe_cases` and `float_grant`, checked by `the_possession_vector_is_the_gateways` (Rust), `possession.test.ts` and `test_possession.py`; the old `mandate_digest_cases` kept as `legacy_digest_cases`, reproduced by `legacy_arguments_digest`; *"a grant issued before the change honoured or refused as the window states"* — honoured, counted and named (above); *"an integer outside ±(2^53 − 1) refused by name"* — above. Seen failing first, one build or run per plant: listed in the PR. **Not built in this PR:** PR 2's and PR 3's items; the refusal of the older form, the release after this one (§9 **O5**). **Review round 1 (#750, independent):** (1) **number ties** — Rust's digits came from `{:e}`, which rounds a tie up, where ECMAScript and Python round to even: 1 220 of 299 896 sampled doubles differed (`1658206780088562.2` written `…562.3`), in the client signature's `cs` and the mandate digest alike, since both are `canonical_json`. The digits now come from `ryu` (already in the tree through `serde_json`, Apache-2.0 OR BSL-1.0; `ryu-js` is its fork for the layout, which `es_number` already writes). `tests/fixtures/cs/es_numbers.json` holds 606 doubles as Node writes them: 300 of the mismatches, a random sample and edges. `rfc_8785_writes_every_sampled_double_as_ecmascript_does` checks it in Rust, jest (through `argumentsDigest`) and pytest. A tie case is in both vectors. Seen failing first with `{:e}` restored: 302 of 606 differ, and the vector test fails too; (2) **the counter** — on `/stats` (`legacy_digest_matches`, `provider_legacy_digest_matches`) and `mycelium_legacy_digest_matches_total{point}`, in memory, the evidence `checked` named as the durable record; (6, the double count) — a node with provider enforcement assessed one call at the door and again at the provider into one counter, so it is now one counter per point; (3) **upgrade order** in §93 — verifiers before binders, and what an un-upgraded verifier refuses; (4) **standing requests** — the request door's and the agent's wait's reuse match either form (`recorded_digest_matches`), the wait verifies from the arguments (`reassess_arguments`), and a grant's binding and its proof may each be in either form (both are computed from the call's own arguments). Without that last change, a reused pre-I3 request's grant (bound to the legacy digest) could not be proved by an upgraded client. `a_request_asked_before_i3_is_reused_and_its_grant_runs_counted_on_stats`, `the_agents_wait_reuses_a_request_asked_before_i3`, seen failing first: plants listed in the PR; (5) **evidence without a mandate** — the precision loss and the cross-upgrade digest change stated in §93, the legacy form not recorded; (6) the doors' refusal names (`unsafe_integer` the request door's only) in §93, the feature page and both READMEs, and the X3 test's doc comment moved back above its test; (7) §9 O5. **Review round 2 (#750, at 0b7263e8; all seven confirmed):** (1) the fixture gained 1e21, the doubles below it (1e21·(1−2⁻⁵²) and the next down), 1e-6 (the literal `9.999999999999999e-7` is the same double), the double below it, and 1e-7. Checked in Rust and pytest; jest compares Node with Node; (2) the agent's wait counts a legacy match at its own point (`agent_legacy_digest_matches`, `CountAt::Agent`). A node-held grant goes to its provider directly, and a provider without provider enforcement never assessed it, so a legacy grant could run with every counter and `checked` at zero. Seen failing first with the counter in place and nothing incrementing it (`(0, 1, 0)`); §93 and O5's watch list name it; (3) §93 states the integer/float twin a pre-I3 grant admits. | this PR |
| I3 (PR 2 of 3) — D3, D5 | **Enumerated** (`git grep -n "gw_grant_decide\|gw_grant_revoke\|decide_grant\|revoke_grant\|grant:issue"` over `src`, both SDKs and the docs): the two decision routes (`http/grants.rs`), their scope rows (`required_scope` in `http/authz.rs`), the route-free `decide_grant` and `revoke_grant` (the `grant` fault class, `faults/grant.rs`), the SDK verbs `grant_decide` / `grantDecide` and `grant_revoke` / `grantRevoke`, `rbac.md`'s table and the feature page. *"the decision routes take a body signed by a certificate-holding principal; the gateway verifies it"* — `SignedDecision` over `grant_decision_message` (`src/agent/grant_decision.rs`), verified under the key it carries, that key the decider's **current** client key here (`client_key_standing`). *"checks the signer's `grant:issue`"* — a named token of this gateway's issuer whose principal the decider is (`decider_holds_grant_issue`), else `403 decider_lacks_scope`; the evaluator then decides with the decider as the actor. *"checks the addressed node is itself (`not_addressed_here`)"* — `409 not_addressed_here`. *"and records the signer"* — the evidence record's `subject` is the decider; the grant's `purpose` and the refusal and revocation records name the decider and `relayed_by`. *"the route admits `grant:issue` on the bearer **or** `grant:relay` … one-of-two for these routes"* — `alternative_scope`; the scope granted is the one that admitted the request, and a token holding neither is refused naming both (`or_scope`). *"SDK verbs sign"* — `signer=` / `{ signer }` on the decide and revoke verbs, the node read from `GET /health`; `ClientSigner.sign_grant_decision` / `signGrantDecision`; `tests/fixtures/decision/vector.json` checked in Rust, jest and pytest. *Exit:* *"`not_the_decider` on a foreign signature"* — a key that is not the named decider's, and a body whose terms differ from the signed ones; *"`not_addressed_here` on misdelivery"*; *"a relay with only `grant:relay` reaching the route and succeeding with the signer as actor, refused unsigned"* — `decision_unsigned`; *"a re-targeted body refused"* — `decision_retargeted`; *"the journal names the signer"* — every one in `a_signed_decision_relayed_under_grant_relay_is_the_signers`. **From the upgrade notes:** *"an unsigned decision body counted then refused"* — **counted** (`/stats` `grant_decisions_unsigned`); **refused: not built**, it waits a release with the counter visible (§9 **O6**). *"`grant:relay` inert at an un-upgraded gateway"* — an older gateway refuses the scope at the route; not shown against the previous binary. **Not built, stated (the row's):** M-of-N; an `oidc:` decider; a relay at an un-upgraded gateway. **Also not built:** a decider whose scopes come from OIDC groups or positional tokens; a nonce cache (a replayed decision finds the request answered or the grant revoked). Seen failing first, one build per plant: listed in the PR. **Review round 1 (#754, independent):** (1) **concurrent replays** — sixteen concurrent copies of one decision were each answered `200`, each a separately signed grant with its own evidence record; a decision now takes a claim on its request at this node (a papaya set on `TaskCtx`, no lock, no lock-order row) before it reads the request and holds it until the answer is written, so the rest answer `decision_in_flight` and then `already_answered` / `already_revoked`, signed or not (`concurrent_copies_of_one_decision_are_answered_once`); **the open question** — yes, before the claim a stale concurrent issue could write the first checkpoint after a revocation's second, and the KV keeps the later write; with the claim no route reaches that order, and an issue now refuses to write below a later checkpoint (`an_issue_never_lowers_a_later_checkpoint`); two gateways answering one request at once are not serialised, stated; (2) **the window** — 300 s plus `max_clock_drift_ms` into the past, `max_clock_drift_ms` into the future, stated in §95 and on the page, both edges tested (`a_signed_decision_is_fresh_inside_its_window_and_refused_stale_or_malformed_outside`); (3) **the counter** counts an answered decision only, with `mycelium_gateway_decisions_unsigned_total{decision}`, and O6's watch reads it; (4) **the evidence kept** — the evidence record's new `signed_decision` holds the body, the terms and `relayed_by`, the refusal and revocation records hold the body, and `SignedDecision::verify_signature` re-checks it (`a_signed_refusal_and_replays_one_at_a_time_are_answered_once_and_the_evidence_kept`); (5) `409 gateway_identity_issuer_unset` (`a_signed_decision_at_a_gateway_without_an_identity_issuer_is_refused_by_name`); (6) `21-mandates.md`, the page's route list and the CHANGELOG; (7) tests for `decision_signature_malformed`, `decision_unverifiable` (`a_node_without_an_identity_cannot_verify_a_signed_decision`), sequential replay, a superseded key (`a_decision_signed_with_a_superseded_key_is_not_the_deciders`) and a signed refusal. | this PR |
| I3 (PR 3 of 3) — **I3 closed on evidence, one promise moved** | **Closing I3, promise by promise, across #750, #754 and this PR.** *What:* *"the decision routes take a body signed by a certificate-holding principal; the gateway verifies it"* — `SignedDecision` over `grant_decision_message`, `grant_decision::verify`: the signature under its key, that key the decider's current client key here (`client_key_standing`) (#754). *"checks the signer's `grant:issue`"* — `decider_holds_grant_issue`, `403 decider_lacks_scope` (#754). *"checks the addressed node is itself (`not_addressed_here`)"* — `409 not_addressed_here` (#754). *"and records the signer"* — the evidence record's `subject` and `signed_decision` (the body, the terms, `relayed_by`), the answer records' `signed` and `relayed_by` (#754 and its review). *"the route admits `grant:issue` on the bearer **or** `grant:relay` … one-of-two"* — `authz::alternative_scope` (#754). *"SDK verbs sign"* — `signer=` / `{ signer }` on the decide and revoke verbs, `sign_grant_decision` / `signGrantDecision`, the decision vector in three suites (#754); their docstrings list every refusal the two routes answer, door and policy, `decision_in_flight` and `gateway_identity_issuer_unset` included (this PR and its review). *"O1 … RFC 8785 … a window … the large-integer rule"* — #750 and its two review rounds, row above. *"Depends on: I1"* — closed (#745). *Exit:* *"`not_the_decider` on a foreign signature; `not_addressed_here` on misdelivery; a relay with only `grant:relay` reaching the route and succeeding with the signer as actor, refused unsigned; a re-targeted body refused; the journal names the signer"* — `a_signed_decision_relayed_under_grant_relay_is_the_signers` (#754), with the edges the review added: concurrent copies answered once (`concurrent_copies_of_one_decision_are_answered_once`), the freshness window both ways, a malformed signature, an unverifying node, a superseded decider key, a signed refusal, replays one at a time, the evidence kept and re-verifiable (#754's review). *"the Cedar re-pin with a refusal case"* — **not built: moved to §9 O7.** The private adapter pins the substrate by **release tag** only (its `check-pin.py` refuses `rev`, `branch`, `path` and `[patch]` by name), and no tag carries I3: the newest is v2.39.0, which predates #750. The re-pin waits for the release that carries I3, and cutting that release is the maintainer's. *"for O1, …"* — #750's row. *Not built, stated (the row's):* M-of-N; an `oidc:` decider; a relay at an un-upgraded gateway — argued from v2.39.0's code in #754's review (the route requires the one scope; the old body ignores `signed`), not shown against a binary. *Upgrade notes (§5):* *"an unsigned decision body counted then refused"* — counted once answered (`grant_decisions_unsigned`, `mycelium_gateway_decisions_unsigned_total`); refused: §9 O6. *"`grant:relay` inert at an un-upgraded gateway"* — as above. **This PR, the residuals of #754's confirm round:** (a) the SDK decide and revoke docstrings name `decision_in_flight` and `gateway_identity_issuer_unset`; (b) the issue path's later-checkpoint guard records the refused decision's execution — `None` since #756's review (M1, L5): the guard now stands before the checkpoint is offered or written, so nothing ran — `an_issue_never_lowers_a_later_checkpoint`, tied to that decision's `operation_id`, seen failing first twice (no execution, then `Failed`); (c) the feature page says a re-verifier of `signed` compares the signed `request`, `agent` and `node` with the record it reads, since `verify_signature` checks the signature only. **Moved:** the Cedar re-pin → §9 O7. **Review round 1 (#756, independent):** (M1, L5) the guard moved above `offer_checkpoint`, recording `Execution::None` as the enum defines it and as the provider's refusals after a permit do; (M2) `a_relayed_signed_decision_the_policy_refuses_is_refused_with_the_decider_as_subject` — a decider with `grant:issue` and no rule, relayed, refused by policy and recorded `Decided` with the decider as subject — `Indeterminate`, not `Deny`: a unit's rules are allowances, so an actor no rule names has no authority established, which is never a permit, and the unit format has no prohibiting rule to yield a deny — the substrate half of O7's refusal case, seen failing first with the relay as the actor; (L1) every return after a permit records an execution — `None` where nothing ran or was written (`no_identity`, `unreadable_request`, the checkpoint's serialisation), `Failed` where the checkpoint was already on disk (the grant's serialisation) or the revocation already stood in memory (its checkpoint's serialisation) — `an_issue_the_node_cannot_sign_records_that_nothing_ran`, seen failing first; (L2) the docstrings list every refusal; (L3) the re-verifier compares the decider with the record's actor and passes the record's own decision and terms; (L4) this row's header marker and §6 item 5 name O7. | this PR |
| I5 (PR 1 of 2) — the provider's verifier | **The split, agreed 2026-11-11:** PR 1 (#759) is the provider's verifier and refusals; PR 2 is the gateway's exchange, its settings and start-time refusals, the DPoP proof, doctor's `id.oidc_exchange`, the Keycloak live suite and I5's closure. **Enumerated** (`git grep -n "MAX_ENVELOPE_BYTES\|ContextTooLarge\|frame_with_client_signature_at\|fn verify(\|fn verify_received\|OidcVerifier\|OidcConfig {"` over `src` and `mycelium-core/src`): the envelope and its one producer (`frame_with_client_signature_at`, now through `frame_envelope_at`) and its cap; the provider's `verify` and `verify_received`, every receive boundary's path to them; the gateway's `OidcVerifier` and its cache; `OidcConfig` and its six literals; `validate()`; `start()`. *"the provider validates it against the issuer's JWKS from its own `[oidc]` table"* — `exchanged_token::check`, the keys through `OidcVerifier::keys_now`, set at `start()` when `exchange_audience` is set. *"the verifier is `#[cfg(feature = "compliance")]` … refuses a call carrying `ot` at call time as `not_in_build`"* — `exchanged_token_at`'s other arm, `a_build_without_compliance_refuses_an_exchanged_token`. *"`aud` contains `exchange_audience`; `azp` equals `exchange_client_id` (`ot_wrong_client`)"*, *"`cnf.jkt` is present (`ot_unbound`), and equals the RFC 7638 thumbprint … (`ot_bound_to_another_key`)"*, *"`p` equals `oidc:{iss}/{sub}` (`ot_subject_mismatch`)"*, *"`exp`, `iat` and `nbf` … `ot_lifetime_too_long`"*, *"a token without `jti` is refused (`ot_without_jti`)"* — each case of `each_check_refuses_an_exchanged_token_by_name`. *"admitted once at this provider"* — `OtReplayWindows::admit` in `verify_received`, `an_honest_exchanged_token_is_verified_and_admitted_once`. *"At most 65 536 entries per gateway, and at most 16 384 per principal … refuses … (`ot_principal_window_full`, `ot_window_full`, both counted)"* — `the_exchanged_token_window_is_bounded_per_principal_and_per_gateway`, lock-order row 78. *"An `oidc:` principal's action needs an `ot`"*, with N4's kinds — `needs_exchanged_token`, `an_oidc_action_without_an_exchanged_token_is_refused`, `the_kinds_an_oidc_action_needs_an_exchanged_token_for`. *"Signed with the envelope … `so` … under the domain tag `mycelium:gateway-caller-ot:v1\n`"* — `ot_signed_message`, `sign_ot`, `the_gateways_signature_over_the_token_is_required`. *"The envelope cap stays at `MAX_ENVELOPE_BYTES`"* — `an_envelope_too_large_with_its_exchanged_token_is_refused_by_the_producer`. *"A provider with `exchange_audience` … needs `exchange_client_id` too"* — `validate()`, `the_exchange_settings_are_refused_by_name_when_incomplete`. *"A provider that does not verify behaves as today"* — `a_provider_without_an_exchange_audience_reads_no_exchanged_token`. **Decided and built here:** keys before the first fetch, `ot_keys_unavailable` (`a_token_arriving_before_any_key_is_held_is_refused_retriably`); `ot_wrong_audience`. **Not built in this PR:** everything the split gives PR 2, including the exit's OIDC user's call against Keycloak. Seen failing first, one build per plant: listed in the PR. **#759's review (2026-11-11), fixed here:** M1 the clocks at the receive boundary's one instant (`a_token_is_read_against_the_instant_its_window_prunes_at`; the gateway's bearer `exp` through the seam, `a_bearers_expiry_is_read_at_the_given_instant`); M2 one verifier per node (`a_node_that_is_gateway_and_provider_fetches_its_keys_once`); M3 `OidcConfig: Default` (`an_oidc_literal_with_default_is_the_minimal_table`), `sso.md` and `ROADMAP.md` corrected, §96 restated; L1 a warning at start and `oidc_exchange_needs_tls`; L2 and Q2 one window across gateways, an expiry queue, `ot_provider_window_full` (`the_window_is_one_across_gateways_and_frees_a_departed_ones`); L3 the order restated and pinned, the names corrected (`the_first_failing_check_names_the_refusal`); L4 no panic off a runtime (`a_verify_off_a_runtime_refuses_rather_than_panics`), lock-order row 17; L5 the RFC 8037 vector and the untested paths; Q1 `cap.invoke/` (the maintainer's decision); Q3 `typ` and `oidc_exchange_audience_is_gateway_audience`. **Its confirm round:** N1 the provider-wide ceiling and the cross-gateway denial stated, not scaled; N2 the keys fetched in `start()`, not `preflight()`, so doctor fetches nothing (`preflight_builds_the_verifier_and_fetches_nothing`); N3 the media-type prefix compared in any case, and the order pinned across header, keys, signature and issuer. | #759 |
| I5 (PR 2 of 2) — **I5 closed on evidence** | **The gateway's exchange, and I5 closed promise by promise across #757 (the spike), #759 (the provider) and this PR.** **Enumerated** (`git grep -n "gateway_rpc_call_with_mandate\|ResolvedPrincipal {\|frame_envelope_at\|OidcConfig {\|MATRIX_ROWS"` over `src` and `mycelium-core/src`, and — after #760's review found the second site — `grep -n "resolve_bearer(" src/agent/http`): the one dispatch path and its envelope producer, every `ResolvedPrincipal` literal (ten), the **two** places a bearer resolves to an `oidc:` principal (`gateway_auth` and `a2a_optional_auth`, `http/authz.rs`; `subject_token` set at both since the review's H1 — the PR as opened set it at the first only, so `/a2a` refused every OIDC skill call), `OidcConfig` and its `Default`, `start()`, the guarantee catalogue and doctor's matrix. *What:* *"RFC 8693 exchange at the IdP by the gateway, under its own client credential"* — `OidcExchanger::exchange` (`src/agent/oidc_exchange.rs`), the form checked field by field in `an_oidc_users_call_is_exchanged_at_the_gateway_and_verified_at_the_provider`; *"`exchange_token_endpoint`, `exchange_client_id`, the secret from `exchange_client_secret_env` (default `MYCELIUM_OIDC_EXCHANGE_SECRET`)"* — `OidcConfig`, `OidcExchanger::load`; *"a DPoP proof signed with its Ed25519 node identity key (`alg: EdDSA`)"* — `dpop_proof`, `the_dpop_proof_is_an_eddsa_jws_under_the_node_key`, and the mock IdP binding to the proof's key; *"sender-bound … and verified at the provider"* — #759's `exchanged_token::check`. *"The node key signs a JWS … three shapes"* — #757's note and its confirm rounds (the argument, not code). *"Clients do not change. A gateway-audience token is never forwarded"* — no SDK file changes in this PR; the user's token is the exchange's `subject_token` only. *"The exchanged assertion travels in `ot`, once per call"* — `exchanged_token_for` → `frame_envelope_at`. *"The trigger …"* — restated by the maintainer on 2026-11-11 (#760's review, M1): **exchanges iff `exchange_token_endpoint`, verifies iff `exchange_audience`** — `the_exchange_settings_are_refused_by_name_at_start` (`unchanged`, `verifies_only`, a node without an HTTP port exchanging), `a_node_verifies_without_exchanging_when_no_endpoint_is_set` (the two guarantees apart). **The maintainer's decisions, each recorded on the row (2026-11-11):** the gateway's own client credential, `exchange_audience` read at both ends, the 8 KiB cap (`an_envelope_too_large_with_its_exchanged_token_is_refused_by_the_producer`), sender binding by DPoP (#757), keys at `start()` and `ot_keys_unavailable` (`a_token_arriving_before_any_key_is_held_is_refused_retriably`, `preflight_builds_the_verifier_and_fetches_nothing`), `ot_wrong_audience` for a gateway-audience token, **exchange only for an action** (`only_an_oidc_users_action_is_exchanged_for`), and `cap.invoke/` an action (`the_kinds_an_oidc_action_needs_an_exchanged_token_for`). *"`doctor` reports `id.oidc_exchange`"* — `guarantee.rs`, `MATRIX_ROWS`, the guarantee catalogue regenerated. **Exit:** *"an OIDC user's call verified at the provider"* — against Keycloak 26.8.2 pinned by digest (`sha256:723b1d6e…`), `oidc_exchange_live_an_oidc_users_call_is_verified_at_the_provider` in CI's `oidc-exchange-live` job under `MYCELIUM_OIDC_EXCHANGE_LIVE_REQUIRED`; **run locally on 2026-11-11** (passed: two `/mcp` calls, each exchanged and admitted once at the provider; seen failing first with the exchange switched off — `ot_required`); in process against a mock IdP, `an_oidc_users_call_is_exchanged_at_the_gateway_and_verified_at_the_provider`. *"the same exchanged token presented twice refused, by its `jti`"* — `an_honest_exchanged_token_is_verified_and_admitted_once`, `the_window_is_one_across_gateways_and_frees_a_departed_ones`. *"a token lifted into another node's envelope refused `ot_bound_to_another_key`, and a token without `cnf` refused `ot_unbound`"*, *"a token whose subject is not the envelope's principal refused"*, *"a token whose `azp` is not the gateway's client refused"*, *"a gateway-audience token refused by name"*, *"a member's forged `ot` refused by the JWKS"* — the cases of `each_check_refuses_an_exchanged_token_by_name`, ordered by `the_first_failing_check_names_the_refusal`. *"a provider without `compliance` refusing `ot` as `not_in_build`"* — `a_build_without_compliance_refuses_an_exchanged_token`. *"an oversized envelope refused `caller_context_too_large`"* — above. *"the exchange's settings refused by name at start when unset, and an existing `[oidc]` gateway without `exchange_audience` unchanged"* — `the_exchange_settings_are_refused_by_name_at_start` (endpoint, client, secret unset and empty, the egress denial; a provider-only node and an unchanged gateway start). *"an `oidc:` action without an `ot` refused at a verifying provider"* — `an_oidc_action_without_an_exchanged_token_is_refused`. **Built beyond the row, from this PR:** an exchange that yields no token refuses the call and runs nothing (`oidc_exchange_unreachable`, `oidc_exchange_refused`, `oidc_exchange_too_large`, `oidc_exchange_malformed`, `oidc_exchange_no_subject_token`, `oidc_exchange_no_identity`; `an_exchange_that_yields_no_token_is_refused_by_name`, and the refused case above), JSON-RPC `-32024`, HTTP 502. **#760's review, fixed here:** H1 `/a2a` send and stream exchange (`an_oidc_users_a2a_call_is_exchanged_on_send_and_stream`); M1 above; M2 a grant's use given back on the gateway's own refusals before sending, at `/mcp` and — after its confirm round — at both `/a2a` doors (`a_grant_is_given_back_when_the_gateway_refused_before_sending`, `a2a_a_grant_is_given_back_when_the_gateway_refused_before_sending`; an `oidc:` principal holds no grant today, possession needing a key it lacks, so the A2A test refuses before sending by `provider_without_caller_context`, the same arm); M3 no redirect followed, L1 the answer bounded at 64 KiB and the JWKS and discovery at 1 MiB, L3 one `use_dpop_nonce` challenge answered (`the_exchange_follows_no_redirect_bounds_its_answer_and_answers_one_nonce_challenge`, `a_jwks_past_its_bound_is_a_failed_fetch`); L2 `oidc_exchange_endpoint_insecure`; L3 `htu` without query and fragment, the Basic credential form-urlencoded (`the_dpop_htu_and_the_client_credential_are_encoded_as_the_rfcs_say`); L4 and L5 stated in §97; L6 legacy not demanded, `id.oidc_exchange` not applicable; L7 the live step requires exactly one test passed; L8 stated: **the exit is one node**, the gateway its own provider — the two-node hop is the caller envelope's, tested apart, and a two-node run against Keycloak is not shown. **Not built, stated (the row's list, unchanged):** an IdP without exchange; a decision signed by an assertion; a token bound to its call; the exchanging gateway reusing its own token until `exp` (refused once per provider); the gateway as attacker; the mTLS route; an IdP other than Keycloak, and Entra's `uti`; the provider deriving scopes from the token; and from #759's confirm round, the provider-wide window's cross-gateway denial (rate limits are the bound). Seen failing first, each refusal: listed in the PR. | this PR |

## 9 · Open items, tracked

Items found while building a row that the row does not own. Each names its home, what it changes, and why it waits.

- **O1 · The mandate/grant `arguments_digest` across languages** *(delivered in I3's PR 1; placed in I3 by the
  maintainer, 2026-11-09; found by #742's review, 2026-11-08)*. The digest a
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
    provider noticing the dead connection on its health tick (1 s on `eve_fleet_node`). The default
    `health_check_interval_secs` is 10 s, so a default fleet's window is inferred to be longer; not measured.
  - **Not the writer's idle bound** (the review's hypothesis): the provider's `writer_idle_timeout_secs = 5` changed
    nothing (two runs each way). Its health pings keep that writer busy, so the bound never fires.
  - **Consistent with:** the writer's own `Write to … failed; 1 frame(s) will be dropped during backoff`. A frame
    written into the connection to the peer's previous process is dropped, and an RPC reply is not resent.
  - **Reach:** this is the transport, not I1. Every reply a node sends to a peer that restarted, before it notices,
    is lost. The caller sees a timeout, which the substrate reads as delivery unknown. Whether the tool ran is not
    established here.
  - **Cost under a grant:** a timed-out call has spent the grant's use at the gateway, because a timeout is not one of
    the refusals that give it back.
  - **Home:** [issue #746](https://github.com/RichardTathata/mycelium/issues/746), filed by the maintainer, the
    transport's. Until it is fixed, `run_client_grant.mjs` probes a write each way before calling; the probe from the
    provider surfaces the dead connection. The probes narrow the window and do not close it: in the twelve release runs of
    2026-11-09, five first calls after the restart timed out after them (idle and loaded alike). The driver therefore
    holds a second grant issued before the restart, presented when the first call times out, and counts each such
    timeout (`o2_timeouts`).
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
- **O5 · Refusing the pre-I3 arguments digest** *(open; I3's window, `deprecations.md` §93; from #750's review)*. For
  one release a mandate bound to, or proved over, the older `serde_json` digest is honoured, and a standing request
  recorded with it is reused. Each match is counted, in memory, on `GET /stats`: `legacy_digest_matches`,
  `provider_legacy_digest_matches` and `agent_legacy_digest_matches`. A match at a gateway door or a provider is also
  named on its evidence record's `checked`. The release after
  removes the legacy form:
  - `legacy_arguments_digest`'s use in `digest_forms` and `recorded_digest_matches`;
  - `DigestForm::Legacy`;
  - the counters.

  A mandate on the older form is then refused `BoundToOtherArguments` or as a possession mismatch.
  **Home:** the release after the one that carries I3's PR 1, as its own row. The maintainer schedules it. It outlives
  I3's closure. **Watch before it lands:** all three counters (gateway, provider and agent) at zero on every node,
  and no `checked` match in the evidence journals. The counters reset on restart; the agent's wait writes no evidence
  record, so read its counter over a period longer than a grant's window, with no restart in it.
- **O6 · Refusing an unsigned grant decision** *(open; I3's "an unsigned decision body counted then refused",
  `deprecations.md` §95)*. Under `grant:issue`, a decision without `signed` is still decided as the bearer's own,
  and counted once answered on `GET /stats` as `grant_decisions_unsigned` and as
  `mycelium_gateway_decisions_unsigned_total{decision}`. The refusal waits a release in which the counter is
  visible, as O3's does, so an operator sees whose decisions would be refused, and gives those supervisors a client
  certificate, before anyone is refused. **Home:** the release after the one that carries I3's PR 2, as its own row. The
  maintainer schedules it. **Watch before it lands:** `mycelium_gateway_decisions_unsigned_total` not rising on any
  gateway — or `grant_decisions_unsigned` flat over a period with no restart, since that counter is in memory — while
  supervisors are answering requests (an answered decision is what it counts, so silence without traffic proves
  nothing).
- **O7 · The private Cedar adapter's re-pin on I3** *(delivered 2026-11-12, private #34, merge `7491e61`, head
  `56b7798d`; I3's exit "the Cedar re-pin with a refusal case", moved when I3 closed)*. The private adapter pins the
  substrate by release tag only; v2.40.0 is the first tag carrying I3. Its three promises:
  - *"moves the pin"* — both `mycelium` and `mycelium-core` at `tag = "v2.40.0"`, the lockfile resolving to
    `d974fc76`, `check-pin.py` passing.
  - *"runs the adapter's `ae_contract` suite at it"* — `the_shipped_cedar_adapter_meets_the_contract`: every case,
    two refused at load (the public suite's `accepts_refusal_at_load` cases, `fleet_limit_past_the_seat_cap` and
    `fleet_window_past_the_horizon`, whose bounds the adapter shares with the substrate), none declined. The
    agreement test with the reference evaluator accepts a refusal at load on exactly those cases, asserting the
    adapter's `BuildError::FleetCounter`.
  - *"adds a refusal case: a signed grant decision whose decider the Cedar policy does not permit, refused with the
    decider, not the relay, as the actor"* — `o7_a_signed_decision_the_policy_does_not_permit_is_refused_as_the_deciders`,
    a real node with the Cedar adapter through the grant decision route: a decision signed by a decider the policy
    does not name, carried by a `grant:relay`-only bearer, answers `403 authority_not_established`; its evidence
    record's subject is the decider and its `relayed_by` the relay; the permitted decider's relayed decision is
    admitted under its own name, and no record names the relay. The policy also permits the relay, so a substrate
    keyed on the relay fails three ways. **Seen failing first** by editing the pinned substrate checkout so the
    route's actor was the relay — the adapter never sees the relay, so it cannot be planted there.

  Found on the way: the private lockfile had carried `rustls-webpki` 0.103.15 (RUSTSEC-2026-0382) since the v2.36.0
  re-pin and `rustix` 1.1.4 (RUSTSEC-2026-0486); both bumped to the public v2.40.0 lockfile's versions. The private
  repository's scheduled audit had failed on every run since 2026-09-27 (the action could not open its issue, and later runs failed on a Search validation error), so
  the advisory, reported daily from 2026-10-21, read as that permanent red. Replaced in #34 by `cargo audit --deny
  warnings` over every tracked lockfile, as this repository's `audit` job runs it. The public release was not
  affected.