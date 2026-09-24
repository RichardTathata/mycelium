# dev/security — the v1.x security surface (WS1–WS5) + crown-jewel posture

↑ [dev/](dev.md) · canon: `docs/threat-model.md` · runbooks: `docs/operations/{rbac,sso,audit,cert-rotation,crown-jewel}.md`

Everything here obeys the **detection-not-prevention / promise-strength** posture
([runtime-invariants](architecture/runtime-invariants.md)): enforcement happens where a
resource is *served*, never by teaching Layer I a higher-layer law. All shipped
(v1.x engineering complete); plan record: `docs/plans/v1x-completion.md`.

## WS1 — RBAC / identity (`compliance` feature)

Four layers, all additive/opt-in (`src/agent/rbac.rs`, gateway middleware in
`src/agent/http.rs`):
1. **Signed role claims:** `advertise_roles` writes an Ed25519 `SignedRoleClaim` to
   `sys/role/{node}`; `roles_of` returns it only if the signature verifies against the
   node's identity key learned from the cluster — a forged KV write reads back `None`.
2. **Provider-side capability authz:** `caller_authorized` enforces
   `authorized_callers` at the served path (the one place it's genuinely enforceable).
3. **OAuth2 scope gateway ACLs:** `gateway_scoped_tokens` maps bearer→`resource:verb`
   scopes; deny-by-default (unmapped route ⇒ `admin`). `/health|/ready|/stats|/metrics`
   stay public (M16 edge criterion). **Merged application routers** (`with_http_routes` —
   every companion's gateway surface, A2A) get the same bearer-then-scope layer on their
   `/gateway/…` paths via a prefix-guarded `route_layer` (`gateway_auth_if_gateway_path`);
   their paths outside `/gateway/` stay public by construction. *Fixed 2026-09-04:* the
   layer wrapped only the library's nested router, so `/gateway/reason/route`,
   `/gateway/wiki/ingest`, `/gateway/tuple/put` … answered without a bearer while the
   companion docs claimed coverage — gates in core and `mycelium-reason`, ledger entry
   in `docs/analysis/ratings.md`. Companion paths have **scope families** in
   `required_scope` (`llm:*` for reason, `wiki:*`, `board:*`, `tuple:*`), exact paths only;
   unlisted companion paths stay `admin`. *Fixed 2026-09-05 (external review, finding 4):*
   the **node-level** `/mcp`, `/signals/{kind}`, `/consensus/{slot}` were public by routing
   comment while this page and `rbac.md` listed only the four probes — `POST /mcp` `tools/call`
   invoked any cluster tool **with the node's identity** (confused deputy: provider-side
   `authorized_callers` sees the node, not the HTTP caller). Now behind the same `gateway_auth`
   layer with scopes `mcp:invoke` / `mesh:read` / `consensus:read`. `GET /bulk/{id}` stays
   public **by design** — a capability URL (64-bit random per-call nonce, fetched peer-to-peer
   with no shared bearer). The public surface is exactly: `/health|/ready|/stats|/metrics`,
   `/bulk/{id}`, the A2A descriptor and `POST /a2a` (an optional bearer on it is resolved to the
   caller principal, an unrecognised one is 401 — item 5 below). Gates:
   `regression_node_level_routes_require_bearer_when_token_set`,
   `node_level_routes_honour_scoped_tokens` (`src/agent/http.rs`).
4. **`sys/` namespace tripwire (core, feature-free):** inbound writes naming *self* under
   `sys/identity|load|role|tuple|caller-context/{node}` → `warn!` + `sys_namespace_violations`.
   Detection only — never make it a write guard.
5. **Gateway caller identity (v3 item 7, 2026-09-13; `src/agent/gateway_caller.rs`):** the
   confused deputy the 2026-09-05 fix left open — every gateway dispatch (`tools/call`, `/a2a`,
   `rpc/call`, `scatter`, `emit_reliable`, `llm/*`) ran as the **node**, so layer 2's
   `authorized_callers` saw the gateway and never the client. Now the auth layer constructs a
   `GatewayCaller` (principal `oidc:{sub}` / `token:#{i}` / `token:legacy` / `anonymous` · `via`
   = this node · `scopes` = credential ∩ route, never `*`) and the node attests it (Ed25519 over
   `principal ‖ via ‖ scopes ‖ issued_at ‖ sha256(payload)` under `tls`); it rides inside the
   RPC payload after the nonce (**wire v12 unchanged**), `RpcRequest::payload()` strips it, and
   the provider verifies `via == sender` + the signer against the keys it knows for `via`.
   Providers authorise with **`request_authorized`** (a client by *principal* — listing the
   gateway node admits nothing; a node by id/roles as before); guardrails `check_caller` /
   `guarded_rpc_serve` and SkillRunner use it, and denial seals name the principal + `via`.
   **Secure profile** (`gateway_caller_profile`, default) refuses rather than impersonates:
   forged context (ignored at the gateway / `CallerError` at the provider), missing context
   (`-32020`), over-scoped assertion (intersection by construction), and a provider without
   the `sys/caller-context/{node}` marker (`-32021` / HTTP 412, naming it); `legacy` =
   node-as-caller for a rolling-upgrade window, `warn!` at start, plan §6.6 removal-ledger entry.
   Gates: `gateway_caller_tests` (`src/agent/http.rs` — the four negative cases, the
   `authorized_callers` gate under `tls`+`compliance` with a mesh forgery, `/a2a` principal
   resolution) + `agent::gateway_caller::tests` (framing, sign/verify, tamper, unknown signer).
   **Review-hardened before merge (2026-09-14, four findings):** a secure-profile gateway node
   publishes marker `"2"` and wraps its own RPCs in a signed `node:{self}` envelope, so a raw
   `/gateway/signal/emit` of RPC-shaped bytes is `Missing`, never the node; principals are
   issuer-qualified (`token:{issuer}/…`, `oidc:{idp}/{sub}`; `gateway_identity_issuer`,
   `gateway_named_tokens`); `rpc_rx` verifies at the receive boundary for every loop in every crate,
   and the LLM / MCP / explain receivers verify directly; framing is three-way and malformed refuses,
   with a producer bound. Strength: *HardPrevention* at a `tls` provider; *SelfImposedPrevention* on an unauthenticated
   mesh (`CallerAttestation::UnauthenticatedMesh`). Runbook: `docs/operations/rbac.md` §7; log
   [`.log/2026-09-13-item7-gateway-caller-identity.md`](.log/2026-09-13-item7-gateway-caller-identity.md).

**WS4 OIDC SSO** (`src/agent/oidc.rs`): JWT validated against IdP JWKS, groups→scopes into
the same gate. Alg-confusion-safe (asymmetric-only allowlist *before* key selection);
iss/aud/exp checked; JWKS cached with refresh-on-unknown-kid. Human-operator auth, not agent
identity.

## The identity-proof window, and a diagnosis that was wrong (2026-09-24)

`require_identity_proofs` defaults to **`false`**. Set it and an identity entry this node cannot
authenticate is rejected rather than accepted-and-flagged. It was flipped to `true` on 2026-09-23
and reverted the next day. **Read the correction first, because the original write-up of this
section asserted a cause it had not checked.**

### The correction

The revert was triggered by an intermittent `S12 leader election … Nodes disagree on leader` in the
overlay Docker suite, after twelve consecutive greens. That failure was attributed to the flip. **It
cannot have been caused by it.** Every identity writer and both readers (`prewarm_peer_keys`,
`start_identity_watcher`) live inside the `if let Some(ref tls_cfg) = self.config.tls` block in
`src/agent/lifecycle.rs`; the overlay suite's nodes are the demo binary, which configures no TLS,
and the compose file sets no TLS env. With no TLS there are no `sys/identity/` entries, the
validator never runs, and **the flag is inert**.

So: S12's intermittency is **unexplained and still open**. It is not the flip, and it is not #369's
election-rule change either — that landed after the failing commit.

The mistake is worth naming because it is the same one the [testing
page](testing/testing.md) warns about, run backwards: a red run was taken as evidence about a
*cause*, on the strength of "it was the only change in that commit", without checking whether the
mechanism could reach the test. One correlation, six documents.

### What is real

Identity and proof *were* two separate `kv_set` calls — two gossip messages with no ordering
between them. In a **TLS** fleet a peer could therefore learn an identity before its proof, reject
it, and hold no key for that node until the proof arrived. The key recovers by itself
(`start_identity_watcher` subscribes to the broader `sys/identity` prefix, not `sys/identity/`,
precisely so a late proof re-validates its entry), so the window is transient — but a one-shot
decision taken inside it would not be.

That is a **hazard, not an observed failure**, and the distinction is the whole point of this
section. It justifies the fix on its own; it does not justify a story about a cluster splitting.

### The fix: Phase 3b's sealed record

`sys/identity-signed/{node}` carries `version(1) ‖ history ‖ proof(96)` — the same key history and
the same proof, in **one** KV entry. One entry cannot arrive in two parts, so the hazard is removed
**by construction rather than by timing**, which is the only kind of removal worth having. Every TLS
node writes it *and* the legacy pair, so a node predating the release still learns keys; readers
prefer it; and with `require_identity_proofs` set, readers accept **only** it — the pair is refused
not because it is invalid but because it is two messages.

Rotation writes the sealed record too. It has to: readers *prefer* it, so a rotation that updated
only the pair would leave every proof-requiring peer reading the pre-rotation history and never
learning the new key. Any time a reader gains a preference, every writer of the old thing is a bug
until it writes the new one.

### What is pinned

The default, with its reason, in `config::tests::the_default_requires_identity_proofs`. That a
sealed record authenticates on its own and the legacy pair does not, in
`lib_tests::identity_proof_default` — `the_legacy_pair_is_refused_when_proofs_are_required` is the
canary: if it ever stops holding, the hazard is back. And end to end,
`test_identity_proofs_required_two_nodes_still_authenticate`, which states in its own doc that it
**cannot** measure a cross-process race and proves only that the sealed path is wired and
sufficient.

### The limit none of this closes

*Proofs required* is **not** *identity authenticated*. First sighting of a node never seen is still
**trust on first use**: a self-signed entry is accepted because there is nothing established to
chain it to, so an admitted-but-hostile member can still introduce a key for a node nobody has met.
Anchors — a direct, CA-validated connection (Phase 1b) — are what close that, after which an
unchained key is rejected *and counted* in `identity_anchor_conflicts`. Proofs close the
unsigned-mimic residual; anchors close the first-sighting one; complementary, not alternatives.
`requiring_proofs_does_not_close_trust_on_first_use` is written to **fail if the TOFU window is ever
closed**, so the claim cannot rot.

**Who this matters to beyond the mesh:** `mycelium-commitment`'s offer/award signatures verify
against keys a caller resolves, and `sys/identity/{node}` is the obvious source for a node
participant. That chain is as strong as the caller's key resolution, which on the default
configuration is TOFU.

### What remains

A rollout, not a defect: a node accepts what its peers publish, and a peer on an older release
publishes only the pair. Turn the flag on once every node writes a sealed record
([cert-rotation](../../operations/cert-rotation.md) has the check); flip the *default* a release
later, deliberately.

## Threat model revision 2 (v3 item 8, 2026-09-13)

`docs/threat-model.md` §5 adds the four boundaries the contracts axis introduces — **D** a foreign principal across a
domain edge (item 2: transports never joined, three trust relationships apart, bilateral bundles, no leader, SWIM off in
the enforced profile), **E** an authenticated-but-abusive client (item 7 shipped: the attested `GatewayCaller`; AE0's
`indeterminate ≠ permit`; the gateway is a route-level preflight), **F** evidence confidentiality and the
hash-as-credential (item 3: opaque addresses, heads in KV and bodies in an authorised store, equivocation preserved,
evidence never grants what authorisation denies), **G** a compromised former holder and a forged epoch (item 5: the
decisive epoch invariant enforced inside the store's atomic boundary, `MandateSuperseded`, three lifecycle events
apart) — plus the shared rule *authority is recomputed, never inherited*. §6 fixes what identity, evidence and replay
artefacts may carry: verified claims and scoped attestations, never credentials; replay bundles redacted at the
recording seam with a named protected-artefact class. Items 2, 3 and 5 cite it from their PR 1 ADRs (plan §6.5, done).
Log: [`.log/2026-09-13-threat-model-rev2.md`](.log/2026-09-13-threat-model-rev2.md).

## Threat model revision 3 draft — Boundary H (2026-09-23)

`docs/threat-model.md` §5 adds **H**, a colluding population of admitted members: the plural of §4's "trusted member
acting maliciously within its authorization", which revisions 1 and 2 model only one member at a time. Its gains
are each verified in code:
- any member may advertise any capability name, so a stolen power can be offered as a service;
- a single peer's `Supports` accepts a release under the default `ReaderPolicy`, and `min_supporting` counts records;
- one challenger of any group jams a verdict to `Rejected` or `Conflicted`;
- a key holder can reissue its own audit suffix undetected, and the chain cannot show acts never recorded.

Mitigations in force are attribution, observation by membership, reader-configured independence, mandate expiry
(which stops new admissions; admitted work needs plan A1), and domain containment. The proposals are sequenced in
`docs/plans/boundary-h.md` (rev 0.2, revised after an external design review) and are **not adopted**. The residual
is stated bluntly:
collusion and cooperation are the same behaviour, acts off the substrate are invisible, and there is no central halt
by design. Motivated by the 2026 OpenAI–Hugging Face agent incident.
Log: [`.log/2026-09-23-threat-model-boundary-h.md`](.log/2026-09-23-threat-model-boundary-h.md).

## WS2 — tamper-evident audit (`compliance`)

Per-node hash-chained signed records at `sys/audit/{node}/{seq:016x}` (a global chain would
need a sequencer = coordinator). `SignedAuditRecord` = Ed25519 over canonical bytes;
`verify_chain` returns a precise error naming the offending seq. Sealing holds lock #8 only
for seq/hash/head (~µs); signing and the KV write happen after release. `GET /gateway/audit`
(scope `audit:read`). Records are plain KV — tampering fails verification, is never blocked.

## WS3 — crown-jewel posture (feature-free)

Threat frame: the twin/fleet-state is the concentrated SPOF map; the question regulated
buyers ask is *blast radius*, not "is it secure" (brokered competitors can't answer it —
their broker IS the crown jewel). Two opt-in controls:
- **`DataAtRestCipher`** hook (`src/persistence.rs`) at the four on-disk boundaries (WAL
  append/replay, snapshot write/read). Key custody is the operator's (wrap a KMS); scope is
  disk only.
- **`EgressPolicy { allow_hosts }`** — enforced at every outbound HTTP path the substrate
  chooses (MCP bridge, capability probes, LLM backends, SkillRunner). Fail-closed on
  unparseable hosts.
- **Crypto-shred erasure (WS-F, `tls`)** — `SubjectKeyRegistry` (`mycelium-core/src/erasure.rs`):
  per-subject DEK envelope encryption; GDPR erase = destroy the DEK → all ciphertext dead. The
  per-subject layer *above* the KV value, composing with `DataAtRestCipher`. Physical deletion isn't
  guaranteeable in a gossip+WAL mesh, so key-destruction is the mechanism; production custody is a
  KMS. Design: [`design/data-lifecycle-and-erasure.md`](../../design/data-lifecycle-and-erasure.md);
  runbook [`operations/data-erasure.md`](../../operations/data-erasure.md).

## WS5 — hot cert/identity rotation (`tls`)

`NodeTls` contents live behind `ArcSwap` (read via accessors per connection — never cache a
config past a rotation; no listener drain-swap needed). `rotate_identity`: generate →
publish `sys/identity/{self}` = `new‖old` → wait → activate.
**Retained-key verification (option B):** `peer_keys` accumulates a per-node key set
(union via `merge_peer_keys` — see [concurrency](concurrency/lock-free-and-atomics.md));
every verify path tries the set. Caveat: a retired key still verifies — compromise needs
explicit **revocation** (WS-D shipped the CT-style revocation log + `/gateway/transparency`
inclusion proofs, PRs #77–#82; revocation is now also applied on the consensus verify path,
audit 2026-07-15 pass 3).

> **`sys/identity` authentication — partially closed (WS-E, in progress).** `sys/identity/{node}`
> is a plain Layer-I KV value with **no signature**, and `merge_peer_keys` **accumulates** any key
> that appears there — so historically a compromised or buggy admitted node could LWW-poison a
> peer's verifying-key set, defeating the pass-2 `signer_authorized` bind (a **Byzantine-insider**
> vector, formally outside CFT-not-BFT, closed as defense-in-depth).
> - **Phase 1a (shipped):** `tls::ed25519_key_from_cert_der` extracts a peer's key from its
>   CA-validated cert.
> - **Phase 1b (shipped 2026-07-22):** the outbound writer **harvests** each directly-connected
>   peer's CA-validated key into `peer_anchor_keys` (on `CoreCtx`) + `peer_keys`, and a
>   `sys/identity` KV key differing from the anchor trips `identity_anchor_conflicts` (`/stats`).
>   Detection + an authenticated anchor for connected peers; KV overwrites are **not yet rejected**.
> - **Phase 2 (shipped 2026-07-22):** signed `sys/identity-proof/{V}` = `signer_key‖sig` over the
>   identity history; a node signs its own entry on publish/rotation (rotation signs with the
>   *prior* key so peers chain trust). On merge (`helpers::validate_and_merge_identity`), a key
>   enters `peer_keys` only if the proof is signed by a key already trusted for V (anchor/prior) or,
>   for an unknown V, TOFU-accepts a self-signed first entry. A proof signed by an **untrusted** key
>   is **rejected** — the poisoning vector is closed for any connected/established peer. No proof ⇒
>   rollout tolerance (Phase 3 tightens).
> - **Phase 3 (shipped 2026-07-22):** config flag `require_identity_proofs` (default off) — when set,
>   an identity entry *without* a valid proof is rejected outright, closing the "mimic a pre-upgrade
>   node" residual. Gated by a **config flag, not a wire bump** — Phase 3 changes no frame format
>   (proofs gossip as ordinary Data frames), so a `WIRE_VERSION` bump would gate nothing; the
>   two-release rollout discipline (enable only after the fleet fully writes proofs) is documented
>   like a `PREV_WIRE_VERSION` window in [cert-rotation](../../operations/cert-rotation.md).
>
> Full design: [`docs/design/identity-authentication.md`](../../design/identity-authentication.md).

## Federated domains — the trust boundary (v3 item 2, 2026-09-17)

Record `docs/design/federated-domains.md`; code `src/federation.rs` + `src/federation/`. **A domain is one
independently admitted gossip mesh.** Federation connects *exported services* across domains and **never joins
transports** — the two meshes stay two meshes, which is the whole security claim. Three trust relationships kept
apart (record §3): admission into a mesh · trust in a partner domain's identity · authorisation of one call.

The decisions that carry the boundary, each a checked thing rather than a sentence:

- **D5 — the invocation edge *is* A2A.** No second protocol; `FederatedCaller` / `verify_federated_call`
  (`federation/call.rs`) sign and verify the existing call shape. **On the wire (PR 8, 2026-09-18):** the
  credential rides on an ordinary `tasks/send` to `/a2a` in the `x-mycelium-federation-call` header
  (`federation/edge.rs`, `PresentedCall`). The gateway **authenticates at the auth layer and authorises in the
  handler** — `FederationEdge::authenticate` (signature, lifetime, expiry, skew; binds nothing) before the body
  is read, `FederationEdge::authorize` (the export binding and the policy grant) once the body has named the
  skill. The provider is told `federation:{origin}/{principal}`, never the gateway. Two rules the HTTP layer adds
  (`src/agent/federation_http.rs`): a presented credential that fails is **refused, never anonymised** (a revoked
  partner must not become an anonymous caller), and a bearer plus a credential on one request is a 400.
  Discovery is `GET /federation/catalog` under a credential for the reserved export `federation.catalog`, and
  the reply is the *filtered* list — the grant, not the export list.
- **D6 — reuse the cryptography, never the trust.** OIDC's primitives are borrowed; its issuers are not trusted.
  A `TrustBundle` (`federation.rs`) **decides which key** may sign a partner's descriptor or policy, so a
  self-signed descriptor is not authorised by being internally consistent. Rotation and revocation are first-class.
- **D7 — no `federation/` KV prefix.** Foreign state never enters the gossip medium. `scripts/check-kv-namespaces.sh`
  (`make check`, CI) fails on a forbidden prefix literal in production code — the plan had claimed this sweep
  existed; it was built at PR 1 when it turned out not to.
- **D25 — the NANDA boundary.** AgentFacts publication is the *edge*, not the trust root ([companions](companions/companions.md)).
- **Canonical encodings** are length-prefixed, fixed-width LE, with **domain-separation tags** — chosen over
  canonical JSON because we own both ends. A policy signature can never authenticate a descriptor.
- **A gateway that goes silent is handled** (`GatewayPool`, `on_gateway_silent`); a `PartnerLink` has a
  `Refreshing` state because *reconnected is not ready*; revocation mid-partition is a first-class transition.

**The transport's first arm is built (PR 8, 2026-09-18):** `federation/edge.rs` (provider side, attached with
`GossipAgent::with_federation_edge`) and `federation/client.rs` (`FederationClient`: `PartnerLink` →
`RemoteResolver` → `GatewayPool` → HTTP, the lock never held across the await). The PR 1 harness's
`assert_never_merged` now runs **after a call has crossed** (`lib_tests.rs` →
`a_federated_call_crosses_and_the_meshes_still_never_merge`), which is when it stops being trivially true: the
membership and native-namespace legs of the release gate are met for the simplest topology. **The gate's
choreography (PR 9, 2026-09-18) — met in its in-process form:** every node under the enforced profile (§9: TLS,
SWIM off) and each mesh under its own auto-generated CA; lose the only gateway (explicit `DeliveryUnknown`s,
link `Down`), keep working locally, change the grant mid-partition, replace the gateway, reconnect (refused until
discovery refreshes, and what it refreshes to is the changed grant), retire the dead gateway
(`FederationClient::retire_gateway`), honour authority issued before the partition to its expiry and not past
it; non-merger asserted from the membership tables, the `consensus/` namespace (keys and values) and the
**connection tables** (`GossipAgent::connected_peers`, the traces leg) before, during and after; a node holding
B's CA cannot join A (`lib_tests.rs` → `the_release_gates_choreography_over_the_transport`). **The catalogue
reply is signed (PR 10a, 2026-09-18):** `CatalogReply` is signed under the domain's key over a tagged canonical
form that covers the domain **and the partner it was filtered for**, so a reply cannot be replayed to another
partner or answered by a gateway without the key; `FederationEdge::with_signing_key` signs,
`FederationClient::with_partner_key` requires — an unsigned, forged, wrong-domain or misaddressed catalogue is
`ClientError::Catalogue` and the link stays `Down`. Freshness stays the resolver's job. **The gate is met
without a caveat (PR 10b, 2026-09-18):** the two-mesh **Docker** suite runs the same choreography with one
container per node and the federation link cut by `docker network disconnect` — the two things the in-process
test could not claim. `make test-federation`; CI job `federation`; files `examples/federation_node.rs`,
`docker/docker-compose.federation.yml`, `tests/integration/run_federation.sh`. Every assertion reads a node's
own tables, never a log line.

**TLS on the edge, anchored on a pin (row 11, 2026-09-23):** `src/federation/pinning.rs`. The credential made a
federated call unforgeable; it was still **readable**, and closing that needed a trust anchor the design did not
have — partner trust is one Ed25519 key per partner, bilaterally chosen, with no X.509 material anywhere. Three
candidates, and the choice is recorded in the module doc: the public Web PKI (a root far larger than the
bilateral trust it would carry, under a design whose whole point is bilateral), an exchange of private CAs (more
machinery, a second rotation story, and a CA signs *any* name — it buys delegation, the one property a two-party
link does not need), or **pin the key in the bundle**. The third, because it is the same rule the module already
states for descriptors: *the bundle decides which key, never the document.* `PartnerTrust::tls_spki_sha256` is a
**list** (`pin_tls`/`unpin_tls`) for the same reason `retiring` exists — one pin makes a TLS key change a flag
day. Two refusals, both meaning **nothing was sent**: `Tls(PinMismatch)` and `Tls(PlaintextEndpoint)`, and
neither is a `DeliveryUnknown`, which would say *it may have run* and bar an at-most-once caller from retrying.
**The lesson worth keeping** is where the SPKI comes from: `mycelium_core::tls::ed25519_key_from_cert_der`
finds a key by **scanning the DER for the Ed25519 SPKI prefix**, which is sound *because its input is already
CA-validated* — and that premise is exactly what is absent at a pinning verifier, whose input is whatever an
attacker sent. A plant puts the victim's key bytes in an attacker-signed certificate's **serial number** (it
precedes the real SPKI in the encoding) and asserts the scan is fooled while the structural read is not. The
same reasoning admits `ed25519_spki_sha256`, which *writes* the RFC 8410 encoding for an already-trusted key —
the safe direction — gated by a test that it equals the structural read of a real certificate. Gate:
`lib_tests.rs` → `a_pinned_federation_link_talks_only_to_the_key_the_bundle_names`, two gateways running the
*same* edge and differing only in their TLS key.

**The budget runs both ways (2026-09-23) — the Phase-C audit's last finding, closed as a decision
with a mechanism.** `GatewayPool` metered slots per partner on the *consumer* side only: it bounded
what we send a partner, and nothing bounded what a partner sends us. The threat is not an anonymous
flood (the credential gate stops that) but a **compromised or buggy partner**, whose credentials are
by construction minted by *them*.

The alternative considered and rejected was the `rate` module's M7 shape — shared observation, local
decision — applied to partner domains: it catches a partner fanning out across several of our
gateways, and it puts **foreign domain names into `sys/`**, which is exactly what D7 prevents, and
tells every node in the mesh which partners exist and how hard each calls. It is also unbuildable
without a local counter to clamp; M7's own design is per-peer limit first, aggregate second.

So: `CallPolicy::max_in_flight_per_partner` (0 = unlimited, nobody acquires a cap by upgrading),
`FederationEdge::admit` → a `PartnerSlot` **RAII guard** (release on reply, on a later refusal, on
an unwind — a release a handler must remember is one it misses, and a leaked slot is a partner
permanently short of capacity with nothing saying so), `CallRefusal::AtCapacity` and JSON-RPC
**-32004**, kept apart from -32003 because a transient load answer must not read as a standing
authority answer. **Capacity is asked after authority**, so an unauthorised partner cannot exhaust
an authorised one's allowance. Lock-order row 40; never nested with row 38.

Two limits stated rather than implied: the count is **one gateway's** (N gateways ⇒ N × cap), and it
bounds **concurrency, not rate**. Gates: `the_edge_meters_calls_per_partner_and_refuses_rather_than_queues`
and `the_per_partner_cap_refuses_a_concurrent_call_at_the_live_gateway` — the second drives two
*concurrent* calls through a real gateway against a provider that blocks until released, because
with an instant provider the first call finishes before the second arrives and the cap is never
consulted.

**The consumer side reaches local clients — the SDK verbs (row 11, 2026-09-23):**
`GossipAgent::with_federation_clients` attaches one `FederationClient` per partner to the node, and five gateway
routes drive them: `GET /gateway/federation/{domain,partners,catalog/{domain}}` under **`federation:read`**,
`POST /gateway/federation/{connect,call}` under **`federation:invoke`** (`src/agent/federation_http.rs`, the
consumer half; handlers in `src/agent/http.rs`). The SDKs do **not** re-implement the edge protocol: the
signing key and the trust bundle stay in the node, so one implementation of the trust decisions serves all three
languages — the alternative would have put credential minting into two more languages and the domain's private
key into an SDK process.

Two rules this adds to the boundary:

- **The credential names the local caller, and the body cannot say otherwise** — item 7's fix at one more
  boundary. `FederationClient::call_as` takes the principal from the authenticated request; a gateway minting
  under its *own* configured principal would record a service account in the partner's evidence for work it
  never asked for, and reading it from the body would let any holder of `federation:invoke` have this domain
  vouch for an unauthenticated identity. On an open gateway the principal is `anonymous` — honest, and the
  reason the runbook says to configure tokens *before* connecting two domains.
- **A refusal states whether anything was sent.** `sent` and `delivery` (`none` · `refused` · `completed` ·
  `unknown`) travel in every refusal body, and both SDKs raise a **distinct type** for `unknown`, so the one
  outcome that must not be blindly retried cannot be caught alongside ordinary failures. The in-crate match has
  **no `_` arm**: `ClientError` is `#[non_exhaustive]` to other crates but not to this one, so a new refusal
  stops the build at the place that must decide what it means. The outbound call is also its own **AE
  enforcement point** (`gateway:federation/call`), because an evaluator guarding `/mcp` and `/a2a` and not this
  route is a remit with a third door open. Gate: `lib_tests.rs` →
  `the_gateway_verbs_carry_the_local_caller_across_the_boundary`.

**More than two domains (row 11, 2026-09-23) — what two could not state.** With exactly two domains,
*filtered for the asker* is indistinguishable from *the export list*, a grant to one partner from a
grant, and **trust is not transitive** cannot be expressed at all. The chain alpha → beta → gamma
(alpha and gamma strangers) states all three, and `lib_tests.rs` →
`three_domains_compose_without_trust_composing` pins them: gamma's link to alpha is refused
`UnknownDomain` **at the auth layer**, so it never learns what alpha exports; beta's catalogue to
gamma names beta's own export and nothing beta holds from alpha (a grant you hold is not re-exported
by holding it); alpha exports two skills, grants one, and a planted credential for the other is
refused at the edge without reaching a provider; non-merger is asserted **pairwise over all three
meshes**, since a domain joined through a third would pass a check of only the pair that exchanged
bytes; and `GatewayPool` slots are **per partner**, so a busy neighbour at a hub is not a denial of
service on everyone else — with two domains "per partner" and "per gateway" are the same number.
**Row 11 is closed.**

**What is still not built:** streaming
(`tasks/sendSubscribe` under a credential is refused: federated calls are unary, §5).
`examples/federated_domains.rs` still runs in one process and says so. Ledger: [history](history.md) → *item 2*.

## A named token is a token model (2026-09-23)

`gateway_auth`'s open-gateway predicate (`src/agent/http.rs`) counted `gateway_auth_token` and
`gateway_scoped_tokens` and **not** `gateway_named_tokens`, which 2.10.0 added and `resolve_token` has honoured
since. A deployment whose only credential model was named tokens therefore ran an **open gateway**: no bearer
required, and `open_gateway_scopes` granting each request exactly the scope its route asks for — deny-by-default
inverted. Affected 2.10.0–2.12.0; `GossipConfig`'s own docs say to *prefer* named tokens, so the recommended
configuration was the affected one.

**Why it survived two audits and a fuzz campaign:** the tokens worked. Presenting one was admitted, so every
positive test passed; the only way to see the hole was to present **nothing**, and the one test that configures
named tokens sets the positional table too. Same shape as 2.11.1's unreached fuzz seeds — *a gate that looks
covered because its positive case passes*. The general lesson for this codebase: **an auth test that never sends
an unauthenticated request proves nothing about the gate**, and a predicate listing credential sources is a
completeness claim of the lock-order-table kind — adding a source means adding a term. Pinned by
`named_tokens_alone_still_close_the_gateway` (404-free negative, positive control, and the scope bound still
applied), which fails when the fix is reverted.
