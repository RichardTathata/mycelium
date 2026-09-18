#!/usr/bin/env bash
# The two-mesh federation suite (v3 item 2 PR 10b) — runs inside the runner container.
#
# Item 2's release gate (docs/design/federated-domains.md §13) with **process isolation** (one
# container per node) and a **real network severance** (`docker network disconnect`): discover,
# invoke, lose a gateway, sever every link, keep working locally, change permissions
# mid-partition, reconnect — and prove from membership tables, consensus state and connection
# tables that the meshes never merged. Every assertion reads a table from a node's own admin
# surface; none of them reads a log line.
#
# The runner drives the probe over `control`, a network the federation path never uses, so cutting
# `edge` severs the link under test without cutting the harness.
#
# Style note, learned the hard way: every check runs a **shell function in this shell**. An earlier
# draft wrapped compound checks in `sh -c "post …"`, where the function does not exist — 26 checks
# failed for that reason alone while the code under test was fine.
set -uo pipefail

PASS=0; FAIL=0
banner() { printf '\n\033[1;34m== %s ==\033[0m\n' "$1"; }
ok()     { printf '\033[0;32mPASS\033[0m  %s\n' "$1"; PASS=$((PASS+1)); }
fail()   { printf '\033[0;31mFAIL\033[0m  %s\n' "$1"; FAIL=$((FAIL+1)); }
check()  {
    local label="$1"; shift
    if "$@" >/dev/null 2>&1; then
        ok "$label"
    else
        fail "$label"
        # A failing check that prints nothing costs a whole run to diagnose. If the last probe
        # response is on disk, show it: status is in the label's assertion, the body is here.
        [ -s /tmp/probe.out ] && sed 's/^/      last probe body: /' /tmp/probe.out | head -3 >&2
    fi
}

poll_until() {  # poll_until SECS CMD...
    local t="$1"; shift; local i=0
    while [ "$i" -lt "$t" ]; do
        if "$@" >/dev/null 2>&1; then return 0; fi
        sleep 1; i=$((i+1))
    done
    return 1
}

A1=172.30.0.11; A2=172.30.0.12; GW1=172.30.0.13; GW2=172.30.0.14; ROGUE=172.30.0.19
B1=172.31.0.11; B2=172.31.0.12
PROBE=http://172.33.0.21:8400
ALPHA_IDS="$A1:57000 $A2:57000 $GW1:57000 $GW2:57000"
BETA_IDS="$B1:57000 $B2:57000"

admin() { curl -sf --max-time 10 "http://$1:8300/fed-admin/$2"; }
post()  { curl -sf --max-time 30 -X POST -H 'content-type: application/json' "$1" -d "$2"; }

# probe VERB [JSON] — POST to the probe's control API; body to /tmp/probe.out, status on stdout.
# The body default is spelled out rather than written `${2:-{}}`: in that form bash closes the
# expansion at the first `}` and appends a stray one, which silently corrupted every request that
# carried a body (and only those — which is why `connect` passed while `call` did not).
probe() {
    local verb="$1" data="${2-}"
    if [ -z "$data" ]; then data='{}'; fi
    curl -s --max-time 60 -o /tmp/probe.out -w '%{http_code}' \
         -X POST -H 'content-type: application/json' "$PROBE/$verb" -d "$data"
}
body() { jq -e "$1" /tmp/probe.out >/dev/null 2>&1; }

# -- predicates, all callable by `check` and `poll_until` ------------------------------------
test_peers()   { [ "$(admin "$1" tables | jq -r '.peers | length')" = "$2" ]; }
has_cap()      { admin "$1" tables | jq -e --arg k "cap/$A1:57000/demo/whoami" '.entries[] | select(.key | startswith($k))'; }
commits()      { post "http://$1:8300/fed-admin/propose" "{\"slot\":\"$2\",\"value\":\"$3\"}" | jq -e '.committed == true'; }
slot_absent()  { admin "$1" "committed/$2" | jq -e '.value == null'; }
kv_put()       { post "http://$1:8300/fed-admin/kv" "{\"key\":\"$2\",\"value\":\"$3\"}"; }
kv_is()        { admin "$1" "kv/$2" | jq -e --arg v "$3" '.value == $v'; }
link_is()      { curl -sf --max-time 10 "$PROBE/link" | jq -e --arg l "$1" '.link == $l'; }
retire_gw()    { post "$PROBE/retire" "{\"id\":\"$1\"}" | jq -e '.retired == true'; }
grant_both()   { post "http://$1:8300/fed-admin/policy" \
                   '{"revision":2,"grants":[["beta.example","demo/whoami"],["beta.example","demo/secret"]]}' \
                   | jq -e '.policy_revision == 2'; }
rogue_alone()  { admin "$ROGUE" tables | jq -e '.peers | length == 0'; }
rogue_ready()  { curl -sf --max-time 5 "http://$ROGUE:8300/ready"; }
connect_ok()   { [ "$(probe connect)" = 200 ]; }

# call_is STATUS JQ [JSON] — one federated call, asserting status and a fact about the body.
call_is() {
    local want="$1" filter="$2" data="${3-}"
    [ "$(probe call "$data")" = "$want" ] && body "$filter"
}

# never_merged "IPS" "FOREIGN_IDS" LABEL — the three legs, read from each node's own tables.
never_merged() {
    local ips="$1" foreign="$2" label="$3" rc=0 t
    for ip in $ips; do
        t=$(admin "$ip" tables) || { echo "  $label $ip: tables unreachable" >&2; return 1; }
        for f in $foreign; do
            if echo "$t" | jq -e --arg f "$f" '.peers | index($f)' >/dev/null 2>&1; then
                echo "  $label $ip: membership names foreign $f" >&2; rc=1
            fi
            if echo "$t" | jq -e --arg f "$f" '.connected_peers | index($f)' >/dev/null 2>&1; then
                echo "  $label $ip: connection table names foreign $f" >&2; rc=1
            fi
            if echo "$t" | jq -e --arg f "$f" \
                 '.entries[] | select((.key | contains($f)) or (.value | contains($f)))' >/dev/null 2>&1; then
                echo "  $label $ip: a cap/grp/sys/consensus entry names foreign $f" >&2; rc=1
            fi
        done
    done
    return $rc
}

# -- 0. Both meshes formed, each under its own CA ---------------------------------------------
banner "0 . formation"
check "alpha a1 sees its three peers"  poll_until 90 test_peers "$A1" 3
check "alpha gw2 sees its three peers" poll_until 90 test_peers "$GW2" 3
check "beta b1 sees its one peer"      poll_until 90 test_peers "$B1" 1
check "alpha gw1 learned the provider's capability" poll_until 90 has_cap "$GW1"
check "alpha gw2 learned the provider's capability" poll_until 90 has_cap "$GW2"

# -- 1. Discover, invoke -----------------------------------------------------------------------
banner "1 . discover, invoke"
check "before discovery a call is refused locally as a link refusal" \
      call_is 409 '.error == "link"' '{"export":"demo/whoami"}'
# `connect` writes /tmp/probe.out, so status and body are asserted in two steps.
if [ "$(probe connect)" = 200 ] && body '.exports == ["demo/whoami"]'; then
    ok "the signed catalogue is the grant, not the export list"
else
    fail "the signed catalogue is the grant, not the export list"
fi
check "the call crosses and the provider is told beta's principal" \
      call_is 200 '.reply == "federation:beta.example/svc/billing"' '{"export":"demo/whoami","repeatable":true}'
check "an ungranted export is refused locally, before any byte" \
      call_is 409 '.error == "resolve"' '{"export":"demo/secret"}'

# -- 2. Consensus in each mesh -----------------------------------------------------------------
banner "2 . consensus, per mesh"
check "alpha commits fed/alpha"        commits "$A1" "fed/alpha" alpha-1
check "beta commits fed/beta"          commits "$B1" "fed/beta"  beta-1
check "beta never learns alpha's slot" slot_absent "$B2"  "fed/alpha"
check "alpha never learns beta's slot" slot_absent "$GW2" "fed/beta"

# -- 3. Never merged, at steady state with bytes crossing --------------------------------------
banner "3 . never merged (steady state)"
check "alpha's three tables name no beta node" never_merged "$A1 $A2 $GW1 $GW2" "$BETA_IDS" alpha
check "beta's three tables name no alpha node" never_merged "$B1 $B2" "$ALPHA_IDS" beta

# -- 4. Lose a gateway, then sever every link --------------------------------------------------
banner "4 . lose gw1, then sever every link"
docker stop mycelium-fed-alpha-gw1 >/dev/null
check "repeatable fails over to gw-2 with gw-1 stopped" \
      call_is 200 '.reply != null' '{"export":"demo/whoami","repeatable":true}'
docker network disconnect mycelium-fed-edge mycelium-fed-beta-probe
check "at-most-once with every link severed is DeliveryUnknown via gw-1 only" \
      call_is 409 '.error == "outcome" and (.debug | contains("gw-1")) and (.debug | contains("gw-2") | not)' \
      '{"export":"demo/whoami"}'
check "repeatable having tried every gateway is still unknown, not failed" \
      call_is 409 '.error == "outcome" and (.debug | contains("gw-1")) and (.debug | contains("gw-2"))' \
      '{"export":"demo/whoami","repeatable":true}'
if [ "$(probe connect)" = 409 ]; then ok "discovery cannot refresh with every link severed"
else fail "discovery cannot refresh with every link severed"; fi
check "the link reads Down" link_is Down

# -- 5. Keep working locally, both sides -------------------------------------------------------
banner "5 . both meshes keep working with no link"
kv_put "$A1" "local/alpha" "still here" >/dev/null
kv_put "$B1" "local/beta"  "still here" >/dev/null
check "alpha gossip converges a1 to a2" poll_until 30 kv_is "$A2" "local/alpha" "still here"
check "beta gossip converges b1 to b2"  poll_until 30 kv_is "$B2" "local/beta"  "still here"
check "alpha commits while partitioned" commits "$A2" "fed/alpha-partitioned" alpha-2
check "beta commits while partitioned"  commits "$B2" "fed/beta-partitioned"  beta-2

# -- 6. Change the grant mid-partition ---------------------------------------------------------
banner "6 . change the grant while no link exists"
check "gw2 takes policy revision 2, granting demo/secret as well" grant_both "$GW2"

# -- 7. Reconnect ------------------------------------------------------------------------------
banner "7 . reconnect"
docker network connect --ip 172.32.0.21 mycelium-fed-edge mycelium-fed-beta-probe
check "reconnected is not ready: refused until discovery refreshes" \
      call_is 409 '.error == "link"' '{"export":"demo/whoami","repeatable":true}'
if poll_until 30 connect_ok; then ok "discovery refreshes through gw-2 (gw-1 still dead)"
else fail "discovery refreshes through gw-2 (gw-1 still dead)"; fi
if body '.exports == ["demo/whoami","demo/secret"]'; then
    ok "the refreshed catalogue IS the grant changed mid-partition"
else
    fail "the refreshed catalogue IS the grant changed mid-partition"
fi
check "a repeatable call on the new grant fails over past dead gw-1" \
      call_is 200 '.reply == "federation:beta.example/svc/billing"' '{"export":"demo/secret","repeatable":true}'
check "at-most-once still pays the dead gateway once (the pool keeps no health memory)" \
      call_is 409 '.error == "outcome"' '{"export":"demo/secret"}'
check "retiring gw-1 is accepted" retire_gw gw-1
check "at-most-once now goes straight to gw-2" \
      call_is 200 '.reply == "federation:beta.example/svc/billing"' '{"export":"demo/secret"}'

# -- 8. Never merged, after the whole cycle ----------------------------------------------------
banner "8 . never merged (after the cycle)"
check "alpha commits again after reconnect"   commits "$A1" "fed/alpha-after" alpha-3
check "alpha's live tables name no beta node" never_merged "$A1 $A2 $GW2" "$BETA_IDS" alpha
check "beta's tables name no alpha node"      never_merged "$B1 $B2" "$ALPHA_IDS" beta
check "beta learned no alpha slot after the cycle" slot_absent "$B1" "fed/alpha-after"
check "beta learned no alpha slot from before it" slot_absent "$B1" "fed/alpha"

# -- 9. The admission plant: beta's CA cannot join alpha ----------------------------------------
# Started here rather than by compose: it must not exist while the legs above run. The image and
# the volume are pinned names in the compose file precisely so this can name them.
banner "9 . admission plant"
docker rm -f mycelium-fed-rogue >/dev/null 2>&1
docker run -d --name mycelium-fed-rogue \
    --network mycelium-fed-alpha --ip "$ROGUE" \
    -v mycelium-fed-beta-ca:/ca \
    -e FED_ROLE=member -e FED_DOMAIN=alpha.example -e FED_CA_DIR=/ca \
    -e MYCELIUM_HOSTNAME="$ROGUE" -e MYCELIUM_PORT=57000 -e MYCELIUM_HTTP_PORT=8300 \
    -e MYCELIUM_PEERS="$A1:57000,$A2:57000" \
    -e RUST_LOG=warn \
    mycelium-federation-node:test >/dev/null
check "the rogue node is up (so the negative below is about admission, not a dead container)" \
      poll_until 60 rogue_ready
sleep 10
check "the rogue, holding beta's CA, sees no alpha peer"      rogue_alone
check "alpha a1 neither peers with nor connects to the rogue" never_merged "$A1" "$ROGUE:57000" alpha
docker rm -f mycelium-fed-rogue >/dev/null 2>&1

banner "summary"
printf 'PASS=%d FAIL=%d\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
