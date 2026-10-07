#!/usr/bin/env bash
# The CI flake tier (Run-38 floor fix — docs/wiki/dev/testing/testing.md §The CI flake tier).
#
# Socket-binding / multi-node test suites are timing-sensitive by nature; before this tier a
# single wall-clock flake redded main (2026-07-07: a wiki `AddrInUse` port race), and the
# reactive alternative — widening timeouts — is exactly how a Major liveness bug hid for ten
# analysis runs (Run 37's lesson). This wrapper is the structural compromise:
#
#   * run the suite once; all green → done.
#   * on failure, re-run ONLY the failed tests, individually, once.
#       - rerun fails      → REAL failure → exit 1 (a deterministic bug always reds the build:
#                            it fails twice).
#       - rerun passes     → the build stays green BUT the flake is loudly recorded — a GitHub
#                            warning annotation + step-summary line per test. A flake is a bug
#                            report with visibility, never noise: recurring annotations must get
#                            a root-cause issue, not a timeout widen.
#   * compile errors / suite-level failures with no parseable test list → exit 1 unchanged.
#
# Deterministic unit gates should stay on bare `cargo test`; route only the port-binding suites
# through this tier.
#
# STRICT MODE (`CI_RETEST_STRICT=1`): the retry still runs, and the annotation is still written,
# but a pass-on-retry is a FAILURE. For the gates that carry a security or correctness claim —
# the audit chain and both gateway enforcement points — "eventually green" is not the same as
# deterministic, and an intermittent correctness failure would otherwise pass on the second try
# (external review, 2026-09-26). The isolated rerun is kept because it is diagnostic: it tells
# the reader whether the failure is order-dependent.
set -uo pipefail
strict="${CI_RETEST_STRICT:-0}"

log="$(mktemp)"
trap 'rm -f "$log"' EXIT

# --no-fail-fast: without it cargo stops at the first failing test binary, so a flake in one binary meant
# every later binary in the step never ran — and the retry below re-runs only the failed test, so the step
# went green with those binaries unexecuted. Found by the test-coverage job's first run (2026-10-07):
# in that run mycelium-reason's `tests/ollama.rs` did not execute, behind a flake in an earlier binary.
if cargo test --no-fail-fast "$@" 2>&1 | tee "$log"; then
  exit 0
fi

# A test binary that crashed — a signal, or an exit status other than libtest's 101 for "tests failed" —
# printed no `failures:` list, so a retry of the others could never cover it. Under --no-fail-fast it
# would otherwise go green beside a retried flake (the review of #541).
if grep -E "process didn't exit successfully: .*\((signal: |exit status: [0-9]+)" "$log" | grep -vq "exit status: 101)"; then
  echo "── ci-retest: a test binary crashed (signal or unexpected exit) — real failure ──"
  grep -E "process didn't exit successfully" "$log" | grep -v "exit status: 101)"
  exit 1
fi

# Collect failed test names from every `failures:` block in the combined output — the whole indented line,
# since a doctest's name has spaces (`src/lib.rs - item (line 3)`); `$1` alone retried `src/lib.rs`, which
# matched nothing and passed as a flake.
failed=()
while IFS= read -r t; do failed+=("$t"); done < <(awk '/^failures:$/{f=1;next} f&&/^    [^ ]/{print substr($0,5)} f&&!/^    /{f=0}' "$log" | sort -u)

if [ "${#failed[@]}" -eq 0 ]; then
  echo "── ci-retest: failure with no parseable failed-test list (compile error?) — real failure ──"
  exit 1
fi

echo "── ci-retest: re-running ${#failed[@]} failed test(s) individually (flake tier) ──"
rc=0
for t in "${failed[@]}"; do
  retry="$(mktemp)"
  # A retry passes only if it ran the test: a name that matches nothing runs 0 tests and exits 0.
  if cargo test "$@" -- --exact "$t" 2>&1 | tee -a "$log" > "$retry" \
     && grep -Eq "^test result: ok\. [1-9][0-9]* passed" "$retry"; then
    msg="$t failed once, passed on isolated retry (cargo test $*). A flake is a bug report — root-cause it (testing.md §The CI flake tier); never 'fix' it by widening a timeout (Run-37 lesson)."
    echo "::warning title=FLAKY TEST::${msg}"
    if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
      echo "⚠️ **FLAKY**: \`$t\` (\`cargo test $*\`)" >> "$GITHUB_STEP_SUMMARY"
    fi
    if [ "$strict" = "1" ]; then
      echo "── ci-retest: STRICT — a pass on retry is a failure for this gate ──"
      rc=1
    fi
  else
    echo "── ci-retest: $t failed twice (or its retry ran nothing) — real failure ──"
    rc=1
  fi
  rm -f "$retry"
done
exit $rc
