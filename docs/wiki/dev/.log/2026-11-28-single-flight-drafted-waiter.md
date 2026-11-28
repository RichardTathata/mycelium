## [2026-11-28] ingest | single-flight: a ticketed holder hands the run on once answered (the drafted waiter)

This is the mirror of #822's livelock. A waiter (`refresh_answers`) that found no fold running became the holder, and a
holder runs while `again` is set. Under a caller asking every few µs it never left, though its own ticket was answered. It
hung `tickets_are_answered_under_a_busy_fold_and_changing_roots` about one run in eleven on CI, and it held `revoke_key`
and `change_roots` in the product. The fix is `SingleFlight::run_for(ticket, pass)`, which both folds' `refresh_answers`
now use. Once the ticket is answered the holder offers the run (`offer`: `OFFERED`); the next asker accepts it (`ACCEPTED`)
and is handed it (`HANDED`) without `running` being released. Plain `run` callers keep going while asked, as before.

Two alternatives were rejected. A release-and-retake loses the race to the asker almost never, so the waiter stays
drafted. A hand-off to the identity watcher fails because unstarted and shut-down agents have no watcher.

Pages: `dev/concurrency/lock-free-and-atomics.md` (the rule, the guarantees, the gates). No lock-order row: the change is
atomics only. Also touched: `CHANGELOG.md` (Unreleased › Fixed) and `docs/plans/post-360-hardening.md` (a note at the
test's line).

**After the review (same day).** The model check found no protocol bug, but it did find a hang. A call into the same
fold from inside a pass, on the holding thread, would accept its own thread's offer and wait for ever, and this was
true of `run` as well as `run_for`. A thread-local record of the runs each thread holds now turns that call into a
request. The extra-pass window is now stated precisely (between the asker's `running` swap and its accept CAS), and the
CHANGELOG states the plain caller's new wait.
