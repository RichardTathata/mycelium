# Shared-responsibility matrix (SOC 2 / adopter-facing)

↑ [operations](README.md)

> **What this is.** Mycelium is an **embedded library, not a hosted service** — there is no
> vendor-run control plane. A SOC 2 report is issued to a *service organisation* about an *operated
> system*; the operated system is **your deployment**, so **your** SOC 2 covers your use of Mycelium.
> This table says, per control area, what the **library provides** (a control you inherit and
> configure), what is **shared**, and what **you own** — so "deployer-owned" reads as a deliberate
> boundary, not a gap. It is the evidence artifact your auditor and your customers' security reviews
> ask for on day one.
>
> Mycelium is **CFT, not BFT**: a compromised *admitted* node is formally out of scope (see
> [threat-model](../threat-model.md)). Controls below are "detection-not-prevention" unless noted —
> enforcement happens where a resource is *served*, never as a Layer-I write guard.

Legend: **M** = Mycelium provides (inherit + configure) · **S** = Shared · **D** = Deployer owns ·
⚠ = open gap, tracked in [soc2-audit-gap-closure](../plans/soc2-audit-gap-closure.md).

This matrix says what the library provides and what you own; what the project has and has not
**shown** — proven in CI, demonstrated with a bound, not yet shown — is its companion page,
[what-is-proven.md](what-is-proven.md), dated and refreshed at every release.

## The org-level criteria (CC1–CC5, CC9) — not the library's to hold

CC1 Control Environment · CC2 Communication · CC3 Risk Assessment · CC4 Monitoring · CC5 Control
Activities · CC9 Risk Mitigation are **organisational** — your governance, HR, vendor management,
risk register, policies, and the observation-window evidence that they *operated*. **D** across the
board. Mycelium contributes only as a well-documented **vendor** in your CC9 vendor-risk process
(see the vendor-assurance package: a corporate/SDLC SOC 2 + pentest + this matrix).

## CC6 — Logical & physical access controls (the meaty one)

| Control | Who | Mycelium provides | You own / configure |
|---|---|---|---|
| Node authentication | **M** | mTLS mutual admission against a cluster CA; Ed25519 node identity (`tls`) | Distribute + protect the CA; file-protect on-disk keys |
| Peer-identity integrity | **M** | CA-anchored keys (1b) + **signed `sys/identity-proof/`** (Phase 2 ✅): an identity overwrite is **rejected** unless its proof chains to a key already trusted for that peer. **`require_identity_proofs`** (Phase 3 ✅; on by default since post-360 row S) also rejects entries it cannot authenticate — closing the poisoning vector short of first sighting (trust on first use). `identity_anchor_conflicts` counts rejections | Alert on `identity_anchor_conflicts` > 0. Keep `require_identity_proofs` on (the default); set it `false` only while a node from before 2.3.0 (no proof) remains (→ [cert-rotation](cert-rotation.md)) |
| Operator authentication (SSO) | **M** | OIDC/JWT validation, alg-confusion-safe (`compliance`) | Wire your IdP; `group_scopes` mapping |
| Authorization (RBAC) | **M** | Signed role claims + L1/L2/L3 clearance; forged KV role reads back `None` | Issue/rotate role claims; set clearances |
| Capability authorization | **M** | `authorized_callers` allowlists; resolve-time `capauthz` (route around unauthorised advertisers) | Define policies; empty = open by design |
| Gateway access control | **M** | OAuth2-style `resource:verb` scopes, **deny-by-default** (unmapped route ⇒ `admin`) | Set `gateway_scoped_tokens` / OIDC scopes; keep `/health /ready /metrics` the only public routes |
| Encryption in transit (gossip) | **M** | mTLS on the gossip TCP transport (`tls`) | Enable `tls` (opt-in) |
| Encryption in transit (gateway) | **S** | **Native gateway TLS** (`gateway_tls`, WS-A ✅) **or** front with a TLS proxy | Pick one — never leave the gateway plaintext on a routable interface. → [gateway-tls](gateway-tls.md) |
| Encryption in transit (SWIM UDP) | **D** | Liveness only (no KV/data); plaintext, unauthenticated | Network-segment it if your policy requires |
| Key management — rotation | **M** | Hot Ed25519 identity/cert rotation, no dropped frames | Operate the rotation cadence |
| Key management — compromise | **M** | Signed revocation consulted on all verify paths incl. consensus; `rotate_identity_on_compromise` + `POST /gateway/identity/revoke` (WS-B ✅) | Trigger it on compromise. Force-revoking a *dead/fully-compromised* node's key needs a separate operator authority (not provided). → [cert-rotation](cert-rotation.md) |
| Key custody (CA, at-rest KMS) | **D** | `DataAtRestCipher` hook (disk boundaries) | CA-key custody; wrap a KMS/HSM for at-rest |
| Encryption at rest | **S** | Optional at-rest cipher hook (WS3) | Supply the cipher + key custody |
| **Cross-domain access (federated domains)** | **S** | Bilateral, credentialed, policy-revisioned federation: a partner domain never enters membership, the native namespaces, anti-entropy or a quorum. Every call carries a credential bound to **one** export; absence is denial, with no wildcard and no default-allow. The filtered catalogue never discloses the full export list | **Populate the trust bundle deliberately** — there is no registry and no discovery of trust. Revoke **both** halves (trust *and* grant): either alone leaves half the relationship live. Set the overlap on a key rotation wider than the partner's deployment window. TLS on the federation edge **itself** ships since 2026-09-23 and is **off unless pinned**: `PartnerTrust::tls_spki_sha256` + `FederationClient::with_tls_pins`, anchored on a key you already chose rather than a CA. Also bound what a partner may send you — `CallPolicy::max_in_flight_per_partner` is **0 (unlimited) by default**, and the cap is **per gateway**. → [federation](federation.md) |
| **Scoped authority over a resource (mandates)** | **S** | An appointment with **enumerated** operations (no wildcard), ordered by a monotonic epoch. Once a resource installs a later epoch, nothing authorised only under an earlier one can commit there — whatever the holder retries or restarts. The refusal is `MandateSuperseded`, deliberately **not** a retryable conflict | Issue, renew and withdraw appointments, and keep the three lifecycle events apart in your evidence (*expired* ≠ *withdrawn*). **One resource-side fence ships** (the wiki's store); any other resource wanting this guarantee implements it in its own atomic boundary. → [audit §8](audit.md), [guide 21](../guide/21-mandates.md) |

## CC7 — System operations (monitoring, detection, response)

| Control | Who | Mycelium provides | You own |
|---|---|---|---|
| Security tripwires | **M** | `sys_namespace_violations`, `cap_authz_violations`, `commit_conflicts`, `schema_mismatch` on `/stats` (feature-free) + Prometheus; `governance_unaudited` on `/stats` (governance changes at the gateway that left no audit record) | Alert on them (esp. namespace-violation > 0, and `governance_unaudited` > 0 where the audit trail is a control) |
| Metrics / observability | **M** | Prometheus `/metrics` (`metrics`); emergent-pathology + view-health gauges | Scrape; Grafana; alert recipes → [diagnostics](diagnostics.md) |
| Coordinator-free diagnosis | **M** | `/gateway/{fleet,explain,diagnose}` — plain-English fleet findings | Consume in your ops flow |
| Audit trail | **M** | Per-node hash-chained, Ed25519-signed records + verification API (`compliance`) | Requires `tls` to seal; **instrument application-level events yourself** |
| Audit export | **S** | `AuditSink` streams every sealed record to your SIEM/WORM off the write path (WS-C ✅) | Implement the sink → your SIEM/WORM; chain stays authoritative on a slow sink. → [audit](audit.md) |
| Audit retention | **S** | Signed **checkpoints** (`audit_checkpoint` + `audit_prune_to_checkpoint`) let exported records be pruned while the rest still verifies (WS-D ✅) | Schedule checkpoint→export→prune; WORM is your 7-yr system of record. → [audit](audit.md) |
| Incident response | **D** | Signals + revocation as building blocks | Your IR runbooks, on-call, drills |

## CC8 — Change management

| Control | Who | Mycelium provides | You own |
|---|---|---|---|
| Wire/protocol change | **M** | `WIRE_VERSION` + documented N→N+1 rolling-upgrade policy, test-gated | Roll upgrades within the window |
| Schema evolution | **M** | Registered migrations (`schema_evolution`), never silent coercion; `schema_mismatch` tripwire | Register your migrations |
| Application change management | **D** | — | Your SDLC, approvals, deploy pipeline |
| Stem activation commands (`[[activation]]`) | **D** | The stem runs the argv a unit file declares after a blob is placed, and its probe, with a timeout (default 300 s) — by default with the stem process's own privileges (the WASM sandbox covers components, not these commands). With an `[activation.confine]` profile it runs them under OS confinement (Seatbelt on macOS; Landlock and seccomp on Linux): the placement directory and declared paths, declared TCP ports or no network; a profile the node cannot apply refuses the activation, never runs it unconfined; `stem.activation_confined` in the guarantee report says which applies. Not limited: metadata reads, CPU and memory, and on Linux below Landlock ABI 6 signals to the stem user's processes | Review unit files as code; run the stem as a least-privileged user; name a confinement profile for each activation and keep it to the runtime you intend → [unit-file reference](../reference/unit-file.md#activationconfine) |

## Confidentiality & Availability (optional TSC categories)

| Control | Who | Mycelium provides | You own |
|---|---|---|---|
| Data classification | **M** | L1/L2/L3 clearance in RBAC | Classify + assign |
| Data isolation | **S** | Single cluster **isolated by construction** (no cross-cluster feature) | One cluster = one confidentiality/residency boundary; run per-tenant/region as needed |
| Data residency | **D** | Isolation property + docs | **Where nodes run is yours** — placement, borders |
| Right to erasure (GDPR) | **S** | `SubjectKeyRegistry` crypto-shred helper (WS-F ✅): per-subject DEK; erase = destroy the key → all ciphertext dead. Physical deletion isn't guaranteeable in a gossip+WAL mesh — this is the standards-recognised mechanism. → [data-erasure](data-erasure.md) | DEK custody (**KMS** for production), residency (node placement), keep PII out of audit/logs/labels, backup policy that honours destruction |
| Availability | **S** | AP design (partition-tolerant), epidemic redundancy, anti-entropy repair | Capacity, backups, multi-AZ, DR |
| Egress control | **S** | `EgressPolicy` fail-closed allowlist (per node) | Set `allow_hosts`; network-layer enforcement |

## From a `doctor` line to its row

`mycelium doctor` names one guarantee per line, by id; the [guarantee catalogue](../reference/guarantee-catalogue.md)
says what each promises. This table takes a line of that output to the row above that says who holds it, so an
auditor reading a pilot's `doctor` output, or the [pilot kit](pilot-kit/README.md)'s report, can go from a state to a
responsibility without reading the code. `scripts/check-pilot-kit.py` (in `make check`) holds it complete: every id in
the catalogue is here, every row it names is a row of this page, and a line no row covers says so and why.

| Doctor line | Row (section) |
|---|---|
| `gw.not_open` | Gateway access control (CC6) |
| `gw.exposed_closed` | Gateway access control (CC6) |
| `gw.token_tables` | Gateway access control (CC6) |
| `gw.oidc` | Operator authentication (SSO) (CC6) |
| `gw.tls` | Encryption in transit (gateway) (CC6) |
| `gw.caller_profile` | Capability authorization (CC6) |
| `mesh.tls` | Node authentication · Encryption in transit (gossip) (CC6) |
| `mesh.signed_writes` | Peer-identity integrity (CC6) |
| `id.proofs_required` | Peer-identity integrity (CC6) |
| `id.ca_key_off_node` | Key custody (CA, at-rest KMS) (CC6) |
| `id.revocation_authority` | Key management — compromise (CC6) |
| `ae.authorised_at_seam` | Capability authorization (CC6) |
| `ae.recorded_before_dispatch` | Capability authorization (CC6) · Audit trail (CC7) |
| `prov.enforcement` | Capability authorization (CC6) |
| `a2a.admission` | Gateway access control (CC6) |
| `authz.execution_authority` | Scoped authority over a resource (mandates) (CC6) |
| `authz.durable_epochs` | Scoped authority over a resource (mandates) (CC6) |
| `audit.chain` | Audit trail (CC7) |
| `audit.sink` | Audit export (CC7) |
| `egress.allow_list` | Egress control (Confidentiality & Availability) |
| `egress.credentials` | Egress control (Confidentiality & Availability) |
| `persist.configured` | Availability (Confidentiality & Availability) |
| `persist.sync_mode` | Availability (Confidentiality & Availability) |
| `persist.unreadable_refused` | Availability (Confidentiality & Availability) |
| `at_rest.cipher` | Encryption at rest (CC6) |
| `cons.safety_profile` | *no row* — an exclusive outcome decided once is processing integrity, a category this matrix does not cover; [guide 04](../guide/04-consensus.md) is its record |
| `fed.domain_enforced` | Cross-domain access (federated domains) (CC6) |
| `fed.pinned` | Cross-domain access (federated domains) (CC6) |
| `fed.body_bound` | Cross-domain access (federated domains) (CC6) |
| `fed.partner_slots` | Cross-domain access (federated domains) (CC6) |
| `fed.caller_named` | Cross-domain access (federated domains) (CC6) |
| `fed.catalogue_signed` | Cross-domain access (federated domains) (CC6) |
| `fed.partner_egress` | Cross-domain access (federated domains) (CC6) |
| `fed.partner_posture` | Cross-domain access (federated domains) (CC6) |
| `fed.non_merger` | Cross-domain access (federated domains) (CC6) |
| `fed.clock_agreement` | Cross-domain access (federated domains) (CC6) |
| `trace.spans_not_evidence` | Metrics / observability (CC7) |
| `net.confinement` | Egress control (Confidentiality & Availability) |
| `clock.sync` | Metrics / observability (CC7) |

A line that reads `not_verifiable_here` is the row's *You own* column in the node's own words: the fact is outside what
the node can observe, and the deployment's evidence is what shows it.

## How to read the ⚠ rows

Each open gap is scoped, sequenced, and sized in
[soc2-audit-gap-closure](../plans/soc2-audit-gap-closure.md). Status at last update
(2026-07-22): **WS-A gateway TLS shipped**; WS-B (revocation glue), WS-C/D (audit export +
retention), WS-E (identity authentication), WS-F (erasure) in progress/pending. This matrix's cells
flip as each lands — treat it as the living definition of done.
