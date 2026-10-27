#!/usr/bin/env bash
# Adopter-surface Y3's exit, run: the Ops Console — a static page — rendered by a real browser against a
# three-node fleet (`examples/observed_fleet.rs`), each panel from the routes `examples/ops_console/README.md`
# names, every node read directly by the browser with no server of the console's own.
#
#   scripts/ops-console-render.sh            run every case, against target/debug/examples/observed_fleet
#                                            (cargo build --example observed_fleet --features metrics,compliance)
#   scripts/ops-console-render.sh --list     name the cases, run nothing
#
# Cases (`@@case@@ scripts/ops-console-render.sh::<case>` as each starts):
#   tab-<Tab>       one per row of the README's table: the page, served from a listed origin with the
#                   fleet's bearer, renders that tab with every node `ok` from exactly the routes the row names
#                   (scripts/check-ops-console-render.py reads the README and the dumped DOM);
#   unlisted-origin the same page served from an origin no node lists: the browser withholds every answer,
#                   every node `blocked` — CORS enforced end to end, not only in the gateway's tests;
#   no-bearer       the listed origin without the bearer, on a gated tab (Fleet): every node `unauthorized` —
#                   the setting never stands in for the credential.
# The fleet binary is built with `metrics` and `compliance`, so the Metrics and Audit tabs read a mounted route.
# The browser is $CHROME, else the first of google-chrome, chromium, chromium-browser, or macOS's Chrome.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/.." && pwd)"
readme="$root/examples/ops_console/README.md"
suite="scripts/ops-console-render.sh"
tabs() { python3 "$here/check-ops-console-render.py" --tabs "$readme"; }

if [[ "${1:-}" == "--list" ]]; then
  for t in $(tabs); do echo "@@case-list@@ $suite::tab-$t"; done
  echo "@@case-list@@ $suite::unlisted-origin"
  echo "@@case-list@@ $suite::no-bearer"
  exit 0
fi

cd "$root"
[[ -x target/debug/examples/observed_fleet ]] || {
  echo "ops-console-render: build the fleet first: cargo build --example observed_fleet --features metrics,compliance" >&2
  exit 1
}
chrome="${CHROME:-}"
if [[ -z "$chrome" ]]; then
  for c in google-chrome chromium chromium-browser "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"; do
    if command -v "$c" >/dev/null 2>&1 || [[ -x "$c" ]]; then chrome="$c"; break; fi
  done
fi
[[ -n "$chrome" ]] || { echo "ops-console-render: no Chrome or Chromium found (set CHROME)" >&2; exit 1; }

work="$(mktemp -d "${TMPDIR:-/tmp}/ops-console-render.XXXXXX")"
pids=()
cleanup() {
  for p in "${pids[@]:-}"; do [[ -n "$p" ]] && kill "$p" 2>/dev/null || true; done
  [[ -n "$work" && -d "$work" && "$work" == */ops-console-render.* ]] && rm -rf -- "$work"
}
trap cleanup EXIT

listed="http://127.0.0.1:8099"
token="ops-console-ci"
OBSERVED_FLEET_CORS="$listed" OBSERVED_FLEET_TOKEN="$token" target/debug/examples/observed_fleet >"$work/fleet.log" 2>&1 &
pids+=($!)
python3 -m http.server 8099 --bind 127.0.0.1 --directory "$root/examples/ops_console" >"$work/listed.log" 2>&1 &
pids+=($!)
python3 -m http.server 8097 --bind 127.0.0.1 --directory "$root/examples/ops_console" >"$work/unlisted.log" 2>&1 &
pids+=($!)
for _ in $(seq 1 120); do grep -q "observed-fleet: ready" "$work/fleet.log" && break; sleep 0.5; done
grep -q "observed-fleet: ready" "$work/fleet.log" || { cat "$work/fleet.log"; echo "ops-console-render: the fleet did not start" >&2; exit 1; }
for port in 8099 8097; do
  for _ in $(seq 1 40); do curl -sf -o /dev/null "http://127.0.0.1:$port/index.html" && break; sleep 0.25; done
done
# The tuple space's advertisement and the fleet's membership reach every node by gossip.
nodes="127.0.0.1:9061,127.0.0.1:9062,127.0.0.1:9063"
for _ in $(seq 1 60); do
  n=0
  for p in 9061 9062 9063; do
    curl -sf -H "Authorization: Bearer $token" "http://127.0.0.1:$p/gateway/tuple/overview" | grep -q '"stage":"intake"' && n=$((n+1))
  done
  [[ $n -eq 3 ]] && break; sleep 0.5
done

render() {  # <origin> <query> <fragment> <out>
  "$chrome" --headless=new --disable-gpu --no-first-run --no-default-browser-check ${CI:+--no-sandbox} \
    --user-data-dir="$work/profile-$RANDOM" --virtual-time-budget=15000 \
    --dump-dom "$1/?$2#$3" >"$4" 2>"$4.err" || { cat "$4.err" >&2; return 1; }
}

fail=0
for t in $(tabs); do
  echo "@@case@@ $suite::tab-$t"
  render "$listed" "once&nodes=$nodes&tab=$t" "token=$token" "$work/$t.html"
  python3 "$here/check-ops-console-render.py" --check "$readme" "$t" ok "$nodes" "$work/$t.html" || fail=1
done
echo "@@case@@ $suite::unlisted-origin"
render "http://127.0.0.1:8097" "once&nodes=$nodes&tab=Overview" "token=$token" "$work/unlisted.html"
python3 "$here/check-ops-console-render.py" --check "$readme" Overview blocked "$nodes" "$work/unlisted.html" || fail=1
echo "@@case@@ $suite::no-bearer"
render "$listed" "once&nodes=$nodes&tab=Fleet" "" "$work/no-bearer.html"
python3 "$here/check-ops-console-render.py" --check "$readme" Fleet unauthorized "$nodes" "$work/no-bearer.html" || fail=1
exit $fail
