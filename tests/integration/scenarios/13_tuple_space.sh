#!/usr/bin/env bash
# Scenario 13: TupleSpace pipeline — pull-based work distribution.
#
# node-a runs the ts13 tuple space as primary, node-b as secondary mirror.
# Tests the cross-node properties unit tests cannot cover:
#
#   (I)   Primary capability is discoverable cluster-wide (/api/tuple role)
#   (II)  put on node-a / take on node-b — client ops route via RPC to the
#         primary, ids preserved, payloads round-trip through base64
#   (III) In-flight accounting — taken-not-acked items show as inflight in
#         depth, and terminal acks clear them
#   (IV)  Monitoring aggregation — /api/tuple reports both roles and the
#         put/take counters
#   (V)   Empty take times out with 408 (the blocking-pull contract)
set -euo pipefail
source /tests/lib/helpers.sh

H_A="http://${NODE_A_HOST:-node-a}:${NODE_HTTP_PORT:-8300}"
H_B="http://${NODE_B_HOST:-node-b}:${NODE_HTTP_PORT:-8300}"
NS="ts13"

# ── Client deadlines: the observer must outlive the observed ────────────────
# Every tuple operation carries its own server-side budget, and a client deadline shorter than
# that budget cannot observe the gateway's answer — it can only report its own impatience, as
# curl exit 28 or HTTP 000, with no status code and no body to diagnose from.
#
#   discovery   resolve_primary_blocking waits up to 3 × cap_refresh; the demo sets
#               cap_refresh = 2 s (examples/three_node_demo.rs), so 6 s.
#   put/ack/    one rpc_call with a fixed 10 s deadline (mycelium-tuple-space/src/lib.rs).
#   depth
#   take        one rpc_call at timeout_secs + 5 s — the handler parks up to timeout_secs and
#               the pad lets the park, not the transport, decide.
#
# Worst case is therefore 6 + 10 = 16 s for a put, an ack or a depth, and 6 + (TAKE_PARK + 5)
# for a take. This scenario previously allowed 5 s against put's 16 s and exactly 10 s against
# take's 16 s, so any request frame lost to an outbound-writer reconnect backoff — routine right
# after scenarios 03/04/05 restart node-a, the whole cluster and node-c — failed the run with an
# undiagnosable transport error instead of the gateway's reply. Two of the three main runs on
# 2026-09-15 died that way, once on a put and once on take #9.
#
# This is **not** the forbidden "widen the timeout until it goes green" (testing.md §The CI flake
# tier). These numbers are not a tolerance for slowness: they are the window in which the
# server's verdict is allowed to arrive at all. Set below the server's own budget, the assertion
# is unobservable by construction — the test can only ever report that it gave up first.
#
# The rule binds single-shot assertions, which fail the run outright. The short deadlines left
# inside `poll_until` probes below are deliberate and correct: there the retry *is* the recovery,
# so a lost frame costs one iteration rather than the scenario.
CLIENT_MAX=20      # > 16 s: discovery + a 10 s tuple RPC
TAKE_PARK=5        # the take's server-side park
TAKE_MAX=20        # > 6 + TAKE_PARK + 5

b64() { printf '%s' "$1" | base64 | tr -d '\n'; }

# ── Phase I: primary discoverable ────────────────────────────────────────────
# Note on what this proves, and what it does not. `/api/tuple` is aggregated from the gossiped
# `sys/tuple/{node}/{ns}/role` records the metrics writer publishes each tick. Those records
# carry no freshness stamp and are never cleared on shutdown — unlike the backpressure pheromone
# the *same* loop stamps with `written_at_ms` and evaporates after 3× its cadence — and they
# survive a restart in the replayed WAL. So a passing poll here means "a node published the
# primary role at some point", not "that node is serving now". It was not the cause of either
# 2026-09-15 failure (this phase passed in both), and it is left as the cheap first gate it is;
# the deadlines above are what make the phases after it observable. The missing freshness on the
# role record is recorded as a separate finding against the monitoring endpoint.
primary_visible() {
    local role
    role=$(curl -s --max-time 3 "${H_A}/api/tuple" 2>/dev/null \
        | jq -r '.nodes[] | select(.ns=="'"$NS"'") | select(.role=="primary") | .role' \
        2>/dev/null | head -1)
    [ "$role" = "primary" ]
}
poll_until 30 primary_visible || {
    printf 'FAIL: ts13 primary not visible in /api/tuple within 30s\n' >&2
    false
}

# ── Phase II: put 10 on node-a, take 10 on node-b ────────────────────────────
put_ids=""
for i in $(seq 0 9); do
    # Capture the HTTP code and body rather than piping `curl -sf` into jq: that form reports a
    # lost put as `jq exited 28` — no iteration, no status, no body — which is how the
    # 2026-09-15 failure arrived. The take loop was given this treatment in #150; the put loop
    # was not, and inherited the same blindness.
    : > /tmp/put_body.json
    code=$(curl -s --max-time "$CLIENT_MAX" -o /tmp/put_body.json -w '%{http_code}' \
        -X POST -H "Content-Type: application/json" \
        -d "{\"ns\":\"$NS\",\"stage\":\"s13-work\",\"payload_b64\":\"$(b64 "item-$i")\"}" \
        "${H_A}/gateway/tuple/put") || code=000
    if [ "$code" != "200" ]; then
        echo "FAIL: put #$i on node-a: HTTP $code body='$(head -c 200 /tmp/put_body.json 2>/dev/null)'" >&2
        exit 1
    fi
    put_ids="$put_ids $(jq -r '.id' /tmp/put_body.json)"
done
put_count=$(echo "$put_ids" | tr ' ' '\n' | grep -c '^[0-9]' || true)
assert_eq "$put_count" 10 "10 puts must return 10 ids"

take_ids=""
for i in $(seq 0 9); do
    # Same request as before, but capture the HTTP code + body instead of -f's silent death:
    # a failing take must say WHICH iteration and WHAT the gateway answered (408 = the take RPC
    # timed out server-side vs 5xx = provider/plumbing error) — the first hosted CI failure was
    # undiagnosable without this (#150).
    # Truncate first: curl leaves the file untouched when it never gets a response, so the
    # failure report below would otherwise print the *previous* iteration's body. The
    # 2026-09-15 run said `take #9 ... body='{"id":8,...}'` for exactly that reason, which
    # reads as a wrong-id bug rather than the timeout it was.
    : > /tmp/take_body.json
    code=$(curl -s --max-time "$TAKE_MAX" -o /tmp/take_body.json -w '%{http_code}' \
        -X POST -H "Content-Type: application/json" \
        -d "{\"ns\":\"$NS\",\"stage\":\"s13-work\",\"timeout_secs\":$TAKE_PARK}" \
        "${H_B}/gateway/tuple/take") || code=000
    if [ "$code" != "200" ]; then
        echo "FAIL: take #$i on node-b: HTTP $code body='$(head -c 200 /tmp/take_body.json 2>/dev/null)'" >&2
        exit 1
    fi
    take_ids="$take_ids $(jq -r '.id' /tmp/take_body.json)"
done
# Every put id was taken exactly once (set equality, order-independent).
sorted_put=$(echo "$put_ids"  | tr ' ' '\n' | grep '^[0-9]' | sort -n | tr '\n' ',')
sorted_take=$(echo "$take_ids" | tr ' ' '\n' | grep '^[0-9]' | sort -n | tr '\n' ',')
assert_eq "$sorted_take" "$sorted_put" "taken ids must equal put ids"

# ── Phase III: in-flight accounting ──────────────────────────────────────────
inflight=$(curl -sf --max-time "$CLIENT_MAX" \
    "${H_B}/gateway/tuple/depth?ns=$NS&stage=s13-work" \
    | jq -r '.stages[0].inflight')
assert_eq "$inflight" 10 "all taken items must be in-flight before ack"

for id in $take_ids; do
    curl -sf --max-time "$CLIENT_MAX" -X POST -H "Content-Type: application/json" \
        -d "{\"ns\":\"$NS\",\"id\":$id}" "${H_B}/gateway/tuple/ack" > /dev/null
done
inflight_after() {
    local n
    n=$(curl -s --max-time 3 "${H_B}/gateway/tuple/depth?ns=$NS&stage=s13-work" \
        | jq -r '.stages[0].inflight' 2>/dev/null || echo -1)
    [ "$n" = "0" ]
}
poll_until 15 inflight_after || {
    printf 'FAIL: inflight count did not return to 0 after acks\n' >&2
    false
}

# ── Phase IV: /api/tuple aggregation shows both roles and the counters ──────
counters_visible() {
    local doc puts takes secondary
    doc=$(curl -s --max-time 3 "${H_A}/api/tuple" 2>/dev/null) || return 1
    puts=$(echo "$doc" | jq -r '[.nodes[] | select(.ns=="'"$NS"'") | select(.role=="primary")
        | .stages[] | select(.stage=="s13-work") | .put_total][0] // 0')
    takes=$(echo "$doc" | jq -r '[.nodes[] | select(.ns=="'"$NS"'") | select(.role=="primary")
        | .stages[] | select(.stage=="s13-work") | .take_total][0] // 0')
    secondary=$(echo "$doc" | jq -r '[.nodes[] | select(.ns=="'"$NS"'")
        | select(.role=="secondary")] | length')
    [ "$puts" -ge 10 ] && [ "$takes" -ge 10 ] && [ "$secondary" -ge 1 ]
}
poll_until 30 counters_visible || {
    printf 'FAIL: /api/tuple never showed primary counters and a secondary\n' >&2
    false
}

# ── Phase V: empty take → 408 (blocking-pull contract) ───────────────────────
# 1 s park + 5 s pad + up to 6 s discovery = 12 s server-side; the old 10 s here could not see
# the 408 it asserts on.
status=$(curl -s -o /dev/null -w "%{http_code}" --max-time "$CLIENT_MAX" \
    -X POST -H "Content-Type: application/json" \
    -d "{\"ns\":\"$NS\",\"stage\":\"s13-empty\",\"timeout_secs\":1}" \
    "${H_B}/gateway/tuple/take")
assert_eq "$status" "408" "take on empty stage must time out with 408"
