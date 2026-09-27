## [2026-09-27] fix | the example catalogue says per row what runs in CI

Up: [dev](../dev.md) · [examples](../examples.md).

A reader noticed that two group-header rows of `examples/README.md`'s capability matrix read as
notes rather than entries — *"…All run in CI"* in bold prose inside the table. Checked: the claim
was false for `coordination_viz` and `control_envelope_viz` (browser demos, never run) and for
`destination_commit` (not run anywhere). The fix is structural: a **CI** facet column, derived per
row from `ci.yml`, the suite smokes and the cluster-suites workflow (✓ executed · · compiled only),
so the claim is per example and checkable; the two headers are terse again and their sentences
live in a paragraph above the table; `destination_commit` now runs in the effects job so its row
is a ✓ rather than a corrected claim. Browser demos remain · by the UI-example contract's own
statement that the batch smoke does not cover them.
