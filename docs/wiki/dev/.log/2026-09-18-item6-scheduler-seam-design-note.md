## [2026-09-18] ingest | item 6 — the scheduler seam sized: a design note, not a design

Up: [dev](../dev.md) · record `docs/design/replay-nondeterminism-inventory.md` §3 (the kernel row corrected), §3.1
(the note) · the pinned gap `mycelium-commitment`'s `a_whole_node_recording_of_a_linearizable_award_diverges_without_a_scheduler_seam`.

### What landed

A correction and a sizing. The coverage map claimed the kernel owns `select!` readiness; no site is routed, and
CN2 measured the consequence — a whole node's replay diverges on the interleaving of two routed tasks. The note
says what a `select!` wrapper could do (detect an interleaving change), what it cannot (reproduce one — the runtime
decides which task asks next, and no supplied result changes that), and what reproduction needs: the seams' replay
arm advancing tokio's **paused** clock by the recorded wait instead of yielding — so every task keeps its relative
wait on a `current_thread` runtime — and, for any node with peers, a network seam that is a record of its own.

### Two things worth carrying forward

1. **Richer traces are not more reproduction.** Recording the winning branch at every `select!` was the obvious
   next step and would have made every whole-node replay a divergence report. The lever is the *replay arm's
   handling of time*, which is already the kernel's, not a wrapper at 51 sites.
2. **A pinned gap is a gate for the design that closes it.** The first arm is judged by whether CN2's pin flips
   — and if it does not, the divergence that remains names the next seam. The order of work is decided by a
   test that exists, not by a plan of tests to write.

### Not done

The first arm itself (a change inside `sim_seam`'s `sleep_ms`/`tick` replay arms under a paused clock); the
network seam; the `select!` wrapper that would name the branch in divergence reports afterwards.

### Pages touched

- [history.md](../history.md) — a line beside item 6.
- The inventory's §3 row and §3.1.
