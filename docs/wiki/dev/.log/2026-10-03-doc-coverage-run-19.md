## [2026-10-03] ingest | doc-coverage run 19 — the guarantees window (v2.18.2 → v2.21.0)

**What:** seven new matrix rows (the guarantee report · profiles rev 2 · a certificate issued off-node ·
the fail-closed start refusals · the rule catalogue · the decision trace · what the trace does not show),
three auditors opening every page and running every must-work instruction; `docs/analysis/doc-coverage.md`.

**Floor before fixes:** 0 ✗ cells, but one instruction that fails literally (guide 19's `--units ./units`
— the flag takes one file), one setting that silently no-ops where the page implied it (`on_unreadable`
bare at top level; no `deny_unknown_fields`), two non-compiling `PersistenceConfig` literals (guides 01 and
13 — fixed in the examples with v2.20.0, not in the guides), two `start()` refusals with no operator
landing (the gateway bind; the audit sink without `[tls]`), twelve stale sentences (the go-live checklist
still said rev 2 "cannot require" `id.ca_key_off_node`; `gateway-tls.md` "inert, stays plaintext";
`audit.md` "logged, not written"; the serve window dated 2.19.0 on two pages and open on a third; the plan's
own header "Nothing here is built"), and two claims on `what-is-proven.md` resting on tests CI never ran.
**After:** all fixed; the two tests in CI (`decision_trace_replay`, the draw-count test); three `~` that
are recorded gaps (no single Dev chapter lists all eight refusals; the 20th-choice divergence is a prose
finding, not a checked-in assertion).

**Durable knowledge.** (1) A release that fixes the in-tree examples for a struct's new field and not the
guide snippets ships non-compiling docs — the config-literal class' fourth hit; guide literals should be
doctests or examples CI compiles. (2) A confinement claim ("every outbound path fails closed") is a
*guarantee* and needs the call-site enumeration, not the page — calibration entry for run 17's Confined-fleet
cell. (3) A release's prose beside the version line — the checklist's profile bullet, the "will require"
sentences — drifts the same way `RELEASING.md` §6's anchors did; both now named there or fixed here.
