## [2026-10-02] ingest | developer teaching path

Added six connected tutorials and a small signed echo stem fleet, with a declaration
export and scenario-specific observation consumer. Updated shared front doors,
example indexes and authority coverage wording. Corrected locality propagation and
both-ways CI claims. Canon: the example sources and `.github/workflows/ci.yml`;
current synthesis: `../examples.md`. Existing unrelated checkout work was preserved.

Local validation: the compiled fleet example passed installation, invocation,
explicit declaration/observation mapping and graceful replacement. The actual
export passed the consumer and all four refusal cases via the checked-in Python
harness. The consumer was built/linted from its exact source in an isolated Cargo
harness using the repository lockfile's dependencies because the shared target
remained locked; consumer clippy passed with `-D warnings`. The WASM example
compiled in the real workspace; its focused clippy command was cancelled while
waiting for that shared lock. Positioning, new relative links, CI YAML/shell syntax
and changed-file whitespace checks passed. Full repository gates and hosted CI
were not run in this task.
