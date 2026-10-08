#!/usr/bin/env bash
# Verification policy rule 3, for a single demonstration: `scripts/example-case.sh <cargo command…>` prints
# `@@case@@ examples::<NAME>` for the command's `--example NAME` (or `--example=NAME`), then runs the command
# unchanged, so the test-coverage job sees each `cargo run … --example` in ci.yml as a case that executed —
# or did not — rather than as one exit code among a step's. The command's own text follows the wrapper intact.
#
# `--list [workflow]` names every case without running anything: one `@@case-list@@ examples::<NAME>` per
# wrapper call in the workflow (default .github/workflows/ci.yml), read from the workflow's text, and it fails
# on a call it cannot read — a name that is not a literal — and on a `cargo run … --example` the wrapper
# does not front (scripts/check-example-matrix.py --list).
set -euo pipefail
if [[ "${1:-}" == "--list" ]]; then
  here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  exec python3 "$here/check-example-matrix.py" --list "${2:-.github/workflows/ci.yml}"
fi
# Only `cargo [+toolchain] run …`: anything else is not a demonstration this case could stand for.
sub="${2:-}"; [[ "$sub" == +* ]] && sub="${3:-}"
if [[ "${1:-}" != "cargo" || "$sub" != "run" ]]; then
  echo "example-case.sh: fronts only \`cargo [+toolchain] run … --example NAME\`, not: $*" >&2
  exit 2
fi
name="" prev=""
for a in "$@"; do
  [[ "$a" == "--" ]] && break          # what follows is the example's own arguments
  if [[ "$prev" == "--example" ]]; then name="$a"; break; fi
  if [[ "$a" == --example=* ]]; then name="${a#--example=}"; break; fi
  prev="$a"
done
if [[ ! "$name" =~ ^[A-Za-z0-9_][A-Za-z0-9_-]*$ ]]; then
  echo "example-case.sh: no literal --example NAME in: $*" >&2
  exit 2
fi
echo "@@case@@ examples::$name"
exec "$@"
