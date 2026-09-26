# The engagement kit — one bounded customer integration, end to end

*2026-09-26. Built from an external customer-materials review that found no Mycelium-specific
consultancy kit in either repository — only ingredients. This page is the kit's spine; each part
names the existing page it packages and the template it adds. Its acceptance test is at the end,
and it is the only test that matters: an engineer or consultant who did not write the system
completes the bounded engagement without undocumented help.*

**Why a kit at all.** Mycelium is a library. There is nothing to install onto a cluster and switch
on; someone must build and own the customer's integration. That is why three roles are named
throughout — the **consultant** (scopes, rehearses, hands over), the **application engineer**
(embeds the library or fronts one tool path with the gateway, in the customer's runtime), and the
**customer operator** (runs it afterwards). A kit that does not say which role does each step has
not said how the work gets done.

**Scope of the first engagement** ([customer-pilot.md](customer-pilot.md) says why): one existing
application or agent integration, one gateway, a small provider set, one protected business
operation, in the customer's existing runtime. Demonstrate discovery, an authorised action, a refused
action, the evidence trail, and a correction visible downstream. Dynamic consensus membership, cloud
portability, hard financial budgets, autonomous provisioning and general rogue-agent containment are
**separate acceptance dimensions**, each with its own gate; narrowing the demo does not close them.

---

## 1 · Qualification and scope

Decide with the customer, in writing, before anything is built:

| Item | Record |
|---|---|
| The **one business outcome** | a sentence a sponsor would sign |
| The **target application** | name, runtime, language, owner, deployment platform (unchanged by this engagement) |
| **Trusted or untrusted profile** | is the integrated code a trusted mesh member, or an untrusted agent workload behind the gateway? The network and credential designs differ — [confined-fleet](../design/confined-fleet.md) is the untrusted one |
| **Cloud / on-prem constraints** | which cloud, which regions, what may leave the network |
| **Success criteria** | the acceptance script's rows (§5) the customer will watch, and the number that decides |
| **Explicit exclusions** | the dimensions above that are *not* in this engagement, named |

The qualifying question is the buyer deck's: is the coordination genuinely distributed — across
parties, sites or failure domains? Qualify by the workload and its value, not by the number of legal
entities.

## 2 · Pre-site workbook

Answers recorded **before travel**; a blank in this table is a day lost on site.

| Question | Answer |
|---|---|
| Hosts and architectures (x86/arm, OS, container runtime, Kubernetes version if any) | |
| Ports, DNS, proxy and egress rules — gossip port, gateway port, admin port; what the [confined-fleet NetworkPolicy](../../deploy/confined-fleet/network-policy.yaml) would need; **DNS egress is allowed by that policy and can carry data** | |
| Container capability — can the customer build and run the node image ([deployment.md](deployment.md))? | |
| Credentials and their owners — CA key custody ([cert-rotation.md](cert-rotation.md)), gateway tokens ([rbac.md](rbac.md)), OIDC issuer ([sso.md](sso.md)) | |
| Permitted telemetry — may `/metrics` be scraped, may the evidence journal leave the node, to where ([audit.md](audit.md)) | |
| Storage — where the WAL, identity, evidence journal and any companion store live, and how they are backed up **together** ([deployment.md § Backup & restore](deployment.md#backup--restore)) | |
| Model and provider access — which LLM endpoints, which tools, reachable from where | |
| Change approvals — who signs a policy activation, a key rotation, a member removal | |

## 3 · Integration workbook

Filled by the application engineer with the consultant. One row per tool or capability the
integration exposes; **a filled example** for the co-op's procurement agent sits beside the blank
in [`examples/coop/README.md` § 13](../../examples/coop/README.md).

| Column | Meaning |
|---|---|
| Capability / tool | the operation name as the gateway will see it (`tools/call` name, or the A2A skill) |
| Request schema | what the caller sends; what is signed into the body digest |
| Business activity | the catalogue entry it maps to, or **unmapped** — never a guess from the tool's name |
| Principal(s) | who may call it, as the gateway names them (issuer-qualified) |
| Remit / grant issuer | who signs the mandate that permits it, and its ceiling |
| Enforcement point | gateway preflight only, or provider-side (`with_provider_enforcement`), or at the resource with the fencing token |
| Bypass paths | every other way to reach the provider — listed, and each one closed or evidenced |
| Evidence destination | the journal path, the exporter, the consumer's stream name |
| Gaps | what this integration does **not** observe or enforce, in the customer's words |

## 4 · Pinned installation pack

The combined candidate is one thing, or it is not a candidate:

- **The substrate tag** and, where the private companion is used, **its commit and lockfile**, with
  the compatibility manifest that names both (the companion ships `COMPATIBILITY.md`; the substrate's
  side is the tag's `CHANGELOG.md` entry).
- **Features** built, **image digest** if containerised, **SDK versions** (`mycelium-py`,
  `mycelium-ts`, `langgraph-checkpoint-mycelium`, each on its own tag), the **consumer contract
  version** where evidence is exported.
- **Build provenance** — who built the artefact, from which commit, on which toolchain — and the
  **licensing route**: the public substrate is AGPL with a commercial embedding route; the private
  companion's licence grants no third-party use on its own. The engagement carries the actual
  evaluation or commercial licence and support scope, not the repository notices.
- **Secure configuration templates** — the [production-readiness](production-readiness.md) sweep
  turned into a `GOSSIP_*` file per profile: TLS on, gateway auth on, the A2A route behind an
  evaluator or an ingress, `require_identity_proofs` decided, the consensus profile if consensus
  is used. **A profile that fails startup when a required piece is missing** is the ask; until it
  ships, the template plus the checklist is the control, and this line says so.
- **Offline dependency plan** where the customer's network cannot reach crates.io or a registry:
  a vendored source tree or a pre-built image, decided in §2.

## 5 · Acceptance script

Each row is run in front of the customer, by the customer operator where possible, with the
**expected output written before the run** and a pass/fail recorded. No invented all-clear: a row
that cannot be run is recorded as *not run*, never as passed.

| # | Scenario | Expected | Run doc |
|---|---|---|---|
| 1 | Normal work through the boundary | permitted; decision record names what it checked | [procurement_authority act 1](../../examples/coop/README.md) |
| 2 | Denied work | refused at the gateway; **no** business effect; evidence says who attested | act 2 |
| 3 | Missing authority | *authority not established* — not a denial, not drift | act 3 |
| 4 | Malformed request | `-32700` on `/a2a`; a 4xx on the gateway; nothing dispatched | [production-readiness](production-readiness.md) |
| 5 | Node loss | capabilities re-advertise, work re-routes; `/ready` on survivors | [provisioning](../../examples/coop/README.md), [diagnostics](../../examples/coop/README.md) |
| 6 | Partition | both sides keep serving what they can; consensus (if used) refuses rather than double-commits under the supported profile | [threat-model §7](../threat-model.md) |
| 7 | Revocation | a revoked mandate is refused at every door; running work stops within its bound | [`authority_drain`](../design/authority-at-execution.md), the C7 matrix |
| 8 | Late outcome | an outcome arriving after the exporter flushed is a new record plus a correction, never a changed one | the companion's exporter tests |
| 9 | Evidence correction | the correction cites what it replaces; the original stays | act 6 |
| 10 | Restart | identity, epochs and journal come back together; a revoked mandate is **still** refused | [deployment § Backup & restore](deployment.md#backup--restore) |
| 11 | Backup restore | the same, from the backup taken in §2 — and the outbox generation bumped where a companion is used | same |

During rows 5–11, **interrupt the exporter and restart components**: the script is not a happy path.

## 6 · Operations and handover

What the customer operator receives, each with the page that backs it: dashboards and alerts
([observability.md](observability.md), [metrics.md](metrics.md), [diagnostics.md](diagnostics.md));
backups ([deployment.md](deployment.md)); key rotation ([cert-rotation.md](cert-rotation.md));
retention of the evidence journal and any outbox, with the disk envelope stated; upgrade and
rollback ([deployment.md § Upgrades](deployment.md)); the **known limitations** in the customer's
words (the *not claimed* lines of the release notes, and §1's exclusions); a **named support
owner**; and a rehearsal in which the customer operator, not the consultant, runs rows 5, 7 and 10.

## 7 · Closeout and teardown

Customer findings recorded as findings, not smoothed; the evidence collected during §5 handed over;
residual risks and the **next decision** the customer is being asked to make; every test credential
revoked and every test key rotated out; test data and resources removed, or their retention agreed
and written down. A closeout that leaves a live token behind has not closed.

---

## The acceptance test for this kit

An engineer or consultant who did not write the system takes the pinned pack (§4), the workbooks
(§2, §3) and the script (§5) onto a clean machine and completes §1's bounded engagement — including
a restore, a rotation, a deliberately broken dependency diagnosed, and a clean teardown — **without
asking anyone who built it**. Every question they had to ask is a defect in this kit, and is filed
as one.
