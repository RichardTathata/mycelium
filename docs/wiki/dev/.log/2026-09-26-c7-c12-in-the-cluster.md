## [2026-09-26] ingest | closure plan C7 and C12, in the cluster

Up: [dev](../dev.md) · page: [security](../security.md) · record `docs/plans/boundary-h-closure.md` C7 and C12 · harness
`scripts/test-confined-fleet.sh` (phase 2) · image `docker/Dockerfile.confined-fleet` · binary
`examples/confined_fleet_node.rs`.

The bypass matrix (C7) and the stop measurement (C12) had each run in one process. Both said the deployment
variant waited on a node image built in the confined-fleet job. That image now exists, and the job's second phase
uses it.

Phase 2 runs one binary in three roles. The **authority** signs the grant and issues a checkpoint every second. A
**provider** member serves the tools and the skill with enforcement on. The reference **gateway** deployment is
patched to the same image. The agent pod reaches only the gateway, and runs the agent's side from there:
- **C7.** With a valid mandate, `/mcp` and `/a2a` (send and stream) reach the provider's handlers, which is the
  plant. After the authority revokes, none does, and the raw routes refuse protected kinds. The member plane (the
  provider's gossip port) is unreachable from the agent pod, while the control pod can reach it.
- **C12.** Calls to a long-running tool are admitted, the authority revokes, and the provider's own records must
  show T_admit and T_drain within the class's bound plus the checkpoint delivery allowance.

Stated limits:
- the pods share one kind node, and so one clock, so *s* is declared rather than exercised;
- members poll the authority for checkpoints, so delivery is part of the measured stop.

The network policy gained one rule: members may reach the gateway's gossip port. A gateway in a real fleet peers
with its providers, so the reference manifests needed it anyway.
