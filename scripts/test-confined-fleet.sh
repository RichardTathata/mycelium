#!/usr/bin/env bash
# Boundary H item H7 — the confined-fleet deployment test. DEPLOYMENT evidence, not replay evidence.
#
# Brings up a kind cluster with Calico (a CNI that enforces NetworkPolicy), applies deploy/confined-fleet/ UNCHANGED
# except for stand-in images, and checks, from inside an agent pod:
#   - the gateway's API port is reachable;
#   - the internet, the Kubernetes API and the gateway's admin port are NOT reachable;
#   - no service-account token is mounted.
# Every "not reachable" check is paired with a positive control pod that CAN reach the same destination, so a pass
# means the policy blocked it, not that the destination was down.
#
# What it does not show, stated rather than implied:
#   - instance metadata (169.254.169.254): kind has none, so the check cannot discriminate. The default-deny egress
#     policy covers it, and it is reported as not-discriminating rather than as passed;
#   - that a request through the gateway is recorded: the stand-in gateway is not a Mycelium node. The AE seam's own
#     tests cover recording.
#
# Phase 2 (closure plan C7 and C12, deployment variants) then replaces the stand-ins with the real node image
# (docker/Dockerfile.confined-fleet, built here unless NODE_IMAGE names one already built): a mandate authority, a
# provider member and the gateway, and the agent pod running the agent's subcommands. From the agent pod, through
# the gateway only:
#   - C7: with a valid mandate every front door (/mcp, /a2a send and stream) reaches the provider's handlers (the
#     plant); after the authority revokes the term, none does, and the raw routes refuse protected kinds (403). The
#     member plane (the provider's gossip port) is not reachable from the agent pod at all; the control reaches it;
#   - C12: calls to a long-running tool are admitted under the mandate, the authority revokes, and T_admit and
#     T_drain from the provider's own records are within the class's declared bound plus the checkpoint delivery
#     allowance. The pods share one kind node and so one clock: the clock bound is declared, not exercised.
# SKIP_NODES=1 runs phase 1 only.
#
# Needs: docker, kubectl, network access (images, Calico manifest). Uses `kind` from PATH, or $KIND, or downloads it.
set -euo pipefail

CLUSTER="${CLUSTER:-mycelium-confined}"
CALICO_VERSION="${CALICO_VERSION:-v3.28.2}"
KIND_VERSION="${KIND_VERSION:-v0.24.0}"
HERE="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$(mktemp -d)"
FAIL=0
# A private kubeconfig: kind would otherwise switch the caller's current kubectl context.
export KUBECONFIG="$WORK/kubeconfig"

KIND="${KIND:-$(command -v kind || true)}"
if [ -z "$KIND" ]; then
  os="$(uname -s | tr '[:upper:]' '[:lower:]')"; arch="$(uname -m)"
  case "$arch" in x86_64) arch=amd64 ;; aarch64|arm64) arch=arm64 ;; esac
  curl -fsSL -o "$WORK/kind" "https://kind.sigs.k8s.io/dl/${KIND_VERSION}/kind-${os}-${arch}"
  chmod +x "$WORK/kind"; KIND="$WORK/kind"
fi

cleanup() { [ "${KEEP_CLUSTER:-0}" = 1 ] || "$KIND" delete cluster --name "$CLUSTER" >/dev/null 2>&1 || true; rm -rf "$WORK"; }
trap cleanup EXIT

echo "== cluster: kind + Calico ${CALICO_VERSION}"
"$KIND" create cluster --name "$CLUSTER" --config "$HERE/tests/confined-fleet/kind.yaml"  # no --wait: nodes cannot be Ready until the CNI is installed below
kubectl apply -f "https://raw.githubusercontent.com/projectcalico/calico/${CALICO_VERSION}/manifests/calico.yaml" >/dev/null
kubectl -n kube-system rollout status daemonset/calico-node --timeout=300s
kubectl wait --for=condition=Ready nodes --all --timeout=300s

echo "== apply deploy/confined-fleet (stand-in images)"
mkdir -p "$WORK/m"
for f in namespace gateway agents network-policy; do
  sed -e 's|GATEWAY_IMAGE|busybox:1.36|' -e 's|AGENT_IMAGE|curlimages/curl:8.10.1|' \
    "$HERE/deploy/confined-fleet/$f.yaml" > "$WORK/m/$f.yaml"
done
kubectl apply -f "$WORK/m/namespace.yaml"
kubectl apply -f "$WORK/m/"
# Stand-ins need a process to run; the manifests stay as shipped, and only the command is patched in.
kubectl -n confined-fleet patch deployment gateway --type=json -p='[{"op":"add","path":"/spec/template/spec/containers/0/command","value":["sh","-c","mkdir -p /w && echo gateway > /w/index.html && httpd -p 8080 -h /w && httpd -f -p 9090 -h /w"]}]'
kubectl -n confined-fleet patch deployment agents --type=json -p='[{"op":"add","path":"/spec/template/spec/containers/0/command","value":["sh","-c","sleep 100000"]}]'
kubectl apply -f "$HERE/tests/confined-fleet/control-pod.yaml"
kubectl -n confined-fleet rollout status deployment/gateway --timeout=180s
kubectl -n confined-fleet rollout status deployment/agents --timeout=180s
kubectl -n confined-fleet wait --for=condition=Ready pod/control --timeout=180s
AGENT="$(kubectl -n confined-fleet get pod -l app=agents -o jsonpath='{.items[0].metadata.name}')"

# HTTP status from inside a pod; 000 means no connection was made.
code() { kubectl -n confined-fleet exec "$1" -- curl -sk -o /dev/null -w '%{http_code}' --connect-timeout 5 --max-time 8 "$2" 2>/dev/null || true; }
connected() { [ -n "$1" ] && [ "$1" != "000" ]; }

check_reachable() { # name, url
  local c; c="$(code "$AGENT" "$2")"
  if connected "$c"; then echo "PASS  agent reaches $1 ($c)"; else echo "FAIL  agent cannot reach $1 ($c)"; FAIL=1; fi
}
check_blocked() { # name, url — blocked for the agent, reachable for the control
  local a ctl; a="$(code "$AGENT" "$2")"; ctl="$(code control "$2")"
  if ! connected "$ctl"; then echo "FAIL  control cannot reach $1 either ($ctl): the check would prove nothing"; FAIL=1
  elif connected "$a"; then echo "FAIL  agent reaches $1 ($a) — confinement broken"; FAIL=1
  else echo "PASS  agent blocked from $1; control reaches it ($ctl)"; fi
}

# ── Phase 2: the real nodes (C7 and C12, deployment variants) ────────────────────────────────────────────────
NODE_IMAGE="${NODE_IMAGE:-}"
phase2() {
  local ns="-n confined-fleet" img="$NODE_IMAGE"
  if [ -z "$img" ]; then
    img="mycelium-confined-node:ci"
    echo "== phase 2: build the node image ($img)"
    DOCKER_BUILDKIT=1 docker build -q -f "$HERE/docker/Dockerfile.confined-fleet" -t "$img" "$HERE" >/dev/null
  fi
  "$KIND" load docker-image "$img" --name "$CLUSTER" >/dev/null

  echo "== phase 2: keys, the fleet CA, the authority and a provider"
  local auth_seed holder_seed auth_pub holder_pub
  auth_seed="$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')"
  holder_seed="$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')"
  auth_pub="$(docker run --rm "$img" pubkey "$auth_seed")"
  holder_pub="$(docker run --rm "$img" pubkey "$holder_seed")"
  mkdir -p "$WORK/ca"
  docker run --rm --entrypoint sh "$img" -c 'confined_fleet_node ca-init /tmp/ca >/dev/null 2>&1 && tar -C /tmp/ca -cf - ca-cert.pem ca-key.pem' \
    | tar -C "$WORK/ca" -xf -
  kubectl $ns create secret generic member-key --from-file="$WORK/ca/ca-cert.pem" --from-file="$WORK/ca/ca-key.pem" >/dev/null
  sed -e "s|NODE_IMAGE|$img|" -e "s|AUTHORITY_SEED_HEX|$auth_seed|" -e "s|AUTHORITY_PUB_HEX|$auth_pub|" \
      -e "s|HOLDER_PUB_HEX|$holder_pub|" "$HERE/tests/confined-fleet/nodes.yaml" | kubectl apply -f - >/dev/null
  kubectl $ns wait --for=condition=Ready pod/authority pod/provider --timeout=180s >/dev/null
  local provider_ip; provider_ip="$(kubectl $ns get pod provider -o jsonpath='{.status.podIP}')"
  local provider="$provider_ip:57000"

  echo "== phase 2: the gateway and the agent on the node image"
  kubectl $ns patch deployment gateway --type=json -p="[
    {\"op\":\"replace\",\"path\":\"/spec/template/spec/containers/0/image\",\"value\":\"$img\"},
    {\"op\":\"add\",\"path\":\"/spec/template/spec/containers/0/imagePullPolicy\",\"value\":\"Never\"},
    {\"op\":\"replace\",\"path\":\"/spec/template/spec/containers/0/command\",\"value\":[\"confined_fleet_node\"]},
    {\"op\":\"add\",\"path\":\"/spec/template/spec/containers/0/env\",\"value\":[
      {\"name\":\"ROLE\",\"value\":\"gateway\"},
      {\"name\":\"POD_IP\",\"valueFrom\":{\"fieldRef\":{\"fieldPath\":\"status.podIP\"}}},
      {\"name\":\"PROVIDER_NODE\",\"value\":\"$provider\"},
      {\"name\":\"OPERATOR_URL\",\"value\":\"http://authority.confined-fleet.svc:8400\"},
      {\"name\":\"AUTHORITY_PUB\",\"value\":\"$auth_pub\"},
      {\"name\":\"HOLDER_PUB\",\"value\":\"$holder_pub\"}]}]" >/dev/null
  kubectl $ns patch deployment agents --type=json -p="[
    {\"op\":\"replace\",\"path\":\"/spec/template/spec/containers/0/image\",\"value\":\"$img\"},
    {\"op\":\"add\",\"path\":\"/spec/template/spec/containers/0/imagePullPolicy\",\"value\":\"Never\"},
    {\"op\":\"replace\",\"path\":\"/spec/template/spec/containers/0/command\",\"value\":[\"sh\",\"-c\",\"sleep 100000\"]},
    {\"op\":\"add\",\"path\":\"/spec/template/spec/containers/0/env/-\",\"value\":{\"name\":\"HOLDER_SEED\",\"value\":\"$holder_seed\"}}]" >/dev/null
  kubectl $ns rollout status deployment/gateway --timeout=180s >/dev/null
  kubectl $ns rollout status deployment/agents --timeout=180s >/dev/null
  local agent; agent="$(kubectl $ns get pod -l app=agents --field-selector=status.phase=Running -o jsonpath='{.items[0].metadata.name}')"

  report() { kubectl $ns exec provider -- curl -s "http://127.0.0.1:9100/report?revoked_at=${1:-0}"; }
  field() { python3 -c "import json,sys; v=json.loads(sys.argv[1]).get(sys.argv[2]); print('null' if v is None else v)" "$1" "$2"; }
  local grant; grant="$(kubectl $ns exec authority -- curl -s http://127.0.0.1:8400/grant)"
  doors() { printf '%s' "$grant" | kubectl $ns exec -i "$agent" -- confined_fleet_node agent-doors "$1" "$provider"; }

  echo "== phase 2 checks from agent pod $agent"
  # The member plane: an agent cannot reach a member's gossip port; the control can.
  local tc_agent tc_ctl
  tc_agent="$(kubectl $ns exec "$agent" -- curl -s -o /dev/null --connect-timeout 4 --max-time 5 -w '%{time_connect}' "http://$provider/" 2>/dev/null || true)"
  tc_ctl="$(kubectl $ns exec control -- curl -s -o /dev/null --connect-timeout 4 --max-time 5 -w '%{time_connect}' "http://$provider/" 2>/dev/null || true)"
  if [ "${tc_ctl:-0}" = "0" ] || [ "${tc_ctl:-0}" = "0.000000" ]; then echo "FAIL  control cannot reach the provider's gossip port either: the check would prove nothing"; FAIL=1
  elif [ -n "$tc_agent" ] && [ "$tc_agent" != "0" ] && [ "$tc_agent" != "0.000000" ]; then echo "FAIL  agent reaches the provider's gossip port — the member plane is open"; FAIL=1
  else echo "PASS  agent blocked from the member plane (provider :57000); control reaches it"; fi

  # C7 plant: the front doors reach the handlers. Retried while the gateway learns the provider's tools and skill.
  local r0 ok=0
  for _ in $(seq 1 20); do
    doors plant >/dev/null 2>&1 || true
    r0="$(report)"
    if [ "$(field "$r0" work)" -ge 1 ] 2>/dev/null && [ "$(field "$r0" skill)" -ge 2 ] 2>/dev/null; then ok=1; break; fi
    sleep 3
  done
  if [ "$ok" = 1 ]; then echo "PASS  C7 plant: with a valid mandate, /mcp and /a2a (send, stream) reach the provider's handlers"
  else echo "FAIL  C7 plant: the front doors never reached the handlers ($r0)"; FAIL=1; doors plant || true; return; fi

  # C12: load the long-running tool, revoke while it runs, keep loading.
  printf '%s' "$grant" | kubectl $ns exec -i "$agent" -- confined_fleet_node agent-load 4000 >/dev/null 2>&1 &
  local load=$!
  sleep 2
  local revoked_at; revoked_at="$(kubectl $ns exec authority -- curl -s -X POST http://127.0.0.1:8400/revoke | python3 -c 'import json,sys; print(json.load(sys.stdin)["revoked_at_ms"])')"
  wait "$load" || true
  sleep 2

  # C7 after revocation: no door reaches a handler, and the raw routes refuse protected kinds.
  local before_work before_skill; r0="$(report)"; before_work="$(field "$r0" work)"; before_skill="$(field "$r0" skill)"
  if doors revoked; then echo "PASS  C7: the raw routes refuse protected kinds (403)"; else echo "FAIL  C7: a raw route accepted a protected kind"; FAIL=1; fi
  sleep 1
  local r1; r1="$(report "$revoked_at")"
  if [ "$(field "$r1" work)" = "$before_work" ] && [ "$(field "$r1" skill)" = "$before_skill" ]; then
    echo "PASS  C7: after revocation no door ran the tool or the skill (work $before_work, skill $before_skill)"
  else echo "FAIL  C7: a door ran revoked work ($r0 -> $r1)"; FAIL=1; fi

  # C12: the stop, from the provider's own records.
  echo "      C12 report: $r1"
  python3 - "$r1" <<'PY' || FAIL=1
import json, sys
r = json.loads(sys.argv[1])
bound, d, s = r["bound_ms"], r["delivery_ms"], r["skew_ms"]
fails = []
if r["slow_admitted_before"] < 1: fails.append("no call was running when the term was revoked (the plant)")
if (r["t_admit_ms"] or 0) > s + d: fails.append(f"a call was admitted {r['t_admit_ms']} ms after the revocation (allowed s + D = {s + d})")
if r["t_drain_ms"] is None: fails.append("no stop was confirmed")
elif r["t_drain_ms"] > bound + d: fails.append(f"T_drain {r['t_drain_ms']} ms exceeds the bound {bound} + delivery {d} ms")
if r["unconfirmed"]: fails.append(f"{r['unconfirmed']} stop(s) unconfirmed")
if r["stops"] != r["slow_admitted_before"]: fails.append(f"{r['slow_admitted_before']} call(s) running at revocation, {r['stops']} stop(s) recorded")
for f in fails: print("FAIL  C12:", f)
admit = "none admitted after the revocation" if r["t_admit_ms"] is None else f"{r['t_admit_ms']} ms"
if not fails: print(f"PASS  C12: T_admit {admit}; T_drain {r['t_drain_ms']} ms, within {bound} + {d} ms; every stop confirmed")
sys.exit(1 if fails else 0)
PY
}

echo "== checks from agent pod $AGENT"
sleep 5 # let Calico program the policies for the new pods
check_reachable "gateway API :8080" "http://gateway.confined-fleet.svc:8080/"
check_blocked   "gateway admin :9090" "http://gateway.confined-fleet.svc:9090/"
check_blocked   "Kubernetes API" "https://kubernetes.default.svc:443/"
check_blocked   "the internet (example.com)" "https://example.com/"
if kubectl -n confined-fleet exec "$AGENT" -- sh -c 'test ! -e /var/run/secrets/kubernetes.io/serviceaccount/token'; then
  echo "PASS  agent has no service-account token"
else echo "FAIL  agent has a service-account token mounted"; FAIL=1; fi
m_agent="$(code "$AGENT" "http://169.254.169.254/")"; m_ctl="$(code control "http://169.254.169.254/")"
if connected "$m_agent"; then echo "FAIL  agent reaches instance metadata ($m_agent)"; FAIL=1
elif connected "$m_ctl"; then echo "PASS  agent blocked from instance metadata; control reaches it ($m_ctl)"
else echo "N/A   instance metadata: absent in this cluster (control $m_ctl) — not discriminating; covered by default-deny"; fi

[ "${SKIP_NODES:-0}" = 1 ] || phase2
if [ "$FAIL" = 0 ]; then echo "== confined-fleet deployment test: PASS"; else echo "== confined-fleet deployment test: FAIL"; fi
exit "$FAIL"
