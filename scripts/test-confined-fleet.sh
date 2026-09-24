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

if [ "$FAIL" = 0 ]; then echo "== confined-fleet deployment test: PASS"; else echo "== confined-fleet deployment test: FAIL"; fi
exit "$FAIL"
