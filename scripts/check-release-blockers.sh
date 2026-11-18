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
# USAGE
#   scripts/check-release-blockers.sh [--release] [tls.rs path]
#     --release   a blocker is an error (exit 1); without it, a warning (exit 0)
#     path        the file to read (default mycelium-core/src/tls.rs) — the self-test passes planted copies
#   scripts/check-release-blockers.sh --self-test
set -euo pipefail

self_test() {
  local dir
  dir=$(mktemp -d)
  printf 'pub const CONSENT_GRADE_OID_IS_PROVISIONAL: bool = true;\npub const CA_GENERATION_OID_IS_PROVISIONAL: bool = false;\n' >"$dir/blocked.rs"
  printf 'pub const CONSENT_GRADE_OID_IS_PROVISIONAL: bool = false;\npub const CA_GENERATION_OID_IS_PROVISIONAL: bool = true;\n' >"$dir/blocked-gen.rs"
  printf 'pub const CONSENT_GRADE_OID_IS_PROVISIONAL: bool = false;\npub const CA_GENERATION_OID_IS_PROVISIONAL: bool = false;\n' >"$dir/clear.rs"
  printf 'pub const CONSENT_GRADE_OID_IS_PROVISIONAL: bool = false;\n' >"$dir/missing.rs"
  local ok=1
  if "$0" --release "$dir/blocked.rs" >/dev/null 2>&1; then echo "self-test: a provisional grade OID passed --release"; ok=0; fi
  if "$0" --release "$dir/blocked-gen.rs" >/dev/null 2>&1; then echo "self-test: a provisional generation OID passed --release"; ok=0; fi
  if ! "$0" "$dir/blocked.rs" >/dev/null 2>&1; then echo "self-test: a provisional OID failed without --release"; ok=0; fi
  if ! "$0" --release "$dir/clear.rs" >/dev/null 2>&1; then echo "self-test: registered OIDs failed --release"; ok=0; fi
  if "$0" --release "$dir/missing.rs" >/dev/null 2>&1; then echo "self-test: a file without a constant passed"; ok=0; fi
  local head='pub const CONSENT_GRADE_OID_IS_PROVISIONAL: bool = false;\npub const CA_GENERATION_OID_IS_PROVISIONAL: bool = true;\n'
  printf "${head}    pub fn commit_successor() {\n        mint();\n    }\n    pub fn next() { ca_generation_oid_open(); }\n" >"$dir/ungated.rs"
  printf "${head}    pub fn commit_successor() {\n        if !super::ca_generation_oid_open() { return; }\n        mint();\n    }\n" >"$dir/gated.rs"
  if "$0" "$dir/ungated.rs" >/dev/null 2>&1; then echo "self-test: an ungated commit_successor passed under a provisional OID"; ok=0; fi
  if ! "$0" "$dir/gated.rs" >/dev/null 2>&1; then echo "self-test: a gated commit_successor failed without --release"; ok=0; fi
  rm -f "$dir/blocked.rs" "$dir/blocked-gen.rs" "$dir/clear.rs" "$dir/missing.rs" "$dir/ungated.rs" "$dir/gated.rs"
  rmdir "$dir"
  if [[ $ok == 1 ]]; then echo "check-release-blockers --self-test: ok (7 cases)"; else exit 1; fi
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
for constant in CONSENT_GRADE_OID_IS_PROVISIONAL CA_GENERATION_OID_IS_PROVISIONAL; do
  line=$(grep -E "^pub const ${constant}: bool = (true|false);" "$file" || true)
  if [[ -z "$line" ]]; then
    echo "check-release-blockers: $file has no \`pub const ${constant}: bool = …;\` line — the check cannot read the blocker" >&2
    exit 1
  fi
  if [[ "$line" == *"= true;"* ]]; then
    msg="release blocked: ${constant} is true in $file — the OID is the documentation-PEN placeholder until the registered IANA PEN arc replaces it (identity-and-lifecycle I7 and I2; RELEASING.md § 2)"
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
GATED_WRITERS=(commit_successor)
if grep -qE '^pub const CA_GENERATION_OID_IS_PROVISIONAL: bool = true;' "$file"; then
  for fn in "${GATED_WRITERS[@]}"; do
    if grep -qE "^\s*pub fn ${fn}\(" "$file"; then
      # The function's body: from its `pub fn` line to the next `fn` at the same or an outer level.
      if ! awk -v fn="$fn" '
            $0 ~ "^[[:space:]]*pub fn " fn "\\(" { inside = 1; next }
            inside && $0 ~ /^[[:space:]]*(pub(\([a-z]+\))? )?fn / { exit }
            inside && /ca_generation_oid_open\(\)/ { found = 1 }
            END { exit(found ? 0 : 1) }' "$file"; then
        echo "::error::check-release-blockers: \`${fn}\` in $file writes the caGeneration extension and does not ask \`ca_generation_oid_open()\`: nothing new ships under a provisional OID (identity-and-lifecycle I2, decision (e))" >&2
        exit 1
      fi
    fi
  done
fi
if [[ $blocked == 1 ]]; then exit 1; fi
if [[ $warned == 1 ]]; then exit 0; fi
echo "check-release-blockers: no blocker"
