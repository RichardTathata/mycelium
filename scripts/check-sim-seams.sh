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
#   The test-item skip covers exactly the item a `#[cfg(test)]` / `#[cfg(all(test, …))]` attribute
#   applies to — further attributes, then the header to the `;` that ends it or the `{` that opens
#   its body, which ends at the matching `}` at any indentation (braces counted with string and
#   char literals and `//` comments stripped first). Until 2026-10-12 it ran to the next `}` at
#   column 0 instead: right for a top-level `mod tests { … }`, WRONG for everything else. A
#   one-line `#[cfg(test)] mod x;`, a test-only method inside an `impl`, or a test module whose
#   braces close on their own line each hid the production code after them to the end of the
#   enclosing block (or the file) — found 2026-09-18 when a test-only method inside `impl Journal`
#   hid `append`'s `tokio::time::timeout`, worked round then by moving the method, and closed here
#   with the skip `check-kv-namespaces.sh` took from the adversarial review of #591 (post-360 plan
#   row J). The planted files in `scripts/test-check-sim-seams.sh` hold each shape.
#   An attribute on a comma-terminated element — a struct field, an enum variant, a match arm, a
#   struct-literal field — has neither a `;` nor a body: its skip ends at the `,` (or, for a last
#   element with none, at the enclosing `}`). Without that, `signal.rs`'s `#[cfg(test)] trim_hook`
#   field (#602) hid all of production `impl SignalLog` (row J's adversarial review, finding 1).
#   STILL FAIL-OPEN, stated: a block comment or a string literal that spans lines and holds an
#   unbalanced `{` or `}` inside a skipped item (the braces are counted per line, and the pieces of a
#   multi-line literal look like code); a raw string whose interior holds `"#` for a one-hash
#   delimiter. Each makes a skip end late or never. Block comments and raw strings closed on their own
#   line are stripped before counting (planted: `skip_block_comment`, `skip_raw_string`).
#
#   `use tokio::time;` followed by `time::interval(…)` / `time::sleep(…)` WAS invisible to the
#   `tokio::time::*` pattern — the same alias gap the `fs as <alias>` handling closes for the
#   filesystem. Found 2026-09-18 routing nine interval sites through the timer seam: the baseline
#   moved for the four files that spell `tokio::time::interval` out and not for the three
#   (`opacity.rs`, `emergent_groups.rs`, `tasks.rs`) that go through the alias, whose tickers this
#   check had never counted. **Closed the same day** (`alias_pattern`, the `time` half): a file that
#   imports the module under any spelling has its `<alias>::sleep|interval|timeout|Instant` sites
#   counted, and the baseline was regenerated to admit the pre-existing ones — 19 sites in eight
#   files, 166 → 185. A `time as <alias>` import is handled but has no user today.
#
# EXEMPT
#   `mycelium-sim/**` and `sim_seam.rs` (the seams themselves — §6 exempts the seam
#   implementations, which is where these calls are supposed to live), test files, and each
#   `#[cfg(test)]` / `#[cfg(all(test, …))]` item, at any indentation —
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
# The wasm host is outside the main scan (its artifact/fs code is a baseline decision of its own), but a
# **random draw** there is replay-critical: the provisioner's self-election was raw `fastrand`, so under
# `sim` the draw was not a kernel choice and a recording could not reproduce who installed (2026-10-03).
# Zero baseline, no exceptions.
host_rng=$(grep -rnE 'fastrand::|rand::(thread_rng|random)' mycelium-wasm-host/src --include='*.rs' | grep -vE '^\S+:[0-9]+:\s*//' || true)
if [ -n "$host_rng" ]; then
  echo "check-sim-seams: a random draw outside the seam in mycelium-wasm-host (route it through mycelium::sim_seam::rng_*):" >&2
  echo "$host_rng" >&2
  exit 1
fi

scan_files() {
  find src mycelium-core/src mycelium-wasm-host/src -name '*.rs' \
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

  # The same gap for the timer: `use tokio::time;`, a grouped `use tokio::{…, time, …}` (on one
  # line or with `time,` on its own), `use tokio::time::{self, …}` and `use tokio::time as <alias>`
  # each make `<alias>::sleep(…)` / `::interval(…)` / `::timeout(…)` / `::Instant` a tokio timer
  # call the `tokio::time::` pattern never sees. Found 2026-09-18 routing nine tickers (below);
  # closed here. The call-site alternation is guarded on the left by `(^|[^:a-zA-Z_])` rather than
  # `\b`, so `std::time::Instant` and `tokio::time::sleep` are not matched a second time through it.
  # The bare `use tokio::time;` line itself is not counted — it enables nothing on its own.
  local time_aliases
  # Every alias the module gets under, read from whole `use` statements (perl, so a statement that
  # spans lines, nests groups or is `pub use` is one unit): `use tokio::time as X`, `time as X`
  # anywhere inside a `use tokio::{…}` (one line, its own line, nested groups), and
  # `time::{self as X, …}`. The grep version of this missed a grouped `time as ttime`, which hid
  # `writer.rs`'s timer sites until #602's review (2026-10-10), and its first repair still missed
  # `{time as tt, io}`, `time::{self as t}`, nested groups and `pub use` (the re-review); planted
  # files for each form are checked by `scripts/test-check-sim-seams.sh`. A `std::{time as X}` alias
  # is caught too, which is correct: `X::Instant` is the monotonic clock.
  time_aliases=$(perl -0777 -ne '
      while (/\b(?:pub(?:\([^)]*\))?\s+)?use\s+([^;]*);/gs) {
        my $u = $1;
        next unless $u =~ /\b(?:tokio|std)\b/;
        while ($u =~ /\btime\s+as\s+(\w+)/g) { print "$1\n" }
        while ($u =~ /\btime::\{([^}]*)\}/gs) {
          my $g = $1;
          while ($g =~ /\bself\s+as\s+(\w+)/g) { print "$1\n" }
        }
      }' "$file" 2>/dev/null || true)
  if grep -qE '^use tokio::time;|^use tokio::time::\{[[:space:]]*self|^use tokio::\{.*[[:space:],{]time([[:space:],}]|::\{[[:space:]]*self)|^[[:space:]]+time,[[:space:]]*$' "$file" 2>/dev/null; then
    time_aliases="$time_aliases"$'\n'"time"
  fi
  while IFS= read -r a; do
    [ -z "$a" ] && continue
    extra="$extra|(^|[^:a-zA-Z_])$a::(sleep|interval|timeout|Instant)"
  done <<< "$time_aliases"
  printf '%s' "$extra"
}

# Count forbidden sites outside test code.
#
# Skips exactly the item a `#[cfg(test)]` / `#[cfg(all(test, …))]` attribute applies to: further
# attributes, then its header up to a `;` that ends it (`mod x;`, `use …;`, a `const`) or the `{`
# that opens its body, which ends at the matching `}` — braces counted at any indentation, with
# string and char literals and `//` comments stripped first (the shape `check-kv-namespaces.sh`
# uses). Three earlier versions failed open: the first skipped "everything after the first
# `#[cfg(test)]`", and a probe that appended a live `Instant::now` below a test module sailed through;
# the second ran to the next column-0 `}`, so a one-line `#[cfg(test)] mod x;`, an indented test-only
# method, or a test module closing its braces on its own line hid the production code after it (row
# J, 2026-10-12: of every file scanned, only `src/consensus.rs` moved, 0 -> 5); the third, row J's
# first cut, ended only at a `;` or a body, so a `#[cfg(test)]` field hid the impl after it
# (`signal.rs`, 8 -> 11 when fixed). Each shape is a planted file in `scripts/test-check-sim-seams.sh`,
# which is the only way these are ever found.
count_in() {
  local file="$1"
  awk '
    function braces(s,   t, o, c) {
      t = s
      # Raw strings first (`r#"a " {"#` holds a quote the plain-string pattern would pair wrongly),
      # then plain strings and chars, then block comments closed on this line, then a line comment.
      gsub(/r#+"([^"]|"[^#])*"#+/, "", t)
      gsub(/"([^"\\]|\\.)*"/, "", t)
      gsub(/\047([^\047\\]|\\.)\047/, "", t)
      gsub(/\/\*([^*]|\*+[^*\/])*\*+\//, "", t)
      sub(/\/\/.*$/, "", t)
      o = gsub(/\{/, "{", t); c = gsub(/\}/, "}", t)
      opens = o; closes = c
      semi = (t ~ /;[ \t]*$/)
      comma = (t ~ /,[ \t]*$/)
    }
    # One line of a skipped item: returns once the item has ended.
    function skip_line(s) {
      braces(s)
      if (!started) {
        if (opens > 0) { started = 1; depth = opens - closes; if (depth <= 0) skip = 0; return }
        # A field, a variant, a match arm or a struct-literal field ends at its `,` — or, the last
        # one with no comma, at the enclosing block `}` (the row J review, finding 1).
        if (semi || comma || closes > opens) { skip = 0 }
        return
      }
      depth += opens - closes
      if (depth <= 0) skip = 0
    }
    skip {
      if (!started && $0 ~ /^[ \t]*#\[/) next
      skip_line($0); next
    }
    # `test` must be followed by `,` or `)` so `feature = "test-util"` cannot match; `any(test, …)`
    # is production under a feature and is not skipped.
    /^[ \t]*#\[cfg\((all\()?test[,)]/ {
      skip = 1; started = 0; depth = 0
      rest = $0; sub(/^[ \t]*#\[cfg\([^]]*\)\][ \t]*/, "", rest)
      if (rest != "") skip_line(rest)
      next
    }
    # Comments are prose, not calls. A doc comment that *names* `SystemTime::now` to explain why it
    # is no longer called would otherwise count as a call — which it did, on this very file.
    /^[ \t]*\/\// { next }
    { print }
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

if [ "${1:-}" = "--count" ]; then
  # Count one file's forbidden sites as the gate would (used by scripts/test-check-sim-seams.sh).
  count_in "$2"
  exit 0
fi

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
