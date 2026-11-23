## [2026-11-22] ingest | B1 PR 1: the control lane's mechanism

A second connection per peer for the closed set `CONTROL_KINDS` (`mycelium-core/src/control_lane.rs`, design
`docs/design/control-lane.md`). Durable knowledge: the lane is an **extra copy** of a frame on the shared path (same
encoded bytes, same nonce, the seen-set dedups), so Individual-scope forwarding stays unconditional and the lane is not
a carve-out; a relay never puts a received frame on a lane; the lane's M1 rules (open only beside a connected data
writer, yield within 1 s, 300 s after a refused connect) keep it out of the per-source slot a data reconnect needs;
every site that stops writing to a peer closes both connections (design §4.3); a control kind's quorum-evidence
append leaves the connection reader. No new lock.

Pages touched: `architecture/runtime-invariants.md` (the lane is an extra copy, not a carve-out). Outside the wiki:
`docs/design/replay-nondeterminism-inventory.md` (the lane's streams), `docs/features/signal-mesh.md`,
`docs/guide/deprecations.md` §126, `docs/reference/configuration.md`, `docs/operations/metrics.md`.
