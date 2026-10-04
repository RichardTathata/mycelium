# Guarantee catalogue

**Generated** from `core_guarantees()` (`src/agent/guarantee.rs`); do not edit. Regenerate with `UPDATE_GUARANTEE_CATALOGUE=1 cargo test --lib --features compliance,a2a the_checked_in_guarantee_catalogue_is_current`. A guarantee is a claim the startup report resolves against the node as built and configured — `enforced` · `not_configured` (the setting named) · `not_in_build` (the feature named) · `not_applicable` (the role fact named) · `not_verifiable_here` (external prerequisites, never counted). The plan is `docs/plans/guarantees-and-rule-catalogue.md`; the live report is `guarantee_report()` / `GET /gateway/guarantees`.

Schema `mycelium.guarantees/1` · 24 core guarantees.

## The descriptors

| Id | Rev | Subsystem | Kind | Promise | Needs | Enforcement points | Docs |
|---|---|---|---|---|---|---|---|
| `gw.not_open` | 1 | gateway | NodeEnforced | the HTTP gateway requires a credential on every non-public route | `gateway_auth_token`, or (`compliance`) a token table or `[oidc]` | `gateway_auth` | `docs/operations/rbac.md` |
| `gw.token_tables` | 1 | gateway | NodeEnforced | scoped or named tokens close the gateway with a per-route scope floor | `compliance`; `gateway_scoped_tokens` / `gateway_named_tokens` | `gateway_auth`, `required_scope` | `docs/operations/rbac.md` |
| `gw.oidc` | 1 | gateway | NodeEnforced | human operators authenticate to the gateway through the IdP, groups mapped to scopes | `compliance`; `[oidc]` | `gateway_auth`, `oidc::OidcVerifier` | `docs/operations/sso.md` |
| `gw.tls` | 1 | gateway | NodeEnforced | the gateway serves HTTPS, so bearers and JWTs do not cross the wire in cleartext | `tls`; `[gateway_tls]` (or TLS terminated by a proxy in front — not visible here) | `http::serve_https` | `docs/operations/gateway-tls.md` |
| `gw.caller_profile` | 1 | gateway | NodeEnforced | a provider sees the gateway's client as the caller, attested by the gateway's identity | `tls`; `[tls]`; `gateway_caller_profile = secure` (the default) | `gateway_caller::attest`, `gateway_caller::verify` | `docs/guide/20-authorising-actions.md` |
| `mesh.tls` | 1 | transport | NodeEnforced | gossip is mTLS: every peer presents a certificate signed by the fleet CA | `tls`; `[tls]` | `lifecycle::start (tls init)`, `connection::handshake` | `docs/guide/09-security.md` |
| `id.proofs_required` | 1 | identity | NodeEnforced | an identity entry this node cannot authenticate is rejected (issuer binding rests on it) | `tls`; `[tls]`; `require_identity_proofs = true` (default off) | `lifecycle (identity-proof check)` | `docs/design/identity-authentication.md` |
| `id.ca_key_off_node` | 1 | identity | NodeEnforced | the fleet CA's private key is not on this node, so a removed member cannot mint itself a new identity here | `tls`; `[tls] cert_pem` + `key_pem` (a node certificate issued off-node: `mycelium tls issue`); no `ca-key.pem` in this node's certificate directory | `membership removal (closure plan C5)` | `docs/operations/cert-rotation.md` |
| `ae.authorised_at_seam` | 1 | authority | NodeEnforced | every gateway dispatch is authorised by policy before it runs — permit, deny or indeterminate | `gateway` + `tls`; `with_action_evaluator` | `http::ae_preflight` | `docs/guide/20-authorising-actions.md` |
| `ae.recorded_before_dispatch` | 1 | authority | NodeEnforced | every authorisation decision is journalled, fsynced, before dispatch | `gateway` + `tls`; `with_action_evaluator` and `with_evidence_journal` | `http::ae_record` | `docs/guide/20-authorising-actions.md` |
| `prov.enforcement` | 1 | authority | NodeEnforced | protected work (`mcp.invoke`, `skill.invoke`, …) is authorised where it runs, not only at a gateway | `gateway` + `tls`; `with_provider_enforcement` (with an evaluator; without one every protected call is refused) | `provider_enforcement::check` | `docs/guide/20-authorising-actions.md` |
| `a2a.admission` | 1 | authority | NodeEnforced | `/a2a` is not anonymous skill dispatch | `a2a`; an evaluator that denies anonymous principals — a bearer does not gate this route | `http::a2a_optional_auth`, `a2a preflight` | `docs/operations/production-readiness.md` |
| `authz.execution_authority` | 1 | authority | NodeEnforced | mandates are established and verified at this node (scoped authority, revocation) | `gateway` + `tls`; `with_execution_authority` | `gateway_authority::ExecutionAuthority` | `docs/guide/21-mandates.md` |
| `authz.durable_epochs` | 1 | authority | NodeEnforced | a restart does not restore revoked authority: installed epochs are journalled first | `gateway` + `tls`; `ExecutionAuthority::with_durable_epochs` | `gateway_authority::install_epoch_durably` | `docs/guide/21-mandates.md` |
| `audit.chain` | 1 | audit | NodeEnforced | governance actions at the gateway are sealed into the tamper-evident chain | `compliance`; `[tls]` (records are sealed with the node identity) | `audit::seal_and_write` | `docs/operations/audit.md` |
| `audit.sink` | 1 | audit | NodeEnforced | sealed records are mirrored to an external sink (SIEM / WORM), original bytes kept | `compliance`; `[tls]`; `with_audit_sink` | `lifecycle (sink drain)` | `docs/operations/audit.md` |
| `egress.allow_list` | 1 | egress | NodeEnforced | the substrate's own outbound calls are restricted to a hostname allow-list (MCP bridge, LLM, probes, skillrunner, the wasm host, the wiki sink, the federation client, OIDC) — the first URL and every redirect hop, with the host read the way the client reads it | `egress.allow_hosts` non-empty (empty allows all); the federation client (the node's policy applied to clients it is handed, `FederationClient::with_egress` elsewhere) and OIDC discovery + JWKS since 2026-10-03 (a denied issuer refuses `start()`). Since 2.22.1: every redirect hop re-checked (≤ 5, never https → http), none followed by a client without the policy or carrying a credential header (`mycelium::egress_client`); the host parsed with the client's own WHATWG parser; an object store gated on the endpoint it dials, not its bucket. An `HttpLibrarySource` or `OllamaProbe` built outside an agent is gated only when built `with_egress`. **Not covered:** name resolution (an allowed name resolving to a denied address), a cloud identity's credential traffic, redirects inside `object_store`'s own client | `config::EgressPolicy::permits_url` | `docs/operations/crown-jewel.md` |
| `persist.configured` | 1 | persistence | NodeEnforced | a restart recovers the KV and acceptor memory from disk | `[persistence]` | `lifecycle (WAL replay, snapshot)` | `docs/operations/deployment.md` |
| `persist.sync_mode` | 1 | persistence | NodeEnforced | a WAL acknowledgement is a durability claim: the write is on disk before the ack | `[persistence]` with `sync_mode` other than `async` | `wal::append (SyncMode)` | `docs/design/contracts-receipts.md` |
| `persist.unreadable_refused` | 1 | persistence | NodeEnforced | a node does not start over persisted state it could not read — nothing is compacted over an unreadable snapshot or WAL | `[persistence]` with `on_unreadable = "refuse"` (the default) | `lifecycle (replay; `persistence::quarantine_unreadable`)`, `persistence::do_snapshot (aborts on a corrupt record)` | `docs/operations/deployment.md` |
| `at_rest.cipher` | 1 | persistence | NodeEnforced | the WAL and snapshot are encrypted at rest | `[persistence]`; `with_data_at_rest_cipher`, before `start()` | `lifecycle (cipher read once at start)` | `docs/operations/crown-jewel.md` |
| `cons.safety_profile` | 1 | consensus | ExternalPrerequisite | safety-sensitive agreement runs the supported profile: a fixed voter set, a strict-majority quorum, identical trust slices | chosen per proposal in `ConsensusConfig`; nothing validates it (plan §8) | `consensus::propose` | `docs/threat-model.md` |
| `net.confinement` | 1 | deployment | ExternalPrerequisite | agent pods reach nothing but their gateway | the network: separate pods, an enforcing CNI, a NetworkPolicy |  | `docs/design/confined-fleet.md` |
| `clock.sync` | 1 | deployment | ExternalPrerequisite | this node's wall clock is within the bound every expiry and freshness check assumes | deployment time-sync (NTP or equivalent) |  | `docs/threat-model.md` |

## The state matrix

Each guarantee's resolution on an **unstarted** node (nothing attached) under the reference configurations, in the build the generator ran under (`compliance,a2a`, which implies `gateway` + `tls`). A column is a configuration, a cell is the state the report would log at `start()`; what is missing or not applicable is on the report itself, not here.

| Id | default | `dev` profile | persistence + egress | gateway + bearer |
|---|---|---|---|---|
| `gw.not_open` | not_applicable | not_applicable | not_applicable | enforced |
| `gw.token_tables` | not_applicable | not_applicable | not_applicable | not_configured |
| `gw.oidc` | not_applicable | not_applicable | not_applicable | not_configured |
| `gw.tls` | not_applicable | not_applicable | not_applicable | not_configured |
| `gw.caller_profile` | not_applicable | not_applicable | not_applicable | not_configured |
| `mesh.tls` | not_configured | not_configured | not_configured | not_configured |
| `id.proofs_required` | not_configured | not_configured | not_configured | not_configured |
| `id.ca_key_off_node` | not_configured | not_configured | not_configured | not_configured |
| `ae.authorised_at_seam` | not_applicable | not_applicable | not_applicable | not_configured |
| `ae.recorded_before_dispatch` | not_applicable | not_applicable | not_applicable | not_configured |
| `prov.enforcement` | not_configured | not_configured | not_configured | not_configured |
| `a2a.admission` | not_applicable | not_applicable | not_applicable | not_applicable |
| `authz.execution_authority` | not_applicable | not_applicable | not_applicable | not_configured |
| `authz.durable_epochs` | not_applicable | not_applicable | not_applicable | not_applicable |
| `audit.chain` | not_applicable | not_applicable | not_applicable | not_configured |
| `audit.sink` | not_applicable | not_applicable | not_applicable | not_configured |
| `egress.allow_list` | not_configured | not_configured | enforced | not_configured |
| `persist.configured` | not_configured | not_configured | enforced | not_configured |
| `persist.sync_mode` | not_applicable | not_applicable | enforced | not_applicable |
| `persist.unreadable_refused` | not_applicable | not_applicable | enforced | not_applicable |
| `at_rest.cipher` | not_applicable | not_applicable | not_configured | not_applicable |
| `cons.safety_profile` | not_verifiable_here | not_verifiable_here | not_verifiable_here | not_verifiable_here |
| `net.confinement` | not_verifiable_here | not_verifiable_here | not_verifiable_here | not_verifiable_here |
| `clock.sync` | not_verifiable_here | not_verifiable_here | not_verifiable_here | not_verifiable_here |
