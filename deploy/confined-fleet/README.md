# Confined-fleet reference deployment (Boundary H item H7)

The reference deployment for running **untrusted agents** as Mycelium members. Design and reasoning:
[`docs/design/confined-fleet.md`](../../docs/design/confined-fleet.md). Operator runbook:
[`docs/operations/confined-fleet.md`](../../docs/operations/confined-fleet.md).

**The rule:** agent code never holds a member key, and agent pods can reach nothing on the network except their
gateway. Agents and gateways therefore run in **separate pods**. Containers in one pod share a network namespace, so an
in-pod sidecar cannot give the agent one egress policy and the gateway another.

| File | What it is |
|---|---|
| `namespace.yaml` | The fleet's namespace |
| `gateway.yaml` | The gateway (the Mycelium member): Deployment and Service. The member key is mounted **only here** |
| `agents.yaml` | The agent workload: no service-account token, no key, no cloud identity |
| `network-policy.yaml` | Default-deny for agent pods; egress only to the gateway's API port and cluster DNS; the gateway admin port is closed to agents; instance metadata excluded |

Placeholders to replace: `GATEWAY_IMAGE`, `AGENT_IMAGE`, and the gateway's ports (`8080` API, `9090` admin/metrics).
**The CNI must enforce NetworkPolicy.** On a CNI that does not, these manifests confine nothing, and the runbook says
how to check. The deployment test (`scripts/test-confined-fleet.sh`) applies these files unchanged, with stand-in
images substituted, on a kind cluster running Calico.
