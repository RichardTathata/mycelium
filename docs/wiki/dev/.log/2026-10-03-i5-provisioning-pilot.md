## [2026-10-03] ingest | I5 — the provisioning pilot of the decision trace

**What:** `Provisioner::with_decision_trace`, nine rules recorded from `provision_round` and the install
task; `StemOptions::trace`; `mycelium-stem --trace-dir`. Two gates: trace-off-vs-on equivalence on outcomes
and hosted state, and saturation. The catalogue's nine `prov.*` rows flip to `Instrumented`.

**Durable knowledge:** (1) *A decision point that returns `bool` cannot be traced honestly.* `eligible()`
had four typed refusals as metric labels and returned `false`; `start_install_as` had three causes for
`false`. Both now return the reason they already had (`Result<(), &str>`, `StartOutcome`) with the `bool`
kept as a wrapper — the trace then records the live verdict, never a second evaluation (G7's first risk,
closed by construction: `eligible_traced` evaluates once). (2) The install's record is written **from the
task**, after the `hosted` lock is released, carrying the round number it started in — a record's trigger
is the round, not the moment. (3) What the pilot could not do is a design fact, not a gap to patch:
activation and probe decide inside a runtime hook with no handle on the sink, so linking install →
activation → probe needs an operation id threaded through `ArtifactRuntime::install` — I6's question.
