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
reproduced the CI failure at the same line. Run over the lib suite with the fix, the plant found **two** more tests:
- `at_most_three_posts_in_ten_minutes_counted_at_this_gateway` decided on a limit before the read had finished. It
  failed every time under the plant, and now waits for the read.
- `a_forged_time_in_a_live_members_entry_is_scrubbed_by_its_owner` failed some of the time (the author's one run of
  the suite passed; #811's reviewer saw it fail in the suite and in 2 of 6 runs alone, 0 of 12 without the plant). It
  polled every 25 ms for the forged entry *at its owner*, a state that lasts only until the owner's next 500 ms tick;
  the slow read moved the test against the tick. With the poll slowed to 600 ms it failed every time, which is the
  mechanism. It now waits for a state that stays: the forger's own marked time back at the forger without the forged
  one, which only the owner's rewrite of the forged entry produces. This is the
  [transient-dip rule](2026-10-03-fleet-test-transient-dip.md) again.

One run of a plant is an enumeration only of what fails every time under it. The first log of this work said the plant
found one more test; it had been run once.

The fix removed what had been covering for an unreadable journal: such a node used to refuse nearly everything. It
now logs one warning and shows `evidence_history_unreadable` on `/stats` (`deprecations.md` §119).

**Durable knowledge**, on the [testing page](../testing.md) § The CI flake tier: a fuel budget is not a duration; a
script that restarts a process onto the same log must empty the log before it launches; and a planted delay run over
the whole suite is the enumeration of who depends on the thing being fast.
