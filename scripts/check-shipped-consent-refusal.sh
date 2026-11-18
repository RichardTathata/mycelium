#!/usr/bin/env bash
# A binary built as it ships refuses to issue a consent certificate while the grade OID is provisional
# (identity-and-lifecycle I7; #800's review, check 4 and finding 2). The rule is unit-tested
# (`a_provisional_grade_oid_is_written_only_by_a_test_build`) and its wiring on a closed gate
# (`a_closed_provisional_gate_reads_no_consent_certificate_and_issues_none`), but a test build is a test build: only a
# binary built without `provisional-consent-grade` shows the shipped refusal. CI's Eve fleet job runs this against the
# release's NODE_FEATURES binary.
#
# Four shipped paths, each refused by name (I7's PR 2 added the second and third, PR 3a the fourth):
#   1. `tls issue --consent`: no consent certificate is issued, and nothing is written;
#   2. `gateway_consent_principals`: `mycelium doctor` — which says what `start()` would — refuses the table;
#   3. the grant decision door: a running node answers a consent principal's decision
#      `422 consent_grade_oid_provisional`, before it reads a key (so the body's key and signature are filler).
#   4. `dev consent` (I7's PR 3a): no consent key is created, no certificate enrolled and nothing signed — each verb
#      refuses by name before it resolves a directory, and nothing is written under the home it was given.
# The third starts a node on two loopback ports (CONSENT_CHECK_PORT, default 19461, and the next one up) and stops
# it by its PID.
#
# USAGE: scripts/check-shipped-consent-refusal.sh <path to a mycelium binary built without provisional-consent-grade>
set -euo pipefail
bin="${1:?usage: $0 <mycelium binary>}"
if ! grep -qE '^pub const CONSENT_GRADE_OID_IS_PROVISIONAL: bool = true;' mycelium-core/src/tls.rs; then
  echo "check-shipped-consent-refusal: the grade OID is registered; nothing to refuse"
  exit 0
fi
work=$(mktemp -d)
node_pid=""
cleanup() {
  if [[ -n "$node_pid" ]]; then kill "$node_pid" 2>/dev/null || true; wait "$node_pid" 2>/dev/null || true; fi
  rm -rf -- "${work:?}/ca" "${work:?}/out" "${work:?}/home"
  rm -f -- "${work:?}/k.pem" "${work:?}/pub.pem" "${work:?}/err" "${work:?}/table.toml" "${work:?}/doctor" \
    "${work:?}/node.toml" "${work:?}/node.log" "${work:?}/answer" "${work:?}/dev.err"
  rmdir -- "$work"
}
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

# 2. The consent principals table: `doctor` runs `start()`'s preflight and prints its refusal. The entry is a valid
#    one (it names its grade, as `validate()` requires), so the refusal is the build's, not a malformed table's.
port="${CONSENT_CHECK_PORT:-19461}"
http=$((port + 1))
printf 'bind_port = %s\nhttp_port = %s\n[[gateway_consent_principals]]\nname = "dev"\nscopes = ["grant:issue"]\nsigned_with = "tty"\n' "$port" "$http" > "$work/table.toml"
set +e
"$bin" doctor "$work/table.toml" > "$work/doctor" 2>&1
set -e
if ! grep -q "gateway_consent_principals" "$work/doctor" || ! grep -q "consent certificates are not read by this build" "$work/doctor"; then
  echo "::error::a shipped-configuration binary would start with a gateway_consent_principals table, or did not refuse it by name: $(cat "$work/doctor")"
  exit 1
fi
echo "check-shipped-consent-refusal: gateway_consent_principals refused at start by name: $(grep -m1 "consent certificates are not read by this build" "$work/doctor")"

# 3. The decision door. The decider is a consent principal; the key and signature are filler of the right lengths —
#    a build that reads no consent certificate must refuse before it looks at either.
printf 'bind_port = %s\nhttp_port = %s\ngateway_identity_issuer = "fleet"\n[[gateway_named_tokens]]\nname = "relay"\ntoken = "relay-secret"\nscopes = ["grant:relay"]\n' "$port" "$http" > "$work/node.toml"
"$bin" -c "$work/node.toml" > "$work/node.log" 2>&1 &
node_pid=$!
echo "check-shipped-consent-refusal: started a node, pid $node_pid, on 127.0.0.1:$port (gateway $http)"
up=0
for _ in $(seq 1 100); do
  if curl -fs -o /dev/null "http://127.0.0.1:$http/health"; then up=1; break; fi
  if ! kill -0 "$node_pid" 2>/dev/null; then break; fi
  sleep 0.2
done
if [[ $up -ne 1 ]]; then
  echo "::error::the node did not serve /health on 127.0.0.1:$http: $(tail -5 "$work/node.log")"
  exit 1
fi
k=$(head -c 65 /dev/zero | base64 | tr -d '\n')
sig=$(head -c 70 /dev/zero | base64 | tr -d '\n')
body=$(printf '{"decision":"issue","ttl_secs":60,"signed":{"decider":"consent:fleet/dev","node":"127.0.0.1:%s","agent":"token:fleet/forecaster","request":"r1","t":1,"k":"%s","sig":"%s","grade":"tty"}}' "$port" "$k" "$sig")
status=$(curl -s -o "$work/answer" -w '%{http_code}' -X POST -H 'Authorization: Bearer relay-secret' -H 'content-type: application/json' \
  --data "$body" "http://127.0.0.1:$http/gateway/grants/token%3Afleet%2Fforecaster/r1")
kill "$node_pid" 2>/dev/null || true
wait "$node_pid" 2>/dev/null || true
echo "check-shipped-consent-refusal: stopped the node, pid $node_pid"
node_pid=""
if [[ "$status" != "422" ]] || ! grep -q '"consent_grade_oid_provisional"' "$work/answer"; then
  echo "::error::a shipped-configuration node did not refuse a consent principal's grant decision by name (HTTP $status): $(cat "$work/answer")"
  exit 1
fi
echo "check-shipped-consent-refusal: a consent decision refused at the door, HTTP 422: $(cat "$work/answer")"

# 4. `dev consent`: the developer's side. Each verb is given a home of its own, which must stay absent: a build that
#    reads no consent certificate creates no consent key, keeps no certificate and signs nothing.
for verb in "key" "enrol --cert $work/pub.pem" "approve r1"; do
  set +e
  # shellcheck disable=SC2086  # the verb's own arguments are meant to split
  HOME="$work/home" XDG_DATA_HOME="" "$bin" dev consent $verb --issuer fleet --name dev 2>"$work/dev.err" >/dev/null
  code=$?
  set -e
  if [[ $code -ne 2 ]] || ! grep -q "consent_grade_oid_provisional" "$work/dev.err" \
     || ! grep -q "consent keys are not created or used by this build" "$work/dev.err" || [[ -e "$work/home" ]]; then
    echo "::error::a shipped-configuration binary ran \`dev consent ${verb%% *}\`, or did not refuse it by name (exit $code): $(cat "$work/dev.err")"
    exit 1
  fi
done
echo "check-shipped-consent-refusal: dev consent key, enrol and approve refused by name, exit 2, nothing written: $(cat "$work/dev.err")"
