## [2026-10-20] ingest | The egress proxy's flood test: a test race, and a late-joiner gap beside it

`a_flood_on_one_proxy_leaves_every_other_proxy_its_share` failed once in CI's macOS confinement job ("A holds no more
than its share: 8"). Not a cap violation: it read the whole `FdBudget` straight after both commands were answered,
and a socket is counted until it is *closed*, not until its answer is read — B's served connection and an upstream
socket's relay task drop their slots a moment after the client sees the answer. Shown by holding an answered socket
open 300 ms (CI's exact message, every run). The test now samples A's own `held` for the whole run, bounds the whole
budget only at a moment with nothing in flight, and waits out closing sockets at the end.

Lesson: a count of held resources is true at every instant, so a test must read it at a moment the test controls —
"the client has its answer" is not "the server has closed". Assert the per-owner bound continuously and the global
bound only at a quiescent point, rather than polling the assertion green.

Found beside it, open: a share shrinks when a proxy joins, but descriptors already held are not returned, so a
late-joining proxy can find less than its share (budget 16: a lone proxy holding 12 leaves the second 4, and none for
its upstream reserve). Recorded in the post-360 plan §3.6.

Pages: none changed (no page states the share bound). Repo: `docs/plans/post-360-hardening.md` §3.6, `CHANGELOG.md`.
