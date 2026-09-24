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

# ── The reserved-prefix lists on the front door ──────────────────────────────────────────────────
#
# WHY THIS IS HERE, and why it is a script rather than a lint checklist item.
#
#   `src/lib.rs` § KV namespace ownership is canon. `docs/guide/building-on-mycelium.md` restates it
#   TWICE — a blockquote an adopter reads first, and a copy-paste bullet they paste into their own
#   notes — because a downstream integrator acts on it: writing under a reserved prefix puts their
#   state in a namespace the substrate will overwrite, gossip and anti-entropy.
#
#   That restating has drifted FOUR times (wiki-lint calibration ledger: 2026-07-20, 2026-09-04,
#   2026-09-05, 2026-09-24). Three times the fix was "update the list"; twice the sharpening was
#   "diff EVERY occurrence, mechanically" — and it drifted again anyway, because nothing ran the
#   diff. The fourth occurrence lost all four v3 prefixes (`knowledge/`, `mandate/`, `rights/`,
#   `cn/`) even though the plan's §7 required both lists to be updated at each item's PR 1.
#
#   A check that depends on someone remembering to run it has the same failure mode as the thing it
#   checks. So it runs in `make check` now.
lib_prefixes=$(grep -oE '^//! \| `[a-z_-]+/' src/lib.rs | grep -oE '`[a-z_-]+/' | tr -d '`' | sort -u)
front_door="docs/guide/building-on-mycelium.md"
if [ -f "$front_door" ]; then
  missing=""
  while read -r prefix; do
    [ -z "$prefix" ] && continue
    # The prefix must appear in the reserved-list region of the front door. Both occurrences live
    # between the "do not write under them" line and the end of the copy-paste block; requiring it
    # simply to appear SOMEWHERE in the file would pass on a stray mention in prose, which is how a
    # list stays short while looking covered.
    # Counted with a leading non-identifier char so `log/` does not also match `clog/`, and with
    # no backtick in the pattern (a backtick inside $(...) is command substitution, not a literal).
    count=$(grep -oE "[^a-z_-]${prefix%/}/" "$front_door" | wc -l | tr -d " ")
    if [ "$count" -lt 2 ]; then
      missing="${missing}  ${prefix} (appears ${count}× — the front door states the list twice)\n"
    fi
  done <<< "$lib_prefixes"
  if [ -n "$missing" ]; then
    echo "FAIL reserved-prefix list on the front door is missing prefixes src/lib.rs owns:"
    printf "$missing"
    echo "  fix: docs/guide/building-on-mycelium.md — BOTH the blockquote and the copy-paste bullet"
    status=1
  else
    echo "Reserved-prefix front-door sweep: clean ($(echo "$lib_prefixes" | wc -l | tr -d ' ') prefixes, both lists)."
  fi
fi

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
