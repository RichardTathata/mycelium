## [2026-11-21] ingest | A fold's waiter livelocked against the fold it was waiting on

The kept-anchor budget test (`an_often_rotated_peer_does_not_exhaust_the_kept_anchor_budget`) hung CI's Test job for
six hours, twice. Cause: the identity fold is single-flight on `running` / `again`, and a caller that needed a fold of
its own (`change_roots`, the succession tests' `refold`) retried until it ran one itself. While the identity watcher's
fold ran, each retry set `again`, so that fold re-ran for ever and the waiter never got `running`. Shown by a plant
that slows the fold (old waiter: 382 folds in 10 s, neither thread returned); the full test did not hang locally in the
runs made, so no thread dump of the CI shape exists.

Fix: tickets. `revocation::fold_ticket` before asking; `refresh_answers` is true once a fold that read the ticket at its
start ran to the end, on any thread. The succession tests' fold and durability waits are bounded at 30 s.

Pages: `concurrency/lock-free-and-atomics.md` (the waiter rule, beside the papaya rules); the plan's §3.6 entry
(`docs/plans/post-360-hardening.md`, *Known hang, closed (2026-11-21)*) holds the evidence and the siblings checked.

#822's review added three of the same family, fixed in the PR. A cut fold answering a ticket had no test (two tests
added). A lost request, released in 2.40.0, in both folds, because the request flag was set after the run was tried:
both folds now use `src/agent/single_flight.rs` (ask then try, release then look, `SeqCst`, a guard on `running`).
`change_roots`' client-mark promise now waits on a client ticket. Tests that read after one `refresh` now wait on a
ticket. `Compaction::spawned` gained a drop guard.
