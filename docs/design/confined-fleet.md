# The confined-fleet profile (ADR, Boundary H item H7)

**Status:** **adopted** 2026-09-24. Implemented as a reference deployment (`deploy/confined-fleet/`), a node self-report
(`GossipAgent::confinement_report`, `src/agent/confinement.rs`), an operator runbook
([`operations/confined-fleet.md`](../operations/confined-fleet.md)), and a deployment test
(`scripts/test-confined-fleet.sh`, CI job *Confined-fleet deployment*). Plan:
[`plans/boundary-h.md`](../plans/boundary-h.md) §8 H7. Threat model: §5 Boundary H.

> Posture, once: **acts off the substrate are invisible to it.** The profile's purpose is to turn *invisible* into
> *impossible*: agent code never holds a member key, and agent pods can reach nothing on the network except their
> gateway, which authorises and records what they do. Prevention shape (iii): an **opt-in profile with a declared
> strength**. The substrate verifies what it can see, and says what it cannot.

---

## 1. The problem

In the 2026 OpenAI–Hugging Face incident, the decisive acts went *around* everything a coordination layer could
see: direct network calls, a leaky package proxy, a dormant outside wiki used as a message board, and credentials
the substrate never issued. Every other Boundary H mitigation (attribution, verified evidence, cohorts, challenge
admission) only covers acts that pass through the substrate. For an agent fleet, the question that decides whether
those mitigations matter is: *can an agent act without passing through the substrate?*

## 2. Decision

### 2.1 Separate pods is the reference, not an in-pod sidecar

Containers in one Kubernetes pod share a network namespace, so ordinary `NetworkPolicy` cannot give an agent
container one egress policy and its gateway sidecar another. Whatever the gateway can reach, the agent can too.
**Agents and gateways therefore run in separate pods.** An in-pod mechanism that separates processes (per-process
egress enforcement, for example) is not the reference. Anyone proposing one needs its own enforcement argument and
its own tests.

### 2.2 The surfaces, each closed

| Surface | Rule | Where |
|---|---|---|
| Member key | Mounted **only** in the gateway pod | `gateway.yaml` (the `member-key` Secret) |
| Network egress | Agent pods: default-deny; allow only the gateway's API port and cluster DNS | `network-policy.yaml` |
| Gateway admin / metrics | Not reachable from agent pods; operator pods only | `network-policy.yaml` (`gateway-ingress`) |
| Kubernetes API | `automountServiceAccountToken: false`; no RBAC bindings for the agent service account; not reachable on the network | `agents.yaml`, `network-policy.yaml` |
| Cloud credentials | No cloud IAM binding on the agent service account | `agents.yaml` (comment), runbook |
| Instance metadata (169.254.169.254) | Not reachable: covered by default-deny egress | `network-policy.yaml`, runbook |
| The CNI | **Must enforce NetworkPolicy.** On one that does not, the manifests confine nothing | Runbook: how to check |

Node-level settings the profile also requires:
- `egress.allow_hosts` non-empty, so the substrate's own outbound paths fail closed;
- `require_identity_proofs = true`;
- an audit sink attached;
- an action evaluator **with** an evidence journal.

Reader-level settings, for a node that reads knowledge:
- `UncheckedRule::Exclude`;
- `UndeclaredRule::OneGroup` or `Excluded`;
- the fleet declared as a cohort.

### 2.3 Two evidence records, never conflated

| Record | Produced by | Shows |
|---|---|---|
| **Network enforcement observation** | The CNI's or VPC's own flow telemetry | That the network blocked (or allowed) a direct connection |
| **Gateway decision** | The AE evidence journal | That a request reached the gateway and was permitted or refused |

**A connection the network blocks never reaches the gateway, so the gateway cannot record it.** Without flow
telemetry, the claim is "direct egress is blocked", with its observability limit stated: blocked attempts are not
observed. NovusLens's `network_flow` acquirer consumes Hubble or VPC flow logs as network enforcement observations,
and raises `undeclared-egress` when a confined workload's observed flows contradict its declared structure.

### 2.4 The node reports what it can see, and nothing more

`GossipAgent::confinement_report()` returns a `ConfinementReport`:
- each node-level setting as `Set`, `Unset` or `NotInBuild`;
- network confinement as **`Unverified`, always**. A node cannot observe its own network policy, and a node that
  reported "confined" would be vouching for what it cannot see.

`unmet()` lists the node settings not yet in place. An empty list means the node settings hold, which is **not**
the same as the fleet being confined.

## 3. Evidence: deployment, not replay

`scripts/test-confined-fleet.sh` brings up a **real** cluster (kind) with **Calico**, a CNI that enforces
NetworkPolicy (kind's default kindnet does not, and a test on it would pass for the wrong reason). It applies
`deploy/confined-fleet/` **unchanged** except for stand-in images, and checks from inside an agent pod:
- the gateway's API port is reachable;
- the gateway's admin port, the Kubernetes API and the internet are **not**;
- no service-account token is mounted.

**Every "not reachable" is paired with a positive control pod that can reach the same destination.** Without that,
a block could just mean the destination was down.

**What the test does not show, stated:**
- **Instance metadata:** kind has none, so the check cannot discriminate. It is reported as `N/A (not
  discriminating)`, not as a pass. Default-deny egress covers it.
- **Recording through the gateway:** the stand-in gateway is not a Mycelium node. Recording is covered by the AE
  seam's own tests. An end-to-end run with a real node image is a follow-up.
- **Other CNIs:** Calico only. Operators on another CNI should run the same script with it; the runbook shows how.

## 4. What this does not claim

- **That the profile is in place anywhere.** It is opt-in. Deployments that do not adopt it keep the residual:
  acts off the substrate are invisible (`coverage.complete: false`).
- **Anything about processes within one pod.** That is out of scope by design (§2.1).
- **That a node knows it is confined.** It never does (§2.4).
- **Anything about what agents do *through* the gateway.** That is authorisation's job (AE0–AE2; A1 for admitted
  work; H3 for capabilities).

## 5. Gates

- `agent::confinement::tests`: a default node reports every setting unmet and the network unverified; configured
  settings are reported, with the network still unverified; settings outside the build are reported as `NotInBuild`.
- The deployment test (CI job *Confined-fleet deployment*, `make test-confined-fleet`): the checks above, each
  blocked path with its positive control.
