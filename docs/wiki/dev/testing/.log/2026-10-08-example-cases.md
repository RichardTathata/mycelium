## [2026-10-08] ingest | single example demonstrations as cases; the examples matrix's CI column checked

- Observed: every `cargo run … --example` in `ci.yml` (eighteen) runs through `scripts/example-case.sh`, which
  prints `@@case@@ examples::<NAME>` and execs the command unchanged. `scripts/example-case.sh --list` (→
  `scripts/check-example-matrix.py --list ci.yml`) reads the calls from the workflow's text, fails on a call without
  a literal name and on any `cargo run --example` the wrapper does not front; `test-universe`'s `scripts` block
  lists them. No parser change: the key `script examples::<NAME>` was already in `CASE_KEY`'s charset.
- Static: `scripts/check-example-matrix.py` (make check + CI) — a ✓ row needs an execution site (wrapped call, a
  built example run as a process, a script a running step reaches that runs it, or a listed suite case whose
  suite takes the case as its cargo target); an executed example needs a ✓ row or a place among the README's
  harness binaries. Step semantics are `check-test-inventory.py`'s `ci_commands` (triggers, `if:`, loops).
- First run: `reason_node` and `reheal_node` (the LangGraph rungs' nodes, run in every CI run) said · — flipped
  to ✓. Every ✓ row had a site.
- Pages: `verification-policy.md`, `docs/operations/what-is-proven.md`'s universe row, `examples/README.md`'s legend.
- Review round 1 (#566): ✓ᵖ for path-filtered workflows (the check reads `paths:` itself; the inventory's `_triggers`
  unchanged) — `confined_fleet_node` now ✓ᵖ; the wrapper and the listing refuse anything but `cargo [+tc] run`, a
  name starting with `-`, and an unfronted run after a `cargo build` on one line; a reached file's heredocs,
  comments and Python docstrings are not read, a file counts only in an executing position, a built-example path
  only as a program (or a variable used as one, or a Dockerfile instruction); the harness exemption no longer covers
  a name with a row; a `Makefile:<target>` suite must be reached. Twelve self-test cases, each failing on the
  round-0 implementation.
