# ingest — P6.1: batch commits + whole-batch gate (2026-08-15)

**Shipped:** `PageWrite` + `WikiStore::write_pages(pages, label)` — an **additive default trait
method** (per-page loop; Fs/S3 semantics untouched). `GitStore` overrides it: N blobs → one private
index → **one tree, one commit** per batch (`batch({label}) — N page(s)`), with a tree-equality
check making idempotent re-applies record nothing. The write gate becomes `gate_check_batch`: every
candidate placed in the worktree, **one** validator run with the full file list as argv (their
`check-pages.sh` shape), restore either way — so the 38–90s validator runs once per meeting, not
once per page. `apply_batch` rides `write_pages`: an ingest batch = one per-meeting boundary commit,
and a gate refusal is **whole-batch atomic** (nothing commits — FTT's whole-meetings-only crash
invariant restored; the Phase-4 per-page-skip semantics retired, recorded in the plan).

**A fixture caught the contract change:** the Phase-3 stub validator checked only `$1` — under the
new full-argv contract it silently passed a batch whose THIRD file was invalid (the updated gate
test failed exactly there). The stub is now a file-list validator. Lesson: when a contract widens
from one item to a list, every fixture written against "one" is a silent pass waiting to happen.

**Gates green:** `ingest_is_byte_identical_to_the_serial_writer_and_lands_as_one_commit` (trees
identical to the serial writer; serial=2 commits, batch=1; resubmit records nothing) ·
`a_gate_refusal_refuses_the_whole_batch_atomically` ((0,3) refused, list_pages empty, fixed batch =
one commit) · exactly-once, curator, and control-plane suites unchanged. Gaps 4+6 closed;
next: P6.2 (read plane).
