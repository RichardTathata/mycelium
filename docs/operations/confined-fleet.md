# Running a confined fleet (operator runbook, Boundary H item H7)

For operators admitting **untrusted agents** as Mycelium members: evaluation fleets, third-party agents, anything
whose code you do not vouch for. Design and reasoning: [`design/confined-fleet.md`](../design/confined-fleet.md).
Manifests: [`deploy/confined-fleet/`](../../deploy/confined-fleet/).

**The rule, once:** agent code never holds a member key, and agent pods can reach nothing on the network except their
gateway.

## 1. Before you start: does your CNI enforce NetworkPolicy?

If it does not, the manifests confine **nothing**, silently. Check it the way the deployment test does:

```bash
make test-confined-fleet                         # kind + Calico reference run
# or against your own cluster's CNI, by hand: apply deploy/confined-fleet/ with your images, then from an agent pod
#   curl --connect-timeout 5 https://example.com    # must fail
# and from a pod the agent policies do not select
#   curl --connect-timeout 5 https://example.com    # must succeed (the positive control)
```

A "blocked" result with no positive control proves nothing.

## 2. Deploy

1. Replace `GATEWAY_IMAGE` and `AGENT_IMAGE` in `deploy/confined-fleet/`, and adjust the gateway's ports (`8080` API,
   `9090` admin) to your node's configuration.
2. Create the `member-key` Secret in `confined-fleet` from the gateway's TLS identity. **Never mount it in the agent
   Deployment.**
3. Bind **no** cloud IAM role to the `agents` service account, and create **no** RBAC bindings for it.
4. `kubectl apply -f deploy/confined-fleet/`.

## 3. Configure the gateway node

| Setting | Value | Why |
|---|---|---|
| `egress.allow_hosts` | The hosts the gateway itself must reach, and nothing else | The substrate's own outbound paths fail closed |
| `require_identity_proofs` | `true` | Issuer binding's member path rests on it |
| An audit sink | Attached (`with_audit_sink`) | Keeps original bytes; a reissued audit suffix is detectable |
| An action evaluator **and** an evidence journal | Both attached | Everything an agent does through the gateway is authorised and recorded, fsynced, before dispatch |

Then read what the node itself can confirm:

```rust
let report = agent.confinement_report();
assert!(report.unmet().is_empty(), "node settings missing: {:?}", report.unmet());
// report.network_confinement is always Unverified: the node cannot see its network policy. §1 is your evidence.
```

## 4. Configure readers

Any node that reads knowledge produced by the fleet should set:
- `ReaderPolicy::unchecked = UncheckedRule::Exclude`;
- `undeclared = UndeclaredRule::OneGroup` (or `Excluded`);
- the fleet declared as one cohort (`CohortDeclaration`, signed by you as the operator, from an issuer the readers
  trust).

## 5. Observe what the network blocks

The gateway records what reaches it. It **cannot** record what the network blocked. To see blocked attempts, export
your CNI's flow logs (Cilium Hubble: `hubble observe -o json`; on AWS, VPC Flow Logs) to NovusLens's `network_flow`
acquirer. Blocked flows arrive as **enforcement observations**, and flows that contradict a workload's declared egress
raise `undeclared-egress`. Without flow telemetry, say "direct egress is blocked", and do not say "recorded".

## 6. What this does not give you

- Anything about what agents do **through** the gateway. That is authorisation's job.
- Confinement on a CNI that does not enforce NetworkPolicy (§1).
- A node that knows it is confined. `network_confinement` is always `Unverified`.
