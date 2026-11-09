#!/usr/bin/env bash
# The published stem's feature set, checked on a stem built with it and without dev-dependencies (#749's review,
# finding 2). `mycelium-wasm-host`'s dev-dependencies turn mycelium's `compliance` on for every target `cargo test`
# builds, so `tests/stem_compliance.rs` passes whatever STEM_FEATURES says. This runs a `cargo build` binary:
#
#   cargo build -p mycelium-wasm-host --bin mycelium-stem --features "$STEM_FEATURES"   # release.yml's set
#   scripts/check-release-stem.sh target/debug/mycelium-stem
#
# It starts the stem with a named-token table, [persistence] and [authority], and fails unless an anonymous
# `GET /gateway/guarantees` answers 401 and, with the token, the gateway and authority guarantees read `enforced`.
# A stem built without `compliance` refuses the configuration at start, and this fails naming the refusal.
set -euo pipefail

stem="${1:?usage: check-release-stem.sh <path to mycelium-stem>}"
[ -x "$stem" ] || { echo "check-release-stem: $stem is not an executable" >&2; exit 2; }

dir=$(mktemp -d "${TMPDIR:-/tmp}/release-stem.XXXXXX")
[ -n "$dir" ] && [ -d "$dir" ] || { echo "check-release-stem: no scratch directory" >&2; exit 2; }
pid=""
cleanup() {
  if [ -n "$pid" ]; then kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; fi
  case "$dir" in "${TMPDIR:-/tmp}"/release-stem.*) rm -rf -- "$dir" ;; esac
}
trap cleanup EXIT

free_port() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()'; }
port=$(free_port); http=$(free_port)

mkdir -p "$dir/units"
cat > "$dir/units/stem-unit.toml" <<UNIT
principal = "stem-unit"

[[rule]]
actor     = "*"
operation = "tools/call"
resource  = "tool:echo.say"
UNIT
cat > "$dir/node.toml" <<CFG
bind_address = "127.0.0.1"
bind_port = $port
http_port = $http

[[gateway_named_tokens]]
name = "ops"
token = "ops-secret"
scopes = ["*"]

[persistence]
base_path = "$dir/data"

[authority]
declarations = "$dir/units"
unit = "stem-unit"
journal = "$dir/evidence.journal"
CFG

"$stem" --units "$dir/units/stem-unit.toml" --config "$dir/node.toml" >"$dir/stem.log" 2>&1 &
pid=$!

url="http://127.0.0.1:$http/gateway/guarantees"
status=""
for _ in $(seq 1 600); do
  if ! kill -0 "$pid" 2>/dev/null; then
    echo "check-release-stem: the stem exited before its gateway answered:" >&2
    grep -a "Error" "$dir/stem.log" >&2 || tail -20 "$dir/stem.log" >&2
    exit 1
  fi
  status=$(curl -s -o /dev/null -w '%{http_code}' "$url" || true)
  [ "$status" != "000" ] && break
  sleep 0.1
done
[ "$status" = "401" ] || { echo "check-release-stem: an anonymous request answered $status, not 401" >&2; exit 1; }

curl -sf -H "Authorization: Bearer ops-secret" "$url" > "$dir/report.json"
python3 - "$dir/report.json" <<'PY'
import json, sys
report = json.load(open(sys.argv[1]))
states = {e["id"]: e["resolution"]["state"] for e in report["entries"]}
want = ["gw.not_open", "gw.token_tables", "ae.authorised_at_seam", "ae.recorded_before_dispatch",
        "authz.execution_authority", "prov.enforcement"]
bad = {i: states.get(i) for i in want if states.get(i) != "enforced"}
if bad:
    sys.exit(f"check-release-stem: not enforced on the stem: {bad}")
print(f"check-release-stem: ok — 401 without a bearer; {', '.join(want)} enforced")
PY
[ -f "$dir/evidence.journal" ] || { echo "check-release-stem: [authority]'s evidence journal was not opened" >&2; exit 1; }
