#!/usr/bin/env bash
# The static forbidden-call check — item 6 PR 3 (D12: moved here from PR 7).
#
# WHY IT EXISTS
#   `docs/design/replay-nondeterminism-inventory.md` §6: a direct `SystemTime::now`, `Instant::now`,
#   `fastrand::`, `tokio::time::*`, `tokio::fs`/`std::fs` or unseeded `RandomState::new` outside the
#   seam modules must fail the build, "so the seams cannot erode while PR 4–7 are built". A harness
#   that routes today's nondeterminism through a kernel is worth nothing if next month's code walks
#   round it, and nobody notices until a replay quietly stops reproducing.
#
# WHY A SCRIPT AND NOT THE `clippy.toml` THE INVENTORY SUGGESTED
#   §6 offers "a `clippy.toml` `disallowed-methods` list scoped to those modules". Clippy's
#   `disallowed-methods` is real, but it is **workspace-global** — it cannot be scoped to modules,
#   so it would also fire inside the seam implementations and every test, which is precisely where
#   these calls belong. A `#[cfg]`-gated deny lint has the same problem in reverse: Rust has no lint
#   for "do not call this function". So: a baseline-diffing script, which enforces the same rule the
#   inventory states — *a new site is admitted only by editing the record*.
#
# WHAT IT COMPARES
#   Per file, the number of forbidden sites, against `scripts/sim-seams-baseline.txt`. A file that
#   gains one fails. A file that loses one also reports — quietly, as progress — and the baseline
#   should be regenerated so the count cannot silently drift back up.
#
# WHAT IT CANNOT SEE — stated rather than glossed
#   It matches qualified call sites (`Instant::now`, `tokio::fs::…`), the imports that enable
#   unqualified ones (`use std::time::Instant`), and `fs as <alias>` imports **plus every use of
#   that alias** — the last because `persistence.rs` aliases `tokio::fs` to `tfs` and was otherwise
#   invisible to this check entirely.
#   It does not parse Rust: a call reached through a re-export this script does not know about, or
#   through a type alias, is invisible to it. That is a real gap, and the honest mitigation is that
#   the baseline makes *movement* visible even when it cannot attribute it.
#
#   The test-module skip runs from a `#[cfg(test)]` to the next `}` at column 0. That is right for a
#   top-level item and WRONG for a `#[cfg(test)]` on an item *inside* an `impl` or `mod`: the skip
#   then swallows the rest of the enclosing block, and production code after the gated item goes
#   uncounted. Found 2026-09-18 when a test-only method inside `impl Journal` hid `append`'s
#   `tokio::time::timeout` — the baseline dropped by one for a file whose sites had not changed.
#   Put test-only methods in a separate top-level `#[cfg(test)] impl` block; a baseline that moves
#   when no site moved is the signal that someone did not.
#
# EXEMPT
#   `mycelium-sim/**` and `sim_seam.rs` (the seams themselves — §6 exempts the seam
#   implementations, which is where these calls are supposed to live), test files, and each
#   top-level `#[cfg(test)]` item —
#   a test that reads the real clock is doing its job. Note "each item", not "everything after the
#   first": production code below a test module is still production code.
#
# USAGE
#   scripts/check-sim-seams.sh            # check against the baseline
#   scripts/check-sim-seams.sh --update   # regenerate the baseline (then edit the inventory too)

set -euo pipefail

cd "$(dirname "$0")/.."
BASELINE="scripts/sim-seams-baseline.txt"

# The forbidden shapes. Kept as one alternation so the list is readable and matches §6 one-for-one.
PATTERN='SystemTime::now|Instant::now|fastrand::|tokio::time::(sleep|interval|timeout|Instant)|tokio::fs::|std::fs::|RandomState::new|use std::time::\{?[^}]*Instant|use tokio::time::|fs as [a-z_]|\.try_send\('

# Production sources only. `mycelium-sim` is the seam; `loom-spike` is a different mechanism
# (coverage map: Loom owns CAS interleavings, not the kernel).
scan_files() {
  find src mycelium-core/src -name '*.rs' \
    ! -name '*_tests.rs' \
    ! -name 'lib_tests.rs' \
    ! -name 'test_util.rs' \
    ! -name 'sim_seam.rs' \
    | sort
}

# Any `fs as <alias>` import in this file, as an extra alternation.
#
# `persistence.rs` — the module the inventory calls "the right first target", with 26 fs references
# — imports `fs as tfs` inside a grouped `use tokio::{…}` and then writes `tfs::read(…)`. The first
# version of this check matched only `tokio::fs::`, so that file was **entirely invisible**: the
# check reported "clean" on the single most important file in its scope. Third bug in this script,
# same failure mode as the other two — wrong in the safe direction, which is how a tool quietly
# stops meaning anything.
alias_pattern() {
  local file="$1"
  local aliases
  aliases=$(grep -oE 'fs as [a-z_][a-z0-9_]*' "$file" 2>/dev/null | awk '{print $3}' | sort -u || true)
  local extra=""
  while IFS= read -r a; do
    [ -z "$a" ] && continue
    extra="$extra|\\b$a::"
  done <<< "$aliases"
  printf '%s' "$extra"
}

# Count forbidden sites outside test code.
#
# Skips each top-level `#[cfg(test)]` item, from its attribute to the closing brace in column 0 —
# NOT "everything after the first one". The first version of this did the latter, and a probe that
# appended a live `Instant::now` below a test module sailed through: code after a test module is
# ordinary production code, and Rust is perfectly happy to put it there. Found by testing the
# checker against a deliberately planted site, which is the only way these are ever found.
count_in() {
  local file="$1"
  awk '
    /^[[:space:]]*#\[cfg\(test\)\]/ { skip = 1 }
    skip && /^}/                       { skip = 0; next }
    # Comments are prose, not calls. A doc comment that *names* `SystemTime::now` to explain why it
    # is no longer called would otherwise count as a call — which it did, on this very file.
    /^[[:space:]]*\/\//                { next }
    !skip                              { print }
  ' "$file" | grep -cE "$PATTERN$(alias_pattern "$file")" || true
}

generate() {
  while IFS= read -r f; do
    local_count=$(count_in "$f")
    if [ "$local_count" -gt 0 ]; then
      echo "$local_count $f"
    fi
  done < <(scan_files)
}

if [ "${1:-}" = "--update" ]; then
  {
    echo "# Forbidden-call baseline for the replay seams (item 6 PR 3)."
    echo "# Regenerate: scripts/check-sim-seams.sh --update"
    echo "#"
    echo "# A line is 'count path'. Raising a count fails the check: every site here is one the"
    echo "# kernel must eventually own, and a new one is a new way for a replay to stop reproducing."
    echo "# Admitting a site means editing docs/design/replay-nondeterminism-inventory.md §2 as well"
    echo "# — the record is the decision, this file is only its machine-readable shadow."
    generate
  } > "$BASELINE"
  echo "baseline written to $BASELINE"
  exit 0
fi

if [ ! -f "$BASELINE" ]; then
  echo "error: $BASELINE is missing. Generate it with: scripts/check-sim-seams.sh --update" >&2
  exit 2
fi

status=0
declare -a improved=()

while IFS= read -r line; do
  case "$line" in \#*|'') continue ;; esac
  expected=${line%% *}
  file=${line#* }
  if [ ! -f "$file" ]; then
    echo "note: $file is in the baseline but no longer exists — regenerate the baseline"
    continue
  fi
  actual=$(count_in "$file")
  if [ "$actual" -gt "$expected" ]; then
    echo "FAIL $file: $actual forbidden sites, baseline $expected (+$((actual - expected)))"
    status=1
  elif [ "$actual" -lt "$expected" ]; then
    improved+=("$file: $actual, was $expected")
  fi
done < "$BASELINE"

# A file absent from the baseline must have none at all.
while IFS= read -r f; do
  if ! grep -qE "^[0-9]+ $f\$" "$BASELINE"; then
    actual=$(count_in "$f")
    if [ "$actual" -gt 0 ]; then
      echo "FAIL $f: $actual forbidden sites, not in the baseline"
      status=1
    fi
  fi
done < <(scan_files)

if [ ${#improved[@]} -gt 0 ]; then
  echo "progress — these files now have fewer forbidden sites than the baseline:"
  printf '  %s\n' "${improved[@]}"
  echo "regenerate with: scripts/check-sim-seams.sh --update"
fi

if [ "$status" -ne 0 ]; then
  cat >&2 <<'MSG'

A new nondeterministic call site appeared outside the replay seams.

This is not a style rule. Every site above is one the kernel must eventually own; a new one is a
new way for a recorded run to stop reproducing, discovered later and attributed to nothing.

If the call belongs behind a seam, route it through `mycelium_sim::Seams`.
If it genuinely belongs where it is, admit it deliberately:
  1. add it to docs/design/replay-nondeterminism-inventory.md §2, with what depends on it;
  2. run scripts/check-sim-seams.sh --update.
MSG
fi

exit "$status"
