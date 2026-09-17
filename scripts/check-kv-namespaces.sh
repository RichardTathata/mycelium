#!/usr/bin/env bash
# The KV namespace sweep — item 2 PR 1 (D7).
#
# WHY IT EXISTS
#   `docs/design/federated-domains.md` §8 states an invariant: **foreign observations never enter the gossip
#   KV namespace**. There is no `federation/` prefix and there must not be one, because putting a foreign
#   observation in KV is the shortest path to making it visible everywhere — which is exactly what §2 of that
#   record forbids, and it is indistinguishable from local state once it is there.
#
#   The plan (`docs/plans/v3-contracts-axis.md` §5) already said "the wiki lint's namespace sweep checks it".
#   It did not: no such sweep existed when the ADR was written on 2026-09-17. An invariant nothing checks is a
#   sentence, so this is the check that makes the claim true.
#
# WHAT IT CHECKS
#   No forbidden KV prefix literal appears in production sources. The list is deliberately short and each
#   entry cites the record that forbids it — a prefix is added here only when a design record says why.
#
# WHAT IT CANNOT SEE — stated rather than glossed
#   A prefix assembled at runtime (`format!("{a}/{b}")`) is invisible to it, as is one reached through a
#   constant defined elsewhere. It catches the way this mistake is actually made — someone writes the literal —
#   and it does not pretend to be a type system. The registry of *legitimate* prefixes remains `kv_ns`
#   (`mycelium-core/src/signal.rs`), and adding a namespace still means adding a `kv_ns` entry.
#
# USAGE
#   scripts/check-kv-namespaces.sh

set -euo pipefail

cd "$(dirname "$0")/.."

# prefix<TAB>the record that forbids it
FORBIDDEN=$(cat <<'LIST'
federation/	docs/design/federated-domains.md §8 (D7) — foreign state never enters the medium
LIST
)

# Production sources only. Test modules are exempt for the same reason they are in the seam check: a test
# that names a forbidden prefix to prove it is absent is doing its job.
scan_files() {
  find src mycelium-core/src -name '*.rs' \
    ! -name '*_tests.rs' \
    ! -name 'lib_tests.rs' \
    ! -name 'test_util.rs' \
    | sort
}

# Skip each top-level `#[cfg(test)]` item and comment lines — the same shape `check-sim-seams.sh` uses, and for
# the same reasons (production code below a test module is still production code; a doc comment naming a prefix
# to forbid it is prose, not a write).
production_lines() {
  awk '
    /^[[:space:]]*#\[cfg\(test\)\]/ { skip = 1 }
    skip && /^}/                    { skip = 0; next }
    /^[[:space:]]*\/\//             { next }
    !skip                           { print FILENAME ":" FNR ":" $0 }
  ' "$1"
}

status=0
while IFS=$'\t' read -r prefix record; do
  [ -z "$prefix" ] && continue
  hits=""
  while IFS= read -r f; do
    found=$(production_lines "$f" | grep -F "\"$prefix" || true)
    [ -n "$found" ] && hits="$hits$found"$'\n'
  done < <(scan_files)

  if [ -n "$hits" ]; then
    echo "FAIL forbidden KV prefix \"$prefix\" appears in production code:"
    printf '%s' "$hits" | sed 's/^/  /'
    echo "  forbidden by: $record"
    status=1
  fi
done <<< "$FORBIDDEN"

if [ "$status" -ne 0 ]; then
  cat >&2 <<'MSG'

A forbidden KV prefix appeared in production code.

This is not a naming rule. A prefix listed here is one a design record says must never carry state, because
once it is in the gossip KV namespace it is replicated, anti-entropied and indistinguishable from state this
mesh produced itself.

If the state genuinely belongs in KV, that is a change to the record first:
  1. amend the design record that forbids the prefix, with what changed;
  2. remove it from FORBIDDEN in this script, in the same PR.
MSG
else
  echo "KV namespace sweep: clean."
fi

exit "$status"
