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
- **[hooks.md](hooks.md)** — the cross-layer hooks (`ReplyInterceptor`, `QuorumObserver`,
  `SnapshotDeferHook`, the decision sink, KV change notifications): what each is handed, what it may
  do, and the rule they all keep — no synchronous network I/O, nothing that blocks.
- **Configuration ownership** (code-canon reference, not a wiki page) —
  [`docs/reference/configuration.md`](../../../reference/configuration.md): every `GossipConfig` field's
  component, the feature that enforces it, where a bad value is refused, and whether it needs a restart
  (only five fields change at runtime, `HotConfig`). The configuration split is deferred behind it
  (realignment repairs A2, decision D3).
- **[runtime-invariants.md](runtime-invariants.md)** — the invariants that keep recurring
  in review: Layer III's detection-not-prevention posture, individual-scope routing,
  event-driven fan-out. Read before "optimizing" anything in the gossip loop.

Canon: `src/lib.rs` crate doc (API + KV-namespace ownership table), `ROADMAP.md` (layer
model + milestones), `docs/philosophy.md` (purpose).
