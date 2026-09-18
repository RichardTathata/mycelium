## [2026-09-18] ingest | item 6 PR 3 tail — the timer seam, and a boundary of exact replay

Up: [dev](../dev.md) · record `docs/design/replay-nondeterminism-inventory.md` §2.3 · code
`mycelium-core/src/sim_seam.rs` (`sleep_ms`), `mycelium-sim/src/seams.rs` (`Seams::timer`,
`Sources::advance_ms`), `src/agent/consensus_handle.rs` (six sleeps routed).

### What was asked for

§2.3's third row: *fixed sleeps inside protocol logic* — "the 1 s 'let the winning commit converge' after
`distributed_lock`'s commit is the one whose **duration is a correctness assumption**" — owned by a timer seam,
"each listed in the coverage map as a schedule the kernel must explore (0, exact, and > the sleep)".

### What landed

`sleep_ms(stream, ms)`: production-identical without `sim`; under a kernel, `Record` sleeps and records that the
wait elapsed, **`Replay` never wall-waits** — the recorded effective duration advances both simulated clocks
(`Sources::advance_ms`, both clocks, unlike a wall *jump*) and the task yields once. The two converge sleeps are
`lock/converge` and `elect/converge`; the ballot defers `consensus/defer` and `consensus/suggest-defer`.
`consensus_handle.rs` in the seams baseline: 9 → 3.

### The thing worth carrying forward: exact replay reproduces; it cannot explore

I wrote the exploration claim first — *one edited trace line runs the same code at 0, exact or beyond* — and a
test to match. It failed. In exact replay the **clock reads are replayed too**: an authored timer result is
honoured by the timer, and then the next recorded read says what the recording said. So an authored result is
the *hook* for exploring, not the exploration; re-deriving the reads after an authored wait is **scenario
replay**, the plan's third mode, and the kernel has two. The seam, the kernel and the inventory now say exactly
that, and the test pins both halves so the assertion changes deliberately when scenario replay lands.

This is the same shape as the D4 audit's limitation ("a model, not the live path") and item 2's ("scaffolding,
not a transport"): the harness can now *record* the converge wait, which is what makes the audit's model
replaceable by a replay — but *exploring* schedules is a mode that does not exist yet, and nothing here should
be read as though it does.

### Pages touched

- [history.md](../history.md) → the item 6 PR 3 section gains the timer seam.
- The inventory row is canon and was corrected in the same PR.
