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
#   `CA_GENERATION_OID_IS_PROVISIONAL` is `true`, that function must ask `ca_generation_oid_open()` itself, before it
#   writes anything — so a shipped binary refuses by name and writes nothing (CI's
#   `scripts/check-shipped-ca-generation-refusal.sh` shows the binary doing so). A `commit_successor` without the
#   check fails this script in either mode: nothing new ships under a provisional OID. A function added later that
#   writes the extension is added to GATED_WRITERS below.
#
#   WHAT THIS CHECK IS, AND WHAT IT CANNOT SEE (#812's review F6). It is a **tripwire over text**, not the guard: it
#   requires, in the function's body with comments dropped, an `if !…ca_generation_oid_open()` ahead of the first line
#   that mints or writes (`mint_ca_at(`, `write_public(`, `write_private(`, `hard_link(`, `fs::write(`, `fs::rename(`),
#   whatever the function's visibility. It does not parse Rust: it cannot tell that the `if` returns rather than
#   logging and going on, it does not see a write reached through a helper it does not name, a gate inside a block
#   comment or a string would count, and a writer under another name is invisible until it is listed. **The guards
#   are elsewhere:** `mint_ca_at` (through `generate_ca`) refuses a generation above 0 by itself while the gate is
#   closed, every reader refuses the extension (`ca_generation`), and CI runs the shipped binary
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
  rm -f "$dir/blocked.rs" "$dir/blocked-gen.rs" "$dir/blocked-shown.rs" "$dir/clear.rs" "$dir/missing.rs" "$dir/missing-shown.rs" "$dir/ungated.rs" "$dir/gated.rs" "$dir/comment.rs" "$dir/late.rs" "$dir/crate.rs" "$dir/wrapped.rs" "$dir/wrapped-ungated.rs"
  rmdir "$dir"
  if [[ $ok == 1 ]]; then echo "check-release-blockers --self-test: ok (14 cases)"; else exit 1; fi
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
# test): a listed writer may delegate to another listed writer before it writes anything, and each is checked itself.
GATED_WRITERS=(commit_successor commit_successor_observed)
delegates=$(IFS='|'; echo "${GATED_WRITERS[*]}")
if grep -qE '^pub const CA_GENERATION_OID_IS_PROVISIONAL: bool = true;' "$file"; then
  for fn in "${GATED_WRITERS[@]}"; do
    # Whatever its visibility: `fn`, `pub fn`, `pub(crate) fn`.
    if grep -qE "^[[:space:]]*(pub(\([a-z]+\))? )?fn ${fn}\(" "$file"; then
      # The body runs from the function's line to the next `fn` item. Comments are dropped first, so a call named in
      # one does not count. The gate must be an `if !…ca_generation_oid_open()` — its answer acted on — and must come
      # before the first line that mints or writes.
      if ! awk -v fn="$fn" -v delegates="$delegates" '
            $0 ~ "^[[:space:]]*(pub(\\([a-z]+\\))? )?fn " fn "\\(" { inside = 1; next }
            inside && $0 ~ /^[[:space:]]*(pub(\([a-z]+\))? )?fn / { exit }
            inside {
              line = $0; sub(/\/\/.*/, "", line)
              if (!wrote && line ~ /if[[:space:]]+!.*ca_generation_oid_open\(\)/) gated = 1
              if (!wrote && line ~ ("(^|[^a-z_])(" delegates ")\\(") && line !~ ("fn " fn "\\(")) gated = 1
              if (line ~ /mint_ca_at\(|write_public\(|write_private\(|hard_link\(|fs::write\(|fs::rename\(/) wrote = 1
            }
            END { exit(gated ? 0 : 1) }' "$file"; then
        echo "::error::check-release-blockers: \`${fn}\` in $file writes the caGeneration extension and does not refuse on \`if !…ca_generation_oid_open()\` before its first mint or write: nothing new ships under a provisional OID (identity-and-lifecycle I2, decision (e))" >&2
        exit 1
      fi
    fi
  done
fi
if [[ $blocked == 1 ]]; then exit 1; fi
if [[ $warned == 1 ]]; then exit 0; fi
echo "check-release-blockers: no blocker"
