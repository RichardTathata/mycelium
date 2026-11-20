#!/usr/bin/env bash
# Release blockers that a constant in the tree states (identity-and-lifecycle I7, #800's review finding 1).
#
# WHY IT EXISTS
#   Two provisional OIDs, one rule (the maintainer's decision (e), 2026-11-15): the consent grade extension's
#   (`CONSENT_GRADE_OID`, I7) and the CA generation extension's (`CA_GENERATION_OID`, identity-and-lifecycle I2, PR 2a),
#   each beside its `…_IS_PROVISIONAL` constant in mycelium-core/src/tls.rs. Each is checked alike.
#   The consent grade extension's OID (`CONSENT_GRADE_OID` in mycelium-core/src/tls.rs) is provisional until the
#   registered IANA Private Enterprise Number arc arrives (the maintainer, 2026-11-15). A certificate that carries an
#   OID is permanent, so no release may ship while `CONSENT_GRADE_OID_IS_PROVISIONAL` is `true`. The plan said so in
#   prose; this makes it mechanical. The release workflow's `version` job runs it on every tag push (`--release`:
#   a blocker fails the job); on a dry run it only warns, so pull requests stay green.
#
#   A GATED WRITER (identity-and-lifecycle I2, PR 2b-1). `mycelium tls commit` (`commit_successor`) is a new path
#   that would write the generation extension: a successor is a generation-1 CA. While
#   `CA_GENERATION_OID_IS_PROVISIONAL` is `true`, that function must ask `ca_generation_oid_open()` before it
#   writes anything (itself, or through the one delegation THE WRITER RULE allows) — so a shipped binary refuses by
#   name and writes nothing (CI's `scripts/check-shipped-ca-generation-refusal.sh` shows the binary doing so). A
#   `commit_successor` without the check fails this script in either mode: nothing new ships under a provisional
#   OID. A function added later that writes the extension is added to GATED_WRITERS below.
#
#   THE WRITER RULE (narrowed after the independent review of #812, which planted four shapes that passed). While
#   the constant is `true`, every definition of a listed writer must hold the gate itself — an
#   `if !…ca_generation_oid_open()` ahead of its first line that mints or writes (`mint_ca_at(`, `write_public(`,
#   `write_private(`, `hard_link(`, `fs::write(`, `fs::rename(`) — with ONE exception: `commit_successor` may
#   delegate, only to `commit_successor_observed`, only when its own body has no mint or write line, and only when
#   the file defines `commit_successor_observed` and every definition of it holds the gate on its own. No other
#   writer may delegate, so two writers that hand to each other both fail, and a writer that delegates and then
#   writes fails.
#
#   WHAT THIS CHECK IS, AND WHAT IT CANNOT SEE (#812's review F6). It is a **tripwire over text**, not the guard.
#   What it sees: each line with its comments dropped (`//`, nested `/* */`) and the inside of its string literals
#   (plain, byte, raw, across lines) and char literals emptied, so a gate, a call or a write named in a comment or a
#   string counts for nothing; every definition of a listed name, whatever its visibility or qualifiers (`pub(…)`,
#   `const`, `async`, `unsafe`, `extern "…"`, generics), each followed by brace depth to its closing brace and each
#   checked. What it does not see: it does not parse Rust, so it cannot tell that the `if` returns rather than
#   logging and going on, or that `if !x || ca_generation_oid_open()` is no gate; it does not see a write reached
#   through a helper it does not name, nor a macro that expands to one; a listed writer missing from the file, or a
#   writer under another name, is invisible until it is listed (the list is not checked against the code); its
#   scanner is approximate (an unusual literal it misreads could hide what follows). **The guards are elsewhere:**
#   `mint_ca_at` (through `generate_ca`) refuses a generation above 0 by itself while the gate is closed, every reader
#   refuses the extension (`ca_generation`), and CI runs the shipped binary
#   (`scripts/check-shipped-ca-generation-refusal.sh`). This check only makes removing the early refusal a red build.
#
#   A third blocker, not an OID (the maintainer's decision of 2026-11-18; #813's review, F1):
#   `CONSENT_DECISION_DOES_NOT_NAME_WHAT_WAS_SHOWN`, in the same file. A consent decision's signature does not name
#   the tool, resource or digest the approver was shown, so an approval can be moved to another call and replayed.
#   No release may carry consent decisions while it is `true`; the PR that makes the approver sign what it saw
#   (the plan's row I7-S) sets it `false`.
#
# USAGE
#   scripts/check-release-blockers.sh [--release] [tls.rs path]
#     --release   a blocker is an error (exit 1); without it, a warning (exit 0)
#     path        the file to read (default mycelium-core/src/tls.rs) — the self-test passes planted copies
#   scripts/check-release-blockers.sh --self-test
set -euo pipefail

self_test() {
  local dir
  dir=$(mktemp -d)
  local shown_clear='pub const CONSENT_DECISION_DOES_NOT_NAME_WHAT_WAS_SHOWN: bool = false;\n'
  printf "pub const CONSENT_GRADE_OID_IS_PROVISIONAL: bool = true;\npub const CA_GENERATION_OID_IS_PROVISIONAL: bool = false;\n$shown_clear" >"$dir/blocked.rs"
  printf "pub const CONSENT_GRADE_OID_IS_PROVISIONAL: bool = false;\npub const CA_GENERATION_OID_IS_PROVISIONAL: bool = true;\n$shown_clear" >"$dir/blocked-gen.rs"
  printf "pub const CONSENT_GRADE_OID_IS_PROVISIONAL: bool = false;\npub const CA_GENERATION_OID_IS_PROVISIONAL: bool = false;\n$shown_clear" >"$dir/clear.rs"
  printf 'pub const CONSENT_GRADE_OID_IS_PROVISIONAL: bool = false;\npub const CA_GENERATION_OID_IS_PROVISIONAL: bool = false;\npub const CONSENT_DECISION_DOES_NOT_NAME_WHAT_WAS_SHOWN: bool = true;\n' >"$dir/blocked-shown.rs"
  printf 'pub const CONSENT_GRADE_OID_IS_PROVISIONAL: bool = false;\n' >"$dir/missing.rs"
  printf 'pub const CONSENT_GRADE_OID_IS_PROVISIONAL: bool = false;\npub const CA_GENERATION_OID_IS_PROVISIONAL: bool = false;\n' >"$dir/missing-shown.rs"
  local ok=1
  if "$0" --release "$dir/blocked.rs" >/dev/null 2>&1; then echo "self-test: a provisional grade OID passed --release"; ok=0; fi
  if "$0" --release "$dir/blocked-gen.rs" >/dev/null 2>&1; then echo "self-test: a provisional generation OID passed --release"; ok=0; fi
  if "$0" --release "$dir/blocked-shown.rs" >/dev/null 2>&1; then echo "self-test: a consent decision that does not name what was shown passed --release"; ok=0; fi
  if ! "$0" "$dir/blocked.rs" >/dev/null 2>&1; then echo "self-test: a provisional OID failed without --release"; ok=0; fi
  if ! "$0" --release "$dir/clear.rs" >/dev/null 2>&1; then echo "self-test: registered OIDs failed --release"; ok=0; fi
  if "$0" --release "$dir/missing.rs" >/dev/null 2>&1; then echo "self-test: a file without a constant passed"; ok=0; fi
  if "$0" --release "$dir/missing-shown.rs" >/dev/null 2>&1; then echo "self-test: a file without the third constant passed"; ok=0; fi
  # The writer cases: every constant present, so that what fails or passes is the writer's gate.
  local head='pub const CONSENT_GRADE_OID_IS_PROVISIONAL: bool = false;\npub const CA_GENERATION_OID_IS_PROVISIONAL: bool = true;\npub const CONSENT_DECISION_DOES_NOT_NAME_WHAT_WAS_SHOWN: bool = false;\n'
  printf "${head}    pub fn commit_successor() {\n        mint();\n    }\n    pub fn next() { ca_generation_oid_open(); }\n" >"$dir/ungated.rs"
  printf "${head}    pub fn commit_successor() {\n        if !super::ca_generation_oid_open() { return; }\n        mint();\n    }\n" >"$dir/gated.rs"
  if "$0" "$dir/ungated.rs" >/dev/null 2>&1; then echo "self-test: an ungated commit_successor passed under a provisional OID"; ok=0; fi
  if ! "$0" "$dir/gated.rs" >/dev/null 2>&1; then echo "self-test: a gated commit_successor failed without --release"; ok=0; fi
  # #812's review F6: the gate named only in a comment, the gate asked after the publish with its answer discarded,
  # and an ungated writer that is `pub(crate)`.
  printf "${head}    pub fn commit_successor() {\n        // if !super::ca_generation_oid_open() { return; }\n        mint_ca_at();\n    }\n" >"$dir/comment.rs"
  printf "${head}    pub fn commit_successor() {\n        mint_ca_at();\n        hard_link();\n        let _ = super::ca_generation_oid_open();\n        if !super::ca_generation_oid_open() { return; }\n    }\n" >"$dir/late.rs"
  printf "${head}    pub(crate) fn commit_successor() {\n        mint_ca_at();\n    }\n" >"$dir/crate.rs"
  if "$0" "$dir/comment.rs" >/dev/null 2>&1; then echo "self-test: a gate named only in a comment passed"; ok=0; fi
  if "$0" "$dir/late.rs" >/dev/null 2>&1; then echo "self-test: a gate asked after the mint and the publish passed"; ok=0; fi
  if "$0" "$dir/crate.rs" >/dev/null 2>&1; then echo "self-test: an ungated pub(crate) commit_successor passed"; ok=0; fi
  # A wrapper that hands to the writer holding the gate passes; one that hands to an ungated writer does not.
  printf "${head}    pub fn commit_successor() {\n        commit_successor_observed()\n    }\n    pub(super) fn commit_successor_observed() {\n        if !super::ca_generation_oid_open() { return; }\n        mint_ca_at();\n    }\n" >"$dir/wrapped.rs"
  printf "${head}    pub fn commit_successor() {\n        commit_successor_observed()\n    }\n    pub(super) fn commit_successor_observed() {\n        mint_ca_at();\n    }\n" >"$dir/wrapped-ungated.rs"
  if ! "$0" "$dir/wrapped.rs" >/dev/null 2>&1; then echo "self-test: a wrapper over a gated writer failed"; ok=0; fi
  if "$0" "$dir/wrapped-ungated.rs" >/dev/null 2>&1; then echo "self-test: a wrapper over an ungated writer passed"; ok=0; fi
  # The review of the delegation rule (after #812): only `commit_successor` delegates, only to
  # `commit_successor_observed`, only when it writes nothing itself, and the delegate holds the gate on its own; a name
  # inside a string or a comment is neither a call nor a gate. Each case below must fail except M; M and N pin the
  # scanner's reading of literals.
  local gated_observed=$'    pub(super) fn commit_successor_observed() {\n        if !super::ca_generation_oid_open() { return; }\n        mint_ca_at();\n    }\n'
  plant() { printf '%b%s' "$head" "$2" >"$dir/$1"; }
  # A: the wrapper calls the delegate, discards its answer, and mints ungated itself.
  plant wrap-writes.rs $'    pub fn commit_successor() {\n        let _ = commit_successor_observed();\n        mint_ca_at();\n    }\n'"$gated_observed"
  # C: the delegate is ungated and names `commit_successor()` only in a log string.
  plant observed-string.rs $'    pub fn commit_successor() {\n        commit_successor_observed()\n    }\n    pub(super) fn commit_successor_observed() {\n        log("commit_successor()");\n        mint_ca_at();\n    }\n'
  # D: each hands to the other, so neither holds the gate.
  plant mutual.rs $'    pub fn commit_successor() {\n        commit_successor_observed()\n    }\n    pub(super) fn commit_successor_observed() {\n        commit_successor();\n        mint_ca_at();\n    }\n'
  # F: the wrapper is ungated and names `commit_successor_observed()` only in a string.
  plant wrap-string.rs $'    pub fn commit_successor() {\n        log("commit_successor_observed()");\n        mint_ca_at();\n    }\n'"$gated_observed"
  # G: the gate only inside a string literal.
  plant gate-string.rs $'    pub fn commit_successor() {\n        let s = "if !ca_generation_oid_open() {";\n        mint_ca_at();\n    }\n'
  # H: the gate only inside a block comment, over two lines.
  plant gate-block.rs $'    pub fn commit_successor() {\n        /* if !super::ca_generation_oid_open() {\n           return; } */\n        mint_ca_at();\n    }\n'
  # I: a gated definition for one build and an ungated one for another: every definition is checked.
  plant two-defs.rs $'    #[cfg(test)]\n    pub fn commit_successor() {\n        if !super::ca_generation_oid_open() { return; }\n        mint_ca_at();\n    }\n    #[cfg(not(test))]\n    pub fn commit_successor() {\n        mint_ca_at();\n    }\n'
  # K: the wrapper hands to a delegate that is not in the file, so nothing shows it holds the gate.
  plant wrap-absent.rs $'    pub fn commit_successor() {\n        commit_successor_observed()\n    }\n'
  # L: an ungated writer declared `async`, invisible to a `[pub] fn` pattern.
  plant async.rs $'    pub async fn commit_successor() {\n        mint_ca_at();\n    }\n'
  # M (must pass): a string over two lines that names a write, then the gate, then the write.
  plant multiline-ok.rs $'    pub fn commit_successor() {\n        let m = "mint_ca_at( is named \\\n            here";\n        if !super::ca_generation_oid_open() { return; }\n        mint_ca_at();\n    }\n'
  # N (must fail): a `\'"\'` char literal does not open a string that would hide the mint from the late gate.
  plant char-quote.rs $'    pub fn commit_successor<\'a>(x: &\'a str) {\n        let q = \'"\';\n        mint_ca_at();\n        let r = \'"\';\n        if !super::ca_generation_oid_open() { return; }\n    }\n'
  if "$0" "$dir/wrap-writes.rs" >/dev/null 2>&1; then echo "self-test: (A) a wrapper that minted after delegating passed"; ok=0; fi
  if "$0" "$dir/observed-string.rs" >/dev/null 2>&1; then echo "self-test: (C) an ungated delegate naming the wrapper in a string passed"; ok=0; fi
  if "$0" "$dir/mutual.rs" >/dev/null 2>&1; then echo "self-test: (D) two writers handing to each other passed"; ok=0; fi
  if "$0" "$dir/wrap-string.rs" >/dev/null 2>&1; then echo "self-test: (F) an ungated wrapper naming the delegate in a string passed"; ok=0; fi
  if "$0" "$dir/gate-string.rs" >/dev/null 2>&1; then echo "self-test: (G) a gate inside a string literal passed"; ok=0; fi
  if "$0" "$dir/gate-block.rs" >/dev/null 2>&1; then echo "self-test: (H) a gate inside a block comment passed"; ok=0; fi
  if "$0" "$dir/two-defs.rs" >/dev/null 2>&1; then echo "self-test: (I) an ungated second definition passed"; ok=0; fi
  if "$0" "$dir/wrap-absent.rs" >/dev/null 2>&1; then echo "self-test: (K) a wrapper over a delegate not in the file passed"; ok=0; fi
  if "$0" "$dir/async.rs" >/dev/null 2>&1; then echo "self-test: (L) an ungated async commit_successor passed"; ok=0; fi
  if ! "$0" "$dir/multiline-ok.rs" >/dev/null 2>&1; then echo "self-test: (M) a gated writer with a write named in a two-line string failed"; ok=0; fi
  if "$0" "$dir/char-quote.rs" >/dev/null 2>&1; then echo "self-test: (N) a late gate behind a quote char literal passed"; ok=0; fi
  rm -f "$dir/blocked.rs" "$dir/blocked-gen.rs" "$dir/blocked-shown.rs" "$dir/clear.rs" "$dir/missing.rs" "$dir/missing-shown.rs" "$dir/ungated.rs" "$dir/gated.rs" "$dir/comment.rs" "$dir/late.rs" "$dir/crate.rs" "$dir/wrapped.rs" "$dir/wrapped-ungated.rs" \
    "$dir/wrap-writes.rs" "$dir/observed-string.rs" "$dir/mutual.rs" "$dir/wrap-string.rs" "$dir/gate-string.rs" "$dir/gate-block.rs" "$dir/two-defs.rs" "$dir/wrap-absent.rs" "$dir/async.rs" "$dir/multiline-ok.rs" "$dir/char-quote.rs"
  rmdir "$dir"
  if [[ $ok == 1 ]]; then echo "check-release-blockers --self-test: ok (25 cases)"; else exit 1; fi
}

release=0
file=mycelium-core/src/tls.rs
for a in "$@"; do
  case "$a" in
    --self-test) self_test; exit 0 ;;
    --release) release=1 ;;
    *) file="$a" ;;
  esac
done

blocked=0
warned=0
for constant in CONSENT_GRADE_OID_IS_PROVISIONAL CA_GENERATION_OID_IS_PROVISIONAL CONSENT_DECISION_DOES_NOT_NAME_WHAT_WAS_SHOWN; do
  line=$(grep -E "^pub const ${constant}: bool = (true|false);" "$file" || true)
  if [[ -z "$line" ]]; then
    echo "check-release-blockers: $file has no \`pub const ${constant}: bool = …;\` line — the check cannot read the blocker" >&2
    exit 1
  fi
  if [[ "$line" == *"= true;"* ]]; then
    if [[ "$constant" == CONSENT_DECISION_DOES_NOT_NAME_WHAT_WAS_SHOWN ]]; then
      msg="release blocked: ${constant} is true in $file — a consent decision's signature does not name the tool, resource or digest the approver was shown, so an approval can be moved to another call and replayed (identity-and-lifecycle I7-S: the approver signs what it saw; RELEASING.md § 2)"
    else
      msg="release blocked: ${constant} is true in $file — the OID is the documentation-PEN placeholder until the registered IANA PEN arc replaces it (identity-and-lifecycle I7 and I2; RELEASING.md § 2)"
    fi
    if [[ $release == 1 ]]; then
      echo "::error::$msg"
      blocked=1
    else
      echo "check-release-blockers: warning: $msg"
      warned=1
    fi
  fi
done
# Writers of the generation extension must be gated while its OID is provisional (see A GATED WRITER above).
# `commit_successor` hands straight to `commit_successor_observed`, which holds the gate (and reports its steps to a
# test). That is the one delegation the check accepts (THE WRITER RULE above): every other listed writer holds the gate
# itself.
GATED_WRITERS=(commit_successor commit_successor_observed)
WRAPPER=commit_successor
DELEGATE=commit_successor_observed
if grep -qE '^pub const CA_GENERATION_OID_IS_PROVISIONAL: bool = true;' "$file"; then
  # One pass over the file. Each line is first cleaned: comments (`//`, nested `/* */`) dropped, the inside of string
  # literals (plain, byte and raw, across lines) and char literals emptied, so nothing named inside one counts. Then
  # every definition of a listed writer — `fn NAME(` or `fn NAME<`, after any of `pub`, `pub(…)`, `const`, `async`,
  # `unsafe`, `extern "…"` — is followed by brace depth to its closing brace, and one record printed per definition:
  #   NAME GATED WROTE CALLS
  # GATED: an `if !…ca_generation_oid_open()` before the first line that mints or writes; WROTE: any such line;
  # CALLS: a call of DELEGATE (`DELEGATE(`), counted only in a definition of WRAPPER.
  records=$(awk -v names="$(IFS='|'; echo "${GATED_WRITERS[*]}")" -v wrapper="$WRAPPER" -v delegate="$DELEGATE" '
    function clean(s,   out, i, n, c, nx, j, h, k, pre) {
      out = ""; n = length(s); i = 1
      while (i <= n) {
        c = substr(s, i, 1); nx = substr(s, i + 1, 1)
        if (blk > 0) {
          if (c == "*" && nx == "/") { blk--; i += 2; continue }
          if (c == "/" && nx == "*") { blk++; i += 2; continue }
          i++; continue
        }
        if (str) {
          if (c == "\\") { i += 2; continue }
          if (c == "\"") { str = 0; out = out "\"" }
          i++; continue
        }
        if (raw >= 0) {
          if (c == "\"") {
            for (k = 1; k <= raw && substr(s, i + k, 1) == "#"; k++) {}
            if (k > raw) { out = out "\""; i += 1 + raw; raw = -1; continue }
          }
          i++; continue
        }
        if (c == "/" && nx == "/") break
        if (c == "/" && nx == "*") { blk = 1; i += 2; continue }
        if (c == "\"") { str = 1; out = out "\""; i++; continue }
        pre = (i == 1) ? "" : substr(s, i - 1, 1)
        if (c == "b" && nx == "r" && pre !~ /[A-Za-z0-9_]/) { out = out c; i++; continue }
        if (c == "r" && (pre !~ /[A-Za-z0-9_]/ || (pre == "b" && (i == 2 || substr(s, i - 2, 1) !~ /[A-Za-z0-9_]/)))) {
          j = i + 1; h = 0
          while (substr(s, j, 1) == "#") { h++; j++ }
          if (substr(s, j, 1) == "\"") { raw = h; out = out "\""; i = j + 1; continue }
        }
        if (c == "\047") {
          if (nx == "\\") {
            j = index(substr(s, i + 3), "\047")
            if (j > 0) { out = out "\047 \047"; i = i + 3 + j; continue }
          } else if (substr(s, i + 2, 1) == "\047") { out = out "\047 \047"; i += 3; continue }
        }
        out = out c; i++
      }
      return out
    }
    BEGIN { raw = -1; decl = "^[[:space:]]*(pub(\\([^)]*\\))?[[:space:]]+)?((const|async|unsafe|extern[[:space:]]+\"[^\"]*\")[[:space:]]+)*fn[[:space:]]+(" names ")[(<]" }
    {
      line = clean($0)
      if (!inside && line ~ decl) {
        fname = line; sub(/^.*fn[[:space:]]+/, "", fname); sub(/[(<].*$/, "", fname)
        inside = 1; opened = 0; depth = 0; gated = 0; wrote = 0; calls = 0
      }
      if (!inside) next
      if (!wrote && line ~ /(^|[^A-Za-z0-9_])if[[:space:]]+!.*ca_generation_oid_open[[:space:]]*\(\)/) gated = 1
      if (line ~ /(^|[^A-Za-z0-9_])(mint_ca_at|write_public|write_private|hard_link|fs::write|fs::rename)[[:space:]]*\(/) wrote = 1
      if (fname == wrapper && line ~ ("(^|[^A-Za-z0-9_])" delegate "[[:space:]]*\\(")) calls = 1
      t = line; o = gsub(/\{/, "", t); t = line; cl = gsub(/\}/, "", t)
      if (o > 0) opened = 1
      depth += o - cl
      # A declaration without a body (`fn NAME(…);`) is a definition with nothing gated.
      if ((opened && depth <= 0) || (!opened && line ~ /;[[:space:]]*$/)) { print fname, gated, wrote, calls; inside = 0 }
    }
    END { if (inside) print fname, gated, wrote, calls }' "$file")
  refuse() {
    echo "::error::check-release-blockers: \`$1\` in $file $2: nothing new ships under a provisional OID (identity-and-lifecycle I2, decision (e))" >&2
    exit 1
  }
  delegate_defs=0
  delegate_gated=1
  while read -r name gated wrote calls; do
    [[ -z "$name" ]] && continue
    if [[ "$name" == "$DELEGATE" ]]; then
      delegate_defs=$((delegate_defs + 1))
      [[ $gated == 1 ]] || delegate_gated=0
    fi
  done <<<"$records"
  while read -r name gated wrote calls; do
    [[ -z "$name" ]] && continue
    [[ $gated == 1 ]] && continue
    if [[ "$name" == "$WRAPPER" && $wrote == 0 && $calls == 1 ]]; then
      if [[ $delegate_defs -gt 0 && $delegate_gated == 1 ]]; then continue; fi
      refuse "$name" "hands to \`$DELEGATE\`, and no definition of \`$DELEGATE\` in the file refuses on \`if !…ca_generation_oid_open()\` before its first mint or write"
    fi
    if [[ "$name" == "$WRAPPER" && $wrote == 0 ]]; then
      refuse "$name" "neither refuses on \`if !…ca_generation_oid_open()\` before its first mint or write nor hands to \`$DELEGATE\`"
    fi
    refuse "$name" "writes the caGeneration extension and does not refuse on \`if !…ca_generation_oid_open()\` before its first mint or write"
  done <<<"$records"
fi
if [[ $blocked == 1 ]]; then exit 1; fi
if [[ $warned == 1 ]]; then exit 0; fi
echo "check-release-blockers: no blocker"
