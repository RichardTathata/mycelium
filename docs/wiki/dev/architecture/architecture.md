# dev/architecture — the three layers and their invariants

↑ [dev/](../dev.md)

The substrate is three layers on one gossip KV; the crate boundary makes the layer
inversion a compile-time guarantee. Pages:

- **[layers-and-crates.md](layers-and-crates.md)** — the layer model, the `mycelium-core`
  split, `CoreCtx`/`TaskCtx`, the Layer I/II bridge, the core design rules.
- **[contracts.md](contracts.md)** — **what an acknowledgement proves**: the four receipt rungs and
  the rule that nothing infers a higher one from a lower, `LocalDurability`'s four states, why a
  timeout is `DeliveryUnknown` rather than a failure, and the regression floor — *a PR changes an
  ack's meaning by changing a pin, in the open*.
- **[runtime-invariants.md](runtime-invariants.md)** — the invariants that keep recurring
  in review: Layer III's detection-not-prevention posture, individual-scope routing,
  event-driven fan-out. Read before "optimizing" anything in the gossip loop.

Canon: `src/lib.rs` crate doc (API + KV-namespace ownership table), `ROADMAP.md` (layer
model + milestones), `docs/philosophy.md` (purpose).
