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

**Establish mandates at the gateway** (Boundary H A1), so that policy rules requiring a mandate can actually be
satisfied, and satisfied only by current, unrevoked, possessed appointments:

```rust
use mycelium::mandate::authority::{ClockModel, ExecutionGate, FreshnessPolicy, ResourceTier};
use mycelium::mandate::grant::{EntitlementTable, GrantVerifier};
use mycelium::mandate::{PrincipalId, ResourceAuthority};
use mycelium::ExecutionAuthority; // re-exported; tests/gateway_mandate_external.rs keeps it constructible

let mut entitled = EntitlementTable::new();
entitled.entitle("depot", PrincipalId::new("operator:acme").unwrap()); // who may appoint for "depot"
let gate = ExecutionGate::strict(
    ResourceAuthority::new("depot", 1),
    ResourceTier::Serialised,
    ClockModel { skew_ms: 500 },                                        // your time-sync bound s
    FreshnessPolicy { freshness_ms: 120_000, interval_ms: 30_000, delivery_ms: 10_000 }, // F, I, D
)?;                                                                      // refuses F ≤ 4s or I + D > F − 4s
agent.with_execution_authority(Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(entitled), external)));
// Feed the authority's signed revocation checkpoints (at least every I, even when nothing is revoked):
agent.offer_revocation_checkpoint(&checkpoint);
```

Policy rules for protected operations must require the mandate (`Rule::requiring_mandate("depot")`).
`ReferenceEvaluator::allowances_without_mandate(&[("skill.invoke", "skill:depot/dispatch@…")])` lists any
allowance that would not, and it must be empty. Callers present `mandate=` from the SDKs (`A2aClient.send`), with a
possession proof they sign over `mandate_request_bytes(...)`.

**Issue agents the narrowest tokens** (closure plan C1). Use named tokens (`gateway_named_tokens`), never the legacy
`gateway_auth_token`, which holds `*`:

| Agent role | Scopes |
|---|---|
| Calls skills and tools | none on the raw routes: skills go through `/a2a`, tools through `/mcp` (`mcp:invoke`) |
| Serves skills through the gateway | `mesh:serve` only (the serve stream and `rpc/respond`) |
| Emits coordination signals | `mesh:write`, and only if it must |

`mesh:write` opens the raw routes (`rpc/call`, `scatter`, `signal/emit`, …). Those refuse protected kinds
(`mcp.invoke`, `skill.invoke`, `llm.invoke`, and any in `protected_rpc_kinds`) with `403 protected_kind`, so they
cannot carry protected work around `/mcp` and `/a2a`. List every kind your own providers serve as protected work in
`protected_rpc_kinds` (`GOSSIP_PROTECTED_RPC_KINDS`).

**Enforce at the provider too** (closure plan C3). On every node that serves tools or skills, attach the same action
evaluator and execution authority as the gateway, and turn on `agent.with_provider_enforcement()`. Protected work
is then decided where it runs, so a member that reaches a provider without the gateway is refused just as the
gateway would refuse it. Feed revocation checkpoints to every such node (`offer_revocation_checkpoint`).

**Cap the fleet together at each provider** (closure plan C4). `agent.with_cohort_budget(CohortBudget::new(per_cohort,
undeclared), CohortView::trusting([operator]), external)`, then `offer_cohort_declaration` for your fleet's signed
declaration. A colluding population inside every member's own cap is still held to the cohort's cap; strangers
share the undeclared pool. Watch `cohort_budget_refusals()`.

**Keep every node's clock in sync** (closure plan C11). Every expiry and freshness check assumes each node's wall
clock is within *s* of real time: run NTP (or equivalent) and monitor it. The node's report says
`clock_sync: Unverified`, because a node cannot vouch for its own clock.

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
