# 2026-09-19 — item 6: the scheduler seam's first arm, and a whole node that replays

**What shipped.** `mycelium_core::sim_seam::pause_clock_for_replay` / `resume_clock_after_replay`; the replay
arms of `sleep_ms` and `Ticker::tick` routed through one `replay_timer` helper; `tokio/test-util` added to the
`sim` feature; two unit tests at the seam; `mycelium-commitment`'s pin flipped into the claim. Docs: inventory
§3.1.1 and the coverage-map row, testing, changelog, plan (item 6's sequence, CN2 now ✓).

**The design note was right about the mechanism and wrong about the blocker.** §3.1 predicted that taking each
recorded wait on a paused clock would restore the recorded interleaving. Built exactly that, and the whole-node
pin **did not flip** — the divergence stayed at seq 7, where the recording holds a governor's `rng jitter` draw
and the replay holds the round's `consensus/defer` timer.

**What was actually wrong.** `Record` wrote its trace entry *after* the wait; `Replay` checked its request
*before* it. So a recording's request order was the order waits **completed**, and a replay's was the order
tasks **entered** them. Those agree only when waits do not overlap — and overlapping waits are the only thing
the arm is for. Moving the replay's check to after the wait, so both modes check in at the same point, flipped
the pin immediately.

**How it was found, and why the small test earned its place.** The whole-node run says only *seq 7 differs*. A
two-task unit test — 50 ms spawned *before* 10 ms, so spawn order and completion order disagree — says which
order the replay produced, and made the asymmetry obvious in one reading. Both tests are kept: the unit one
because it localises, the whole-node one because it is the claim anyone actually cares about. The unit test's
plant (`without_the_paused_clock_the_same_two_waits_diverge`) is what stops the armed test passing on a replay
that had merely kept spawn order and happened to agree.

**A design consequence worth carrying forward.** The replayed wait is the caller's **nominal** duration, not the
kernel's effective one, because the effective value is only knowable by consuming the trace entry — which is the
check itself. In an exact replay the code asks for what it asked for, so ordering is reproduced; a kernel that
rewrites a duration is authoring a different schedule, which is scenario replay and a different mode. Stated in
the seam's rustdoc so the next arm does not re-derive it.

**Pairing, not two knobs.** `pause_clock_for_replay` does both the pause and the arming, because either alone is
worse than neither: pausing without arming collapses every wait to one instant, and arming without pausing makes
a replay sleep for real. `resume_clock_after_replay` exists for exactly one situation — a test that replays the
same trace twice on one runtime, armed then unarmed, so the plant starts from the recording's clock discipline.
`take()` disarms the replay arms but deliberately does not resume the clock, which belongs to the runtime rather
than to the thread's kernel.

**What is now owned.** Task interleaving on one node, on a `current_thread` runtime, where the order comes from
seamed waits. **Not owned, unchanged:** a multi-threaded runtime; real peers (the network seam is still a record
of its own); and two tasks both runnable at the same instant with no wait between them — a paused clock orders
waits, it does not choose between ready tasks. The `select!` wrapper is now worth building, for divergence
reports that name the branch; before this it could only have named what could not be reproduced.

**Gates.** 231 core-with-`sim` / 192 core / 6 sim / 7 commitment, and the whole-node claim run five times for
stability before it was believed.
