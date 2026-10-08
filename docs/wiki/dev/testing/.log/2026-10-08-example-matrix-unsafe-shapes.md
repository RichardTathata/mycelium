## [2026-10-08] ingest | the examples-matrix check's unsafe-direction approximations narrowed

- `scripts/check-example-matrix.py` read four shapes in a reached script as example runs that never run, each able
  to pass a ✓ row nothing executes: a single-quoted `$(…)`, an unused array assignment, a command under `if false`
  or in an uncalled function, and a Python list outside a subprocess call (a `print` continuation line).
- Now: a quote-aware `$(…)` scan (placeholders, bodies walked in their command's context); a shell walker with a
  control stack (`if`/`elif`/`else`/`fi`, loops, brace groups, function definitions — a body counts only for a
  called function, transitively, or one `trap` names); arrays collected as assignments; Python via `ast`, vectors
  only of process-starting calls.
- Left, documented in the docstring: a conditional or loop whose condition is not a literal still counts
  (unsafe direction, kept — it plausibly runs; round 1 below lists the rest); misses (computed names, prefixes, reusable
  workflows) stay in the safe direction.
- Evidence: four self-test cases, each failing against the 2.28.0 check; the real tree's site set unchanged
  (`CHECK_EXAMPLE_MATRIX_VERBOSE=1` output identical before and after).
- Page: `verification-policy.md`.
- Review round 1 (#571): closed a quote spanning lines (lines joined while a quote is open), `}` as an argument
  (a closer only at command position), `f() ( … )`, an `elif` chain after a literal `true` branch, a plain `$ARR`
  (first element only), `false &&` / `true ||`, code after a top-level `exit`, `if ! false` / compound conditions
  (live), a `case` pattern word, an unparseable Python file (reported by name). Nine cases, each with an assertion
  failing on the round-0 check; site set unchanged. The unsafe-direction residue is listed, not called closed: other
  conditionals and loops, every `case` arm, a function that exits or `return`s, shadowing, nested definitions, a
  quoted `"$RUN"`, Python control flow and scope.
