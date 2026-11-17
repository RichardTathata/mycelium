## [2026-11-17] ingest | three flakes on main, CI run 44085801848: a fuel budget read as a time, a log read before it was emptied, a version that was a constant

All three failed on #808, which changes CI configuration and documents only.

**The row D heartbeat test** (`a_long_guest_call_does_not_block_another_task_on_a_current_thread_runtime`,
`mycelium-wasm-host/src/provisioner.rs`) made its guest burn 3 000 000 000 units of fuel and asserted the call took at
least 300 ms. A runner burned it in 298.69 ms, twice. The guest now runs until a 600 ms call deadline stops it, which
cannot come early (`EPOCH_TICK` in `host.rs`: `ceil(d / tick) + 1` epochs, a tick at least 10 ms). The assertion is
unchanged.

**The two-domains scaffold step** exited 1 printing nothing, with `data/kitchen-gw.log` empty in the trap's output.
`fleet.sh start` launched the node with `> data/$n.log &` and then polled the log with two reads, `grep -q … && { grep
…; exit 0; }`. The redirection belongs to the background shell, so on a restart the previous run's log is still there
until that shell runs: the first read matched the old `up as` line, the launch emptied the file, the second read
found nothing, and `set -e` ended the script. The other outcome of the same race returns 0 on the old line before the
node is up. `start` now empties the log itself, the node appends, and the loop reads once. The template shipped in
v2.40.0 has the defect; the README's own `stop` then `start-all` reaches it.

**The history index's version** (`src/agent/evidence_journal.rs`, `HistoryIndex::view`) was a constant 0 whenever no
history could be given — the journal still being read, or unreadable. A permit is admitted only at its actor's
version, so once an actor had anything recorded, each later permit failed `admit` three times and the door refused it
as *the actor's history kept changing*. `the_shadow_profile_counts_and_holds_nothing` makes two calls straight after
start and failed whenever the read outlasted them. A product defect, fixed in `view`; a 3 s sleep planted in `load`
reproduced the CI failure at the same line, and run over the whole lib suite with the fix it found one more test that
decided on a limit before the read had finished (`at_most_three_posts_in_ten_minutes_counted_at_this_gateway`), which
now waits for the read.

**Durable knowledge**, on the [testing page](../testing.md) § The CI flake tier: a fuel budget is not a duration; a
script that restarts a process onto the same log must empty the log before it launches; and a planted delay run over
the whole suite is the enumeration of who depends on the thing being fast.
