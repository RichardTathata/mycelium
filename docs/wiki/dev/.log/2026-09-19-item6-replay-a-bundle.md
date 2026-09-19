# 2026-09-19 — item 6's decisive demonstration: replay a bundle

**What shipped.** `mycelium-sim/examples/replay_a_bundle.rs`, run in CI; gallery row; plan §12.1 row
for item 6 marked delivered.

**The design question, and why the obvious answer was wrong.** §12.1 asks for *"the WAL/snapshot race
captured as a bundle, replayed to the failing witness, then the fix replayed green"*. A binary cannot
do that. The corpus bundle's scenario lives in a `cfg(test)` helper, and — more to the point —
reproducing that failure means setting `persistence::WITNESS_SKIP_WAL_MERGE`, which **removes the
WAL-tail merge**. Exposing it outside tests would put a data-loss switch in a shipped binary. That is
a worse thing to own than the demonstration is a good one, so the example owns *its own* scenario and
*its own* injectable bug, and says plainly where the real one is replayed instead.

**What it demonstrates.** A depot's surplus-food sweep — read the clock, draw a jitter, journal a
verdict per offer — recorded through the kernel, written as a bundle, read back, replayed clean. Then
the same bundle against a version that drops the grace window a depot allows a late van: the kernel
**diverges at seq 4** and prints the recorded effect beside the replayed one. Then the fix, green.

**The second overclaim this session, caught the same way.** The first draft said "the offer whose
verdict flipped is named in the two lines above". It is not: the trace records a **sha256 digest** of
the payload, not the payload, so the two lines show a differing digest and length and nothing else.
That turned into the better paragraph — a trace is a log of decisions, not a copy of the data, which
is what keeps a bundle small and keeps payloads out of an artefact that gets attached to bug reports.
The example now says *the trace localises; the code names*, and prints the flipped verdict from its
own data.

**The distinction worth keeping.** A bug whose effects reach a seam is caught as a **divergence**, and
the bundle localises it to one effect. A bug whose effects never reach a seam — a wrong value
computed and returned, never written, never timed — replays *perfectly* and is caught by the bundle's
**witness** assertion instead. That is why a bundle carries a witness, and why one with an unknown
witness cannot prove its own fix. The real WAL/snapshot race is the second shape; this example is the
first, and it says so.

**Still owed by §12.1:** item 4's control-envelope viz, item 5's curator handover.
