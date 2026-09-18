## [2026-09-18] ingest | item 6 PR 6 — replay scenario C, the interacting governors

Up: [dev](../dev.md) · records `docs/design/adaptive-stability.md` §5 (D19), `docs/design/replay-nondeterminism-inventory.md`
· code `src/control/scenario_c.rs` (test-only), `src/agent/{opacity,tuning_governor,membership_governor}` made
crate-visible for it.

### What landed

The combined-feedback harness the ADR promised as *replay stage 6, built once, reusing the governors' pure
decision functions*: a schedule sweep (48 schedules × 2 profiles) over the shipped decisions — `gate_at`/`acted_at`,
`opacity_state_for → opacity_transition → spaced_transition`, `decide`/`classify` under `control::decide` with
spacing and settling — coupled through a small plant. With the loop-breakers on, every run settles; with them off,
schedules flap and chatter; the decisive rule holds under `EnforceLocal` and not under `Legacy`; the sweep asserts
its own size.

### Three things worth carrying forward

1. **State the objective on the thing the breaker governs.** The first sweep said *two boundary transitions never
   closer than 300 ms* and failed under the shipped breakers — a release followed 100 ms later by a re-shed. That
   is the decisive rule working (the shed is never held), not a flap. The objective is on **releases**; what the
   release spacing bounds is how often the release's cost is paid.
2. **A first-only violation report lets one kind mask another.** The witness reported "no flap" while the same
   schedules chattered, because the checker returned after the first violation. `violations` collects all; the
   witness asserts both spacing objectives did work, not one.
3. **A plant that is not caught is a finding, not a failure to plant.** Removing the hysteresis in shipped code did
   not fail the breakers-on sweep: the 1 s release spacing alone bounds the release rate, so a lost hysteresis
   changes only how often a proposal is *held* — which is no stability objective. Recorded as what the sweep does
   not prove rather than papered over with an objective invented to make the plant fail. The tuning plant (no
   spacing) was caught at once: knob chatter at 100 ms.

### What is modelled, and what is not shown

The plant: this node's share of a fleet inbound, halved while opaque, minus what the writer drains; an advisor that
recommends a writer depth from the group size *and* the load (the load term is what makes tuning and opacity share
an input; its quantization is what an unspaced knob chatters on). The ADR's sharper claim — *several loops can
oscillate together while each is stable alone* — is **not** demonstrated: the witness removes every breaker at
once. No real node is involved; the loops' wiring is outside the sweep, which is why it is deterministic.

### Pages touched

- [testing/testing.md](../testing/testing.md) — *Replay scenario C*.
- [history.md](../history.md) — the item 6 PR 6 section.
- The ADR's §5 carries a dated note; CHANGELOG.
