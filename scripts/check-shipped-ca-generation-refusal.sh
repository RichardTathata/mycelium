#!/usr/bin/env bash
# A binary built as it ships refuses, by name, a CA carrying the `caGeneration` extension while that extension's OID is
# provisional (identity-and-lifecycle I2, PR 2a; the maintainer's decision (e), I7's #800 precedent). The rule is
# unit-tested (`a_closed_provisional_gate_reads_no_ca_generation_and_mints_none`), but a test build is a test build:
# only a binary built without `provisional-ca-generation` shows the shipped refusal. Anyone holding a CA key can write
# the extension with openssl, so this writes one that way and asks the binary to issue under it: the issuer reads its
# CA as every node reads its roots (`ca_cert_to_der`), and must refuse naming the OID and write nothing. CI's Eve fleet
# job runs this against the release's NODE_FEATURES binary.
#
# USAGE: scripts/check-shipped-ca-generation-refusal.sh <path to a mycelium binary built without provisional-ca-generation>
set -euo pipefail
bin="${1:?usage: $0 <mycelium binary>}"
if ! grep -qE '^pub const CA_GENERATION_OID_IS_PROVISIONAL: bool = true;' mycelium-core/src/tls.rs; then
  echo "check-shipped-ca-generation-refusal: the generation OID is registered; nothing to refuse"
  exit 0
fi
work=$(mktemp -d)
cleanup() { rm -rf -- "${work:?}/ca" "${work:?}/out"; rm -f -- "${work:?}/err"; rmdir -- "$work"; }
trap cleanup EXIT
mkdir -p "$work/ca"
openssl genpkey -algorithm ed25519 -out "$work/ca/ca-key.pem" 2>/dev/null
openssl req -x509 -new -key "$work/ca/ca-key.pem" -subj "/CN=Mycelium fleet 0000000000000007 CA generation 1" -days 3650 \
  -addext "basicConstraints=critical,CA:TRUE" -addext "1.3.6.1.4.1.32473.2.1=DER:02:01:01" -out "$work/ca/ca-cert.pem" 2>/dev/null
set +e
"$bin" tls issue --ca-dir "$work/ca" --node 127.0.0.1:7960 --out "$work/out" 2>"$work/err" >/dev/null
code=$?
set -e
if [[ $code -eq 0 ]] || ! grep -q "1.3.6.1.4.1.32473.2.1" "$work/err" || ! grep -q "provisional" "$work/err" || [[ -e "$work/out" ]]; then
  echo "::error::a shipped-configuration binary issued under, or did not refuse by name, a CA carrying the provisional caGeneration OID (exit $code): $(cat "$work/err")"
  exit 1
fi
echo "check-shipped-ca-generation-refusal: refused by name, exit $code, nothing written: $(cat "$work/err")"
