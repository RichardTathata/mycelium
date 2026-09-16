## [2026-09-16] ingest | item 6 PR 2 — the replay kernel that detects divergence

Up: [dev](../dev.md) · plan `docs/plans/v3-contracts-axis.md` §4 · schema
`docs/design/replay-nondeterminism-inventory.md` §5 · code `mycelium-sim/`.

**What PR 1 bought.** The inventory fixed the trace schema *before* any harness existed, and that
turned out to be the whole value: PR 2 implemented a schema rather than inventing one, and the
schema already contained the decision that matters.

**The decision that matters.** A request is *a canonical digest of everything the effect depends on
— never a length alone*. For a write: the content hash of the bytes, the target, the offset, and the
flags that change the write's meaning. Two WAL records of equal length are a different write, and a
trace recording `len=214` would accept changed content as a faithful replay. PR 2's gate is exactly
that test — record a run, replay it with one record's bytes changed at equal length, require a
divergence — and the inventory says why: *a harness that passes that unchanged is not detecting
divergence, only re-seeding.*

**Replay does not run production.** In replay the kernel checks the request against the recorded
entry and then **supplies** the recorded result; the `produce` closure is never called. Calling it
and comparing afterwards would re-run the very nondeterminism the trace exists to remove. The
asymmetry is the design, and there is a test asserting production does not run.

**Divergence carries both sides.** The useful question is never *did it diverge* but *what changed*,
and an error printing only the expectation sends the reader to find the other half. A divergence also
names **where**: three decisions replay cleanly before the fourth departs, which is what tells a
reviewer which change did it.

**Two clocks, not one.** Wall time jumps — an operator corrects it, NTP steps it, a container resumes
— and a lease treating a jump as elapsed time expires early. Monotonic time does not jump and cannot
be compared across processes. A harness recording them as one `time` seam could not replay the jump
that exposes the bug. The `Sources` can step wall time **backwards**, which is the case that matters
and is unreachable without it.

**Named RNG streams, for scenario replay's sake.** Two subsystems drawing from one generator are
coupled: adding a draw in gossip shifts every later value in consensus, and a replay of changed code
then diverges *everywhere* instead of at the change. Streams keep a draw local to what drew it.

**Three outcomes for a write, not two.** `Ok`, `Err`, and **`Short`** — a partial completion is
neither, and the durability argument turns on it.

**The witness is the part people skip.** A bundle that replays but cannot *fail* reproduces a run in
which everything went fine. The witness names the assertion **and the `cfg(test)` toggle** that makes
it fail again — because the 2026-09-05 snapshot fix was verified by hand-disabling the merge, and a
fix whose proof is a manual edit cannot be re-checked.

**Gated from the first commit.** `make check`, `make check-full` and CI all run the crate. A crate
with no gate is the *a job nobody runs never goes red* family, which this repository paid for once
already today (#222/#223); adding the gate with the crate costs nothing and adding it later costs a
green build that meant nothing.

**Scope, stated.** PR 2 is the kernel, the seams, record/replay and the bundle. **Not** here: the
storage and channel *adapters* that route production's own `tokio::fs` and channels through these
seams, and the static forbidden-call check that stops new nondeterminism bypassing them — both PR 3.
Until those land, a call that does not come through a `Seams` is invisible to the kernel, and the
crate docs say so rather than letting the harness look more complete than it is. Scenario replay
(keep `input`/`fault`, re-derive the rest) needs the adapters to re-derive against, so it follows
them.

**Gates.** 30 tests (24 unit + 6 gate), `make check` clean.
