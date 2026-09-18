## [2026-09-18] ingest | item 4 PR 1 — the adaptive-stability ADR

Up: [dev](../dev.md) · record `docs/design/adaptive-stability.md` · plan `docs/plans/v3-contracts-axis.md`
§6.2, D17–D20, D30 · reservations `src/lib.rs` (namespace table), `mycelium-core/src/signal.rs`
(`kv_ns::RIGHTS_HEAD`).

### Why now

Item 4 had no design record. Its one standalone piece (WP5, the cooldown parameter and `staleness_known`)
shipped 2026-09-13, but everything that depends on item 4 — RA1–RA6 in the private lane (D30: *"the ledger's
vocabulary and backend are item 4's decisions, not RA's"*) and item 6 PR 6 (D19: the combined-feedback harness
is replay stage 6) — was waiting on a record that did not exist. This is that record.

### The three things worth carrying forward

1. **A hard bound is hard only when it is local and durable.** The membership governor already says *"bounds
   are convergence targets, not guarantees."* The ADR generalises that: a fleet ceiling is `HardPrevention`
   only with exclusive, durably accounted rights, and the place a bound is actually hard is a node-local ledger
   — not a global agreement. Exclusivity is by allocation, not by consensus.
2. **Rights cannot live in gossip KV.** Every governor input today is evaporating soft state, which is right
   for an advertisement and wrong for a right: an allocation that vanished with its holder's discovery entry is
   an allocation issued twice, and an LWW medium lets a later writer overwrite one. Same problem item 3 met with
   records; same answer — heads in the medium, the thing itself in a store. The `EvidenceJournal` (node-local,
   fsynced, never gossiped) is the backend's shape.
3. **Uncertainty holds speculation and routine scale-down; it never holds protection or rescue.** Four action
   classes, and the asymmetry is the shape of the costs: holding a protective shed on uncertainty is a node
   with a full channel waiting for peers it cannot hear to permit it to protect itself. PR 2 makes the
   predicate pure and sweeps it — the partition-table shape.

### What is deliberately not decided here

The `ControlSpec` fields' exact types (PR 2); the head's signature scheme beyond "bounded and signed" (PR 3
reuses `tls::verify_bytes`, as item 3 did); whether the harness shares the kernel or wraps it — that is item 6
PR 6's question and the inventory's §7 leaves it open on purpose.

### Pages touched

- [history.md](../history.md) — a ledger section.
- The ADR is canon; the wiki cites it. No architecture page lists the v3 modules yet (noted in the coverage
  ingest of the same day) — a lint-pass question, not this entry's.
