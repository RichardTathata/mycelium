#!/usr/bin/env bash
# The KV namespace sweep — item 2 PR 1 (D7); since 2026-10-10 also the namespace-table gate.
#
# Three checks, one script, all in `make check` and CI:
#   1. no FORBIDDEN prefix literal in production code (D7, below);
#   2. every KV prefix production code uses has a row in `src/lib.rs` § KV namespace ownership, or is a
#      declared non-key in `scripts/kv-namespaces-nonkeys.txt` (the table gate, 2026-10-10);
#   3. the front door restates the table's prefixes twice (the reserved-prefix lists).
#
# ── 1. WHY THE FORBIDDEN CHECK EXISTS ────────────────────────────────────────────────────────────
#   `docs/design/federated-domains.md` §8 states an invariant: **foreign observations never enter the gossip
#   KV namespace**. There is no `federation/` prefix and there must not be one, because putting a foreign
#   observation in KV is the shortest path to making it visible everywhere — which is exactly what §2 of that
#   record forbids, and it is indistinguishable from local state once it is there.
#
#   The plan (`docs/plans/v3-contracts-axis.md` §5) already said "the wiki lint's namespace sweep checks it".
#   It did not: no such sweep existed when the ADR was written on 2026-09-17. An invariant nothing checks is a
#   sentence, so this is the check that makes the claim true.
#
#   What it checks: no forbidden KV prefix literal appears in production sources. The list is deliberately
#   short and each entry cites the record that forbids it — a prefix is added here only when a design record
#   says why. A prefix assembled at runtime (`format!("{a}/{b}")`) is invisible to it, as is one reached
#   through a constant defined elsewhere.
#
# ── 2. WHY THE TABLE GATE EXISTS ─────────────────────────────────────────────────────────────────
#   `src/lib.rs` § KV namespace ownership is canon for who owns which prefix, and the wiki lint was the only
#   thing that compared it with code. The lint missed live prefixes FIVE times (ledger 2026-07-07: nine
#   prefixes; … 2026-10-10: `sys/config/` and three `sys/govern/…` keys, rowless since June, because that pass
#   counted only `kv_ns`/`consensus_ns`). A comparison a person must remember to run has the failure mode of
#   the drift it checks, so the enumeration is this script's job now and the lint only reviews the allow-list.
#
#   WHAT IT ENUMERATES — production code in every library crate: `src/` (incl. `src/bin/`), `mycelium-core/src`
#   and every `mycelium-*/src` except `mycelium-gateway-free-tests` (a test crate); never a `tests/` directory,
#   a `*_tests.rs`/`tests.rs`/`test_util.rs` file, or a top-level `#[cfg(test)]` / `#[cfg(all(test, …))]` item.
#   In it, every string literal whose head is a path segment (`"seg/…"`, seg = `[a-z][a-z0-9_.-]*`) in one of
#   three shapes:
#     const   `const X: &str = "seg/…"` / `static X: &str = …` — whatever the name (`_KEY`, `_PREFIX`, `_NS`,
#             `kv_ns::*`, `consensus_ns::*` alike), the literal on the same line or the next;
#     format  `format!("seg/…")` — the literal head, on the same line or the next. Every one, not only those
#             visibly passed to a writer: `let key = format!(…); kv.set(key, …)` is the common shape, and a
#             format! that is not a key is a non-key the allow-list names;
#     call    the first string literal inside a call to the KV call set below, on that line —
#             KvHandle:   set set_async set_with_receipt(_as) set_requiring_sync retry_requiring_sync
#                         retry_with_receipt(_as) prepare_write delete delete_async get scan_prefix
#                         subscribe subscribe_prefix subscribe_prefix_with_predicate
#             core ops:   kv_* (kv_set, kv_get, kv_scan_prefix, kv_subscribe_prefix, …) scan_kv_prefix
#                         scan_prefix_kv_with_ts subscribe_prefix_on_kv make_gossip_update(_stamped)
#                         publish_* (publish_schema, publish_field, publish_*_intent, …)
#             key parsing: strip_prefix starts_with parse_cap_key_or_warn — a prefix being parsed is a
#                         namespace in use.
#             The log verbs (`append`, `scan_log`, `subscribe_log*`, `compact_log`) are deliberately NOT in
#             the set: their argument is a stream name and the key is `log/{stream}/…`, which has a row.
#   Each literal is reduced to its namespace — the top segment (`cap/`), and under `sys/` the second as well
#   (`sys/config/`) — and must match a row's namespace, or start with an allow-list entry.
#
#   WHAT IT CANNOT SEE — stated rather than glossed. A prefix assembled from pieces (`format!("{a}/{b}")`, a
#   literal built with `concat!` or `+`), a key literal held in a `let` and passed later, a call whose literal
#   sits two or more lines below the call, and a `#[cfg(test)]` item nested inside an `impl` (the skip runs
#   to the next column-0 `}`, as in `check-sim-seams.sh`). It does not parse Rust; it closes the way this
#   mistake has actually been made five times — a named constant or a format! key with no row.
#
#   THE ALLOW-LIST — `scripts/kv-namespaces-nonkeys.txt`, one entry per line, `<literal head>  # <reason>`.
#   An entry admits every literal that starts with it. Each entry needs a reason; an entry that admits
#   nothing fails (stale), so the list cannot grow quietly past what the code needs.
#
# ── 3. THE FRONT-DOOR LISTS ── see the comment at that check.
#
# USAGE
#   scripts/check-kv-namespaces.sh               # the gate
#   scripts/check-kv-namespaces.sh --list        # every enumerated literal and how it was classified
#   scripts/check-kv-namespaces.sh --self-test   # plant rowless prefixes in a scratch copy, show the gate
#                                                # fails naming each, then show the real tree passes
#   scripts/check-kv-namespaces.sh --root DIR    # run against another tree (the self-test's mode)

set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
SCRIPT="$SCRIPT_DIR/$(basename "$0")"
ROOT="$SCRIPT_DIR/.."
MODE=gate
while [ $# -gt 0 ]; do
  case "$1" in
    --list)      MODE=list ;;
    --self-test) MODE=self-test ;;
    --root)      shift; ROOT="$1" ;;
    *) echo "usage: $0 [--list | --self-test] [--root DIR]" >&2; exit 2 ;;
  esac
  shift
done

# ── The self-test ────────────────────────────────────────────────────────────────────────────────
# Copies the scanned trees and the table into a scratch directory, plants one finding of each kind the gate
# exists for, and requires the gate to fail naming every one of them (and NOT naming a plant inside a test
# module); then requires the unplanted real tree to pass. A gate that has never been seen failing is not
# known to be a gate.
if [ "$MODE" = self-test ]; then
  cd "$ROOT"
  scratch=$(mktemp -d "${TMPDIR:-/tmp}/kv-ns-selftest.XXXXXX")
  trap 'case "$scratch" in */kv-ns-selftest.*) rm -rf -- "$scratch" ;; esac' EXIT
  for d in src mycelium-*/src; do
    mkdir -p "$scratch/$(dirname "$d")"
    cp -R "$d" "$scratch/$d"
  done
  mkdir -p "$scratch/scripts" "$scratch/docs/guide"
  cp scripts/kv-namespaces-nonkeys.txt "$scratch/scripts/"
  cp docs/guide/building-on-mycelium.md "$scratch/docs/guide/"

  # The plants. Each is a shape one of the five lint misses had.
  cat >> "$scratch/mycelium-core/src/writer.rs" <<'RS'

pub const PLANTED_CONST_KEY: &str = "zz-planted-const/";
pub fn planted_format(x: u32) -> String { format!("sys/zz-planted-sys/{x}") }
pub fn planted_next_line(x: u32) -> String {
    format!(
        "zz-planted-nextline/{x}")
}
#[cfg(test)]
mod planted_tests {
    const TEST_ONLY: &str = "zz-planted-testonly/";
}
RS
  printf '%s\n' 'pub fn planted_call(kv: &KvHandle) { kv.set("zz-planted-call/k", vec![]); }' \
    >> "$scratch/src/agent/intent.rs"
  # The regression itself: a row removed from the table (the 2026-10-10 miss was rows never added).
  grep -v '`sys/config/{param}`' src/lib.rs > "$scratch/src/lib.rs"
  # An allow-list entry that admits nothing, and one without a reason.
  printf '%s\n' 'zz-stale-entry/   # planted by the self-test' 'zz-noreason/' >> "$scratch/scripts/kv-namespaces-nonkeys.txt"

  set +e
  out=$("$SCRIPT" --root "$scratch" 2>&1)
  rc=$?
  set -e
  fails=0
  if [ "$rc" -eq 0 ]; then echo "self-test FAIL: the planted tree passed"; fails=1; fi
  for want in '"zz-planted-const/"' '"sys/zz-planted-sys/{x}"' '"zz-planted-nextline/{x}"' \
              '"zz-planted-call/k"' 'sys/config/' 'STALE zz-stale-entry/' 'NO REASON zz-noreason/'; do
    if ! printf '%s\n' "$out" | grep -qF -- "$want"; then
      echo "self-test FAIL: the planted tree's output does not name $want"; fails=1
    fi
  done
  if printf '%s\n' "$out" | grep -qF 'zz-planted-testonly'; then
    echo "self-test FAIL: a literal inside a #[cfg(test)] module was enumerated"; fails=1
  fi
  if [ "$fails" -ne 0 ]; then
    echo "--- the planted run's output ---"; printf '%s\n' "$out"; exit 1
  fi
  echo "self-test: the planted tree fails naming all 7 plants (exit $rc); the test-module plant is not named."
  if ! "$SCRIPT" --root "$ROOT" > /dev/null 2>&1; then
    echo "self-test FAIL: the real tree does not pass"; "$SCRIPT" --root "$ROOT"; exit 1
  fi
  echo "self-test: the real tree passes."
  exit 0
fi

cd "$ROOT"

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
if [ "$MODE" = gate ]; then
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
fi

# ── 2. Every KV prefix in production code has a row (or is a declared non-key) ───────────────────
NONKEYS="scripts/kv-namespaces-nonkeys.txt"

table_file=$(mktemp "${TMPDIR:-/tmp}/kv-ns-table.XXXXXX")
cand_file=$(mktemp "${TMPDIR:-/tmp}/kv-ns-cand.XXXXXX")
trap 'rm -f -- "$table_file" "$cand_file"' EXIT

# The table's namespaces: every backticked key in the first cell of each row of § KV namespace ownership,
# reduced the same way a literal is (top segment; under `sys/`, the second too).
awk '
  /^\/\/! ## KV namespace ownership/ { on = 1; next }
  on && /^\/\/! ## /                 { on = 0 }
  on && /^\/\/! \| `/ {
    cell = $0; sub(/^\/\/! \| /, "", cell)
    i = index(cell, " | "); if (i) cell = substr(cell, 1, i - 1)
    while (match(cell, /`[^`]+`/)) {
      tok = substr(cell, RSTART + 1, RLENGTH - 2); cell = substr(cell, RSTART + RLENGTH)
      n = split(tok, seg, "/")
      if (seg[1] == "sys" && n >= 2) print "sys/" seg[2] "/"; else print seg[1] "/"
    }
  }
' src/lib.rs | sort -u > "$table_file"

if [ ! -s "$table_file" ]; then
  echo "FAIL could not read any row of src/lib.rs § KV namespace ownership — the table moved or changed shape"
  exit 1
fi

kv_files() {
  find src mycelium-core/src $(ls -d mycelium-*/src | grep -v '^mycelium-gateway-free-tests/') -name '*.rs' \
    ! -path '*/tests/*' ! -name '*_tests.rs' ! -name 'tests.rs' ! -name 'test_util.rs' | sort -u
}

KV_CALLS='set|set_async|set_with_receipt|set_with_receipt_as|set_requiring_sync|retry_requiring_sync|retry_with_receipt|retry_with_receipt_as|prepare_write|delete|delete_async|get|scan_prefix|subscribe|subscribe_prefix|subscribe_prefix_with_predicate|kv_[a-z_]+|scan_kv_prefix|scan_prefix_kv_with_ts|subscribe_prefix_on_kv|make_gossip_update|make_gossip_update_stamped|publish_[a-z_]+|strip_prefix|starts_with|parse_cap_key_or_warn'

# file:line<TAB>literal<TAB>shape
# shellcheck disable=SC2046
awk -v calls="$KV_CALLS" '
  BEGIN {
    lit   = "\"[a-z][a-z0-9_.-]*/[^\"]*\""
    cdecl = "(const|static)[ \t]+[A-Za-z_0-9]+[ \t]*:[ \t]*&(\047static[ \t]+)?str[ \t]*="
    fmt   = "format!\\("
    call  = "(^|[^A-Za-z_0-9])(" calls ")\\("
  }
  function emit(l, shape) { print FILENAME ":" FNR "\t" substr(l, 2, length(l) - 2) "\t" shape }
  FNR == 1 { skip = 0; pending = "" }
  /^[[:space:]]*#\[cfg\((all\()?test[,)]/ { skip = 1 }
  skip && /^}/ { skip = 0; next }
  skip { next }
  /^[[:space:]]*\/\// { next }
  {
    line = $0
    # A shape whose literal opens the next line.
    if (pending != "") {
      if (match(line, "^[ \t]*" lit)) { m = substr(line, RSTART, RLENGTH); sub(/^[ \t]*/, "", m); emit(m, pending) }
      pending = ""
    }
    rest = line
    while (match(rest, "(" cdecl ")[ \t]*" lit)) {
      m = substr(rest, RSTART, RLENGTH); rest = substr(rest, RSTART + RLENGTH)
      emit(substr(m, index(m, "\"")), "const")
    }
    rest = line
    while (match(rest, fmt lit)) {
      m = substr(rest, RSTART, RLENGTH); rest = substr(rest, RSTART + RLENGTH)
      emit(substr(m, index(m, "\"")), "format")
    }
    rest = line
    while (match(rest, call "[^\")]*" lit)) {
      m = substr(rest, RSTART, RLENGTH); rest = substr(rest, RSTART + RLENGTH)
      emit(substr(m, index(m, "\"")), "call")
    }
    if (line ~ "(" cdecl ")[ \t]*$")      pending = "const"
    else if (line ~ fmt "[ \t]*$")        pending = "format"
    else if (line ~ call "[ \t]*$")       pending = "call"
  }
' $(kv_files) | sort -u > "$cand_file"

# Classify: row · non-key · UNCLASSIFIED; then the allow-list's own health (stale entries, missing reasons).
classified=$(awk -F'\t' -v mode="$MODE" '
  FILENAME == ARGV[1] { row[$1] = 1; next }
  FILENAME == ARGV[2] {
    raw = $0; sub(/^[ \t]+/, "", raw)
    if (raw == "" || raw ~ /^#/) next
    split(raw, f, /[ \t]+/); e = f[1]
    order[++n] = e
    if (raw !~ /#[ \t]*[^ \t]/) noreason[e] = 1
    next
  }
  {
    l = $2
    k = split(l, seg, "/")
    if (seg[1] == "sys") ns = "sys/" seg[2] "/"; else ns = seg[1] "/"
    if (ns in row || (seg[1] == "sys" && seg[2] == "")) { if (mode == "list") print "row\t" ns "\t" $1 "\t\"" l "\"\t" $3; next }
    hit = ""
    for (i = 1; i <= n; i++) if (index(l, order[i]) == 1) { hit = order[i]; break }
    if (hit != "") { used[hit] = 1; if (mode == "list") print "nonkey\t" hit "\t" $1 "\t\"" l "\"\t" $3; next }
    print "UNCLASSIFIED\t" ns "\t" $1 "\t\"" l "\"\t" $3
  }
  END {
    for (i = 1; i <= n; i++) {
      if (!(order[i] in used))  print "STALE " order[i] "\tadmits no enumerated literal"
      if (order[i] in noreason) print "NO REASON " order[i] "\tevery entry carries `# reason`"
    }
  }
' "$table_file" "$NONKEYS" "$cand_file")

if [ "$MODE" = list ]; then
  printf '%s\n' "$classified"
  exit 0
fi

unclassified=$(printf '%s\n' "$classified" | grep '^UNCLASSIFIED' || true)
listhealth=$(printf '%s\n' "$classified" | grep -E '^(STALE|NO REASON) ' || true)
ncand=$(wc -l < "$cand_file" | tr -d ' ')
nentries=$(grep -cE '^[[:space:]]*[^#[:space:]]' "$NONKEYS" || true)
if [ -n "$unclassified" ]; then
  echo "FAIL KV prefixes used in production code with no row in src/lib.rs § KV namespace ownership:"
  printf '%s\n' "$unclassified" | awk -F'\t' '{ printf "  %-28s %s  %s  (%s)\n", $2, $3, $4, $5 }'
  echo "  fix: add a row to the table (owner/purpose) — or, if the literal is not a KV key (a stream name, a"
  echo "  signal kind, a schema tag, a slot or ring name, a path), add it to $NONKEYS with its reason."
  status=1
fi
if [ -n "$listhealth" ]; then
  echo "FAIL $NONKEYS:"
  printf '%s\n' "$listhealth" | sed 's/^/  /'
  status=1
fi
if [ -z "$unclassified" ] && [ -z "$listhealth" ]; then
  echo "KV namespace-table sweep: clean ($ncand literals enumerated; $(wc -l < "$table_file" | tr -d ' ') table namespaces; $nentries non-key entries)."
fi

# ── 3. The reserved-prefix lists on the front door ───────────────────────────────────────────────
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

The KV namespace sweep failed. What each finding means:

* A forbidden prefix is not a naming rule. A prefix listed in FORBIDDEN is one a design record says must
  never carry state, because once it is in the gossip KV namespace it is replicated, anti-entropied and
  indistinguishable from state this mesh produced itself. If the state genuinely belongs in KV, amend the
  design record first, then remove the prefix from FORBIDDEN in the same PR.

* A prefix with no row is a namespace the substrate (or a companion) uses that `src/lib.rs` does not say it
  owns — so an adopter can write under it, and the front door's reserved lists cannot name it. Add the row.
  A literal that is not a KV key at all goes into scripts/kv-namespaces-nonkeys.txt, with the reason.
MSG
else
  echo "KV namespace sweep: clean."
fi

exit "$status"
