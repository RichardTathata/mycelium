## [2026-10-03] ingest | a fleet test with self-election at p = 1.0 is a herd, not a fleet

**What:** `the_stem_fleet_fills_a_presence_floor_and_reheals` failed again under the full host suite — at a
different wait than the morning's fix ("exactly two stems host"). Three stems started together with
`self_elect_p: 1.0` over a band of `min = max = 2` all elect in one round (three hosts), then all three shed
at once, and the band oscillates until tick interleavings break the symmetry. The test now starts the two
hosts one after the other and only then the standby, which sees the floor met and never installs; the
kill-and-reheal half is unchanged. 4 s instead of 16–18 s, 3/3 alone and 2/2 full suites.

**Durable knowledge:** `self_elect_p = 1.0` turns herd damping off, which is exactly what a *fleet* test
must not do when it asserts a count — the count is then a function of tick interleavings. Either keep
`p < 1` and wait for convergence, or make the symmetry impossible by construction (start order), which
is what makes a count assertion deterministic. The morning's structural-wait fix was right about its
wait and wrong about the cause: the dip it waited for was the shed-and-reinstall oscillation.
