# 2026-10-24 — X6: `mycelium test --faults`

Adopter-surface X6. `mycelium::faults`: a fleet of in-process nodes from a declaration and a configuration, and the
five fault classes against it — `revoke` (row Q's three in-process verifiers lifted from `lib_tests.rs`, plus a mandate
at an execution authority), `restart` (rung-2 receipts, replay over per-node directories, a corrupt WAL record refused
by name), `partition` (a runtime dropped; eviction, evaporation after `3 × ttl_secs`, the governed floor, the node
back), `replay` (`sim` only; one kernel, the first divergence), `policy-diff` (the call set at the command's own seam,
re-decided under `--against`). The report names the ids exercised and not, in the guarantee report's vocabulary; no
activation runs (D23). Pages: [operations](../operations.md), `docs/features/test-faults.md`, diagnostics § Before the
fleet carries anything, `what-is-proven.md` (the fleet-against-declaration line).
