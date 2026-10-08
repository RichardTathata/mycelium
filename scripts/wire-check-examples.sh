#!/usr/bin/env bash
# X1: the offline wire-check over every example's declaration directory (examples/units/*).
# An example that provisions carries artifacts/ beside its units, passed as --library.
# Exit 1 if any directory fails; one row per example either way.
set -uo pipefail
cd "$(dirname "$0")/.."
# Verification policy rule 3: each example directory is a case; it prints `@@case@@ <suite>::<case>` as it
# starts, and --list names every directory without checking any.
if [[ "${1:-}" == --list ]]; then
  for d in examples/units/*/; do echo "@@case-list@@ scripts/wire-check-examples.sh::$(basename "$d")"; done
  exit 0
fi
BIN=${WIRE_CHECK_BIN:-}
run() {
  if [[ -n "$BIN" ]]; then "$BIN" wire-check "$@"; else cargo run -q --features cli --bin mycelium -- wire-check "$@"; fi
}
fail=0
for d in examples/units/*/; do
  name=$(basename "$d")
  echo "@@case@@ scripts/wire-check-examples.sh::$name"
  args=("$d")
  [[ -d "$d/artifacts" ]] && args+=(--library "$d/artifacts")
  if out=$(run "${args[@]}" 2>&1); then
    echo "ok    $name — $(echo "$out" | tail -1)"
  else
    echo "FAIL  $name"; echo "$out" | sed 's/^/      /'; fail=1
  fi
done
exit $fail
