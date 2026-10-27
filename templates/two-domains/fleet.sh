#!/usr/bin/env bash
# fleet.sh — the two domains' operator steps, each one line of the README. `MYCELIUM` is the `mycelium` binary
# (default: the one on PATH); the node program is this project's, built once by `cargo build`.
#
#   bash fleet.sh setup               the CAs, certificates, domain keys, pins and tokens — every secret, made here
#   bash fleet.sh start-all | stop [node]  the four nodes in the background (logs and pids in data/), or one
#   bash fleet.sh start <node>        one node in the background
#   bash fleet.sh doctor <node>       what start() would say about that node, with its attachments
#   bash fleet.sh connect <gateway>   that gateway fetches its partner's signed catalogue
#   bash fleet.sh call <gateway> <export> [text]  a call at that gateway, by its named local caller, across the edge
#   bash fleet.sh guarantees <node>   the node's own report, from its running gateway
set -euo pipefail
cd "$(dirname "$0")"
M="${MYCELIUM:-mycelium}"
# The nodes are the files under nodes/, and the domains their `cluster_name`s: delete one domain's files and this
# script runs the other alone.
NODES=$(for f in nodes/*.toml; do basename "$f" .toml; done | sort -r | tr '\n' ' ')
DOMAINS=$(sed -n 's/^cluster_name = "\([^"]*\)".*/\1/p' nodes/*.toml | sort -u | tr '\n' ' ')
BIN="${CARGO_TARGET_DIR:-target}/debug/two-domains"

domain_of() { sed -n 's/^cluster_name = "\([^"]*\)".*/\1/p' "nodes/$1.toml"; }
id_of() { echo "127.0.0.1:$(sed -n 's/^bind_port = //p' "nodes/$1.toml")"; }
cert_of() { echo "certs/$1/$(id_of "$1" | tr ':.' '__').cert.pem"; }
gateways() { grep -l '^http_port' nodes/*.toml | xargs -n1 basename | sed 's/\.toml$//'; }
partner_of() { for d in $DOMAINS; do [ "$d" = "$(domain_of "$1")" ] || echo "$d"; done; }
# A gateway's named-token table comes from the environment the node binary and `doctor` both read, never from a
# file in the project: `name|token|scope,scope`.
tokens_env() { if [ -f "secrets/$1.tokens" ]; then echo "GOSSIP_GATEWAY_NAMED_TOKENS=$(cat "secrets/$1.tokens")"; else echo "GOSSIP_UNUSED=1"; fi; }
case "${1:-}" in
  setup)
    cargo build -q                         # first: a build that fails leaves nothing made, so setup can be run again
    [ ! -e secrets ] || { echo "secrets/ exists: setup runs once (remove ca/ certs/ secrets/ exchange/ data/ to start over)"; exit 3; }
    mkdir -p exchange secrets
    for d in $DOMAINS; do
      "$M" tls ca --out "ca/$d"            # each domain's CA, on the issuer's host: ca/ is never a node's directory
      "$BIN" keys "$d"                     # the domain's signing and operator keys; their public halves go to exchange/
    done
    for n in $NODES; do
      "$M" tls issue --ca-dir "ca/$(domain_of "$n")" --node "$(id_of "$n")" --out "certs/$n"
      cp "ca/$(domain_of "$n")/ca-cert.pem" "certs/$n/"   # the CA's certificate, never its key
    done
    for g in $(gateways); do
      "$M" tls pin "$(cert_of "$g")" > "exchange/$g.pin"   # the key the partner's client pins
      "$BIN" tokens "$g"                                   # the gateway's named local callers, from federation.toml
    done
    echo "setup: exchange/ holds what the two operators hand each other: $(ls exchange | tr '\n' ' ')"
    ;;
  start)
    n=$2; mkdir -p data
    env "$(tokens_env "$n")" "$BIN" start "$n" > "data/$n.log" 2>&1 &
    echo $! > "data/$n.pid"
    for _ in $(seq 1 100); do
      grep -q ': up as ' "data/$n.log" && { grep ': up as ' "data/$n.log"; exit 0; }
      kill -0 "$(cat "data/$n.pid")" 2>/dev/null || { cat "data/$n.log"; exit 3; }
      sleep 0.2
    done
    echo "$n did not come up in 20 s"; cat "data/$n.log"; exit 1
    ;;
  start-all)
    cargo build -q
    for n in $NODES; do bash "$0" start "$n"; done
    for n in $NODES; do   # each node ready: its domain's other nodes connected, their markers seen
      for _ in $(seq 1 150); do grep -q ': ready' "data/$n.log" && break; sleep 0.2; done
      grep ': ready' "data/$n.log" || { echo "$n is not ready after 30 s"; cat "data/$n.log"; exit 1; }
    done
    ;;
  stop)
    for n in ${2:-$NODES}; do
      [ -f "data/$n.pid" ] || continue
      pid=$(cat "data/$n.pid")
      kill "$pid" 2>/dev/null || true
      for _ in $(seq 1 100); do kill -0 "$pid" 2>/dev/null || break; sleep 0.1; done   # gone, its files released
      rm -f "data/$n.pid"
    done
    ;;
  doctor)
    env "$(tokens_env "$2")" "$BIN" doctor "$2"
    ;;
  connect)
    "$BIN" post "$2" /gateway/federation/connect "{\"domain\":\"$(partner_of "$2")\"}"
    ;;
  call)
    "$BIN" post "$2" /gateway/federation/call "{\"domain\":\"$(partner_of "$2")\",\"export\":\"$3\",\"text\":\"${4:-stock?}\"}"
    ;;
  guarantees)
    "$BIN" get "$2" /gateway/guarantees
    ;;
  *)
    sed -n '2,12p' "$0"; exit 2
    ;;
esac
