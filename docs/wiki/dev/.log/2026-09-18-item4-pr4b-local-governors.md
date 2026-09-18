## [2026-09-18] ingest | item 4 PR 4b — the two local-input governors through the contract

Up: [dev](../dev.md) · record `docs/design/adaptive-stability.md` §9 row 4b · code
`src/agent/tuning_governor.rs` (`gate_at`, `acted_at`, `set_control_timing`, the snapshot counters),
`src/agent/cluster_tuner.rs` (`acted` after apply; timing from the interval), `src/agent/opacity.rs`
(`spaced_transition`), `mycelium-core/src/signal.rs` (`OpacityHint.release_spacing_ms`).

### What landed

Spacing and settling for the two governors whose view is this node's own — no confidence predicate, because
a knob's readback and a handler's fill ratio are never uncertain. The membership governor (4a) needed the
predicate; these two needed the clocks.

### Three things worth carrying forward

1. **Reconcile means readback, and the tuner already had it.** The ADR's *reconcile* step — "observes the
   effect and settles the reservation" — sounded like new plumbing until the call site was read: `gate(param,
   rec, cur)` already receives the knob's current value on every tick. So *pending until seen at the knob*
   costs one comparison, and *unknown past the timeout* is the honest name for a knob that did not take the
   value (or a caller that applied and did not say so). What it does **not** observe is the throughput
   effect of the change; spacing is what gives that time.
2. **Separate the act from the gate.** A first draft recorded the action inside `gate`. But the tuner runs a
   `ConfigPolicy` *after* the gate, and a rejected value would have consumed spacing and left a phantom to
   settle as unknown on every tick. `acted` is its own call, made only on apply — the same reason the
   rights ledger's persist-then-apply is two steps and not one.
3. **Spacing applies to the release, not the shed.** The obvious "space every transition" would have held
   `GoOpaque` after a recent release — the exact protective action §5's decisive rule says is never held,
   and the full-channel override would have had to fight the spacing. Only `GoTransparent` is spaced; it
   is also the one that flaps. The pure test pins the shed proceeding at 1 ms after a release.

### Defaults, and what they change

The tuning timings are `0` unless `start_cluster_tuner` sets them (two ticks each) — the applier alone is the
old gate. `release_spacing_ms` defaults to 1 s, which *does* change a default: a boundary that went opaque
releases no sooner than a second after its last transition. The hysteresis (fill a full 0.20 below the
effective threshold) already makes a release within a second rare; the spacing bounds it outright.

### Not shown

The tuner loop end to end under spacing (the unit tests pin `gate_at`/`acted_at`; the loop is wiring), and
the combined behaviour of the three governors — item 6 PR 6's harness, by the ADR's own D19.

### Pages touched

- [history.md](../history.md) — the item 4 PR 4b section.
- The ADR's §9 marks 4b landed with its decisions; CHANGELOG carries two upgrade notes (snapshot fields,
  `OpacityHint` field).
