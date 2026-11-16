#!/usr/bin/env bash
# A binary built as it ships refuses to issue a consent certificate while the grade OID is provisional
# (identity-and-lifecycle I7; #800's review, check 4 and finding 2). The rule is unit-tested
# (`a_provisional_grade_oid_is_written_only_by_a_test_build`) and its wiring on a closed gate
# (`a_closed_provisional_gate_reads_no_consent_certificate_and_issues_none`), but a test build is a test build: only a
# binary built without `provisional-consent-grade` shows the shipped refusal. CI's Eve fleet job runs this against the
# release's NODE_FEATURES binary.
#
# USAGE: scripts/check-shipped-consent-refusal.sh <path to a mycelium binary built without provisional-consent-grade>
set -euo pipefail
bin="${1:?usage: $0 <mycelium binary>}"
if ! grep -qE '^pub const CONSENT_GRADE_OID_IS_PROVISIONAL: bool = true;' mycelium-core/src/tls.rs; then
  echo "check-shipped-consent-refusal: the grade OID is registered; nothing to refuse"
  exit 0
fi
work=$(mktemp -d)
cleanup() { rm -rf -- "${work:?}/ca" "${work:?}/out"; rm -f -- "${work:?}/k.pem" "${work:?}/pub.pem" "${work:?}/err"; rmdir -- "$work"; }
trap cleanup EXIT
"$bin" tls ca --out "$work/ca" >/dev/null 2>&1
openssl ecparam -name prime256v1 -genkey -noout -out "$work/k.pem" 2>/dev/null
openssl ec -in "$work/k.pem" -pubout -out "$work/pub.pem" 2>/dev/null
set +e
"$bin" tls issue --ca-dir "$work/ca" --client dev --consent --public-key "$work/pub.pem" --grade tty \
  --issuer fleet --out "$work/out" 2>"$work/err" >/dev/null
code=$?
set -e
if [[ $code -ne 2 ]] || ! grep -q "consent certificates are not issued by this build" "$work/err" || [[ -e "$work/out" ]]; then
  echo "::error::a shipped-configuration binary issued, or did not refuse by name, a consent certificate under the provisional OID (exit $code): $(cat "$work/err")"
  exit 1
fi
echo "check-shipped-consent-refusal: refused by name, exit 2, nothing written: $(cat "$work/err")"
