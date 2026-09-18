# mycelium-commitment — the contract net, as five records

The third of Mycelium's three coordination models, beside the tuple space's competitive `take`
(`mycelium-tuple-space`) and the blackboard's shared facts (`mycelium-blackboard`): **commitment** —
declare, offer, accept, fulfil, assess. A composition, not a subsystem (`docs/plans/v3-contracts-axis.md`
§6.9, §13.2): each record has a mechanism the substrate already has, and there is no planner and no
component that assigns another participant's obligation.

| Record | Mechanism | Where |
|---|---|---|
| announce | KV head, declarer-owned | `cn/{requirement}` |
| offer | `append` | `log/cn/{requirement}/offers` |
| award | lowest-participant rule; written with `set_with_receipt` | `cn/{requirement}/award` |
| report | `append`; outcome or *unknown*, never silence | `log/cn/{requirement}/reports` |
| assess | `append`, Ed25519-signed; the declarer is not the only assessor | `log/cn/{requirement}/assessments` |

**Rules.** One award per requirement — a second is *refused*, never written over the first. A requirement
with no offers is a visible state, not a retry loop. An awardee that vanishes leaves an award with no
report, reported as such; re-announcement is the declarer's decision, never automatic reassignment. An
unsigned assessment is unproven: `verify_assessment` says `false` for it.

Built entirely on Mycelium's public API (`default-features = false`).

## Run the example

```bash
cargo run -p mycelium-commitment --example redistribution_cn
```

The surplus-food redistribution workload the tuple-space and blackboard examples also run: a hub
announces six pickups, three drivers offer on what they can reach (one pickup contested), each pickup
is awarded once with a receipt, a second award is refused, every awardee reports against its award's
operation, and a food-bank auditor — not the hub — signs the assessments. Exits 0 with
`All assertions passed`.

## What it does not do (yet)

CN2 replays an award under `mycelium-sim` with a double-award witness; CN3 checks an award against the
acceptor's mandate epoch (`docs/design/scoped-mandates.md`) where one exists. Participants are named,
not authenticated, here: authority lives in the gateway's caller context and in mandates, not in this
crate.
