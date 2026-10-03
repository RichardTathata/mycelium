## [2026-10-03] ingest | a poll that can miss a transient state is not a wait

**What:** `the_stem_fleet_fills_a_presence_floor_and_reheals` failed 2 of 4 full host-suite runs today at
"the dead host's advertisement evaporates" — a 60 s `wait_until(.., || live(&seed) < 2)` polling every
200 ms. The advertisement's TTL is 5 s and the standby re-provisions within a 300 ms tick of its expiry,
so `live < 2` can hold for less than one poll interval; the count-based wait could miss the dip entirely
while the re-heal itself worked. Now the waits are structural: the victim's node id is absent from the
seed's providers (a state that never reverts), then two providers neither of which is the victim.

**Durable knowledge:** the testing page's rule "structural polls, never fixed sleeps" has a corollary — a
poll on a **transient** condition is a fixed sleep in disguise: it passes when the window happens to
straddle a tick. Wait on conditions that, once true, stay true (an id gone, a count reached), and put the
transient fact in the assertion message, not the wait.
