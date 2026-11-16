#!/usr/bin/env bash
# Release blockers that a constant in the tree states (identity-and-lifecycle I7, #800's review finding 1).
#
# WHY IT EXISTS
#   The consent grade extension's OID (`CONSENT_GRADE_OID` in mycelium-core/src/tls.rs) is provisional until the
#   registered IANA Private Enterprise Number arc arrives (the maintainer, 2026-11-15). A certificate that carries an
#   OID is permanent, so no release may ship while `CONSENT_GRADE_OID_IS_PROVISIONAL` is `true`. The plan said so in
#   prose; this makes it mechanical. The release workflow's `version` job runs it on every tag push (`--release`:
#   a blocker fails the job); on a dry run it only warns, so pull requests stay green.
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
  printf 'pub const CONSENT_GRADE_OID_IS_PROVISIONAL: bool = true;\n' >"$dir/blocked.rs"
  printf 'pub const CONSENT_GRADE_OID_IS_PROVISIONAL: bool = false;\n' >"$dir/clear.rs"
  printf 'pub const SOMETHING_ELSE: bool = true;\n' >"$dir/missing.rs"
  local ok=1
  if "$0" --release "$dir/blocked.rs" >/dev/null 2>&1; then echo "self-test: a provisional OID passed --release"; ok=0; fi
  if ! "$0" "$dir/blocked.rs" >/dev/null 2>&1; then echo "self-test: a provisional OID failed without --release"; ok=0; fi
  if ! "$0" --release "$dir/clear.rs" >/dev/null 2>&1; then echo "self-test: a registered OID failed --release"; ok=0; fi
  if "$0" --release "$dir/missing.rs" >/dev/null 2>&1; then echo "self-test: a file without the constant passed"; ok=0; fi
  rm -f "$dir/blocked.rs" "$dir/clear.rs" "$dir/missing.rs"
  rmdir "$dir"
  if [[ $ok == 1 ]]; then echo "check-release-blockers --self-test: ok (4 cases)"; else exit 1; fi
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

line=$(grep -E '^pub const CONSENT_GRADE_OID_IS_PROVISIONAL: bool = (true|false);' "$file" || true)
if [[ -z "$line" ]]; then
  echo "check-release-blockers: $file has no \`pub const CONSENT_GRADE_OID_IS_PROVISIONAL: bool = …;\` line — the check cannot read the blocker" >&2
  exit 1
fi
if [[ "$line" == *"= true;"* ]]; then
  msg="release blocked: CONSENT_GRADE_OID_IS_PROVISIONAL is true in $file — the consent grade extension's OID is the documentation-PEN placeholder until the registered IANA PEN arc replaces it (identity-and-lifecycle I7; RELEASING.md § 2)"
  if [[ $release == 1 ]]; then
    echo "::error::$msg"
    exit 1
  fi
  echo "check-release-blockers: warning: $msg"
  exit 0
fi
echo "check-release-blockers: no blocker"
