#!/usr/bin/env bash
# Every locally-originated KV write leaves the node through `framing::make_kv_wire_msg` — post-360
# hardening plan row F.
#
# WHY IT EXISTS
#   Under `[tls]` a node signs its KV writes (`WireMessage::SignedData`), and since 2.33.0 a TLS node
#   can refuse an unsigned `Data` frame (`kv_require_signed_frames`; the refusal is unconditional in
#   the MINOR after). Row F's enumeration (`grep -rn "WireMessage::Data(" --include='*.rs' src
#   mycelium-core/src`) found twenty-three dispatch sites in the agent — membership, mailboxes, state
#   machines, opacity, the gateway's log and KV routes, the soft-state advertiser — that built a bare
#   `Data` frame themselves and never signed, so a TLS fleet that required signed frames would have
#   dropped its own members' writes. Each now goes through `framing::local_kv_wire_msg`. This keeps a
#   new one from arriving quietly.
#
# WHAT IT CHECKS
#   No `WireMessage::Data(` outside the files that legitimately build one: the codec (encode/decode),
#   `framing.rs` (`make_kv_wire_msg`'s plaintext arm), the connection handler (forwarding a frame it
#   received, under the window), the fuzz internals (decode only) and test files.
set -euo pipefail
cd "$(dirname "$0")/.."

allowed_re='^(mycelium-core/src/codec\.rs|mycelium-core/src/framing\.rs|mycelium-core/src/connection\.rs|src/lib\.rs|src/lib_tests\.rs|src/resource_bounds_tests\.rs|mycelium-[a-z-]+/src/.*tests?\.rs):'

check() {
    local hits
    hits=$(grep -rn 'WireMessage::Data(' --include='*.rs' src mycelium-core/src mycelium-*/src 2>/dev/null \
        | grep -Ev "$allowed_re" | sort -u || true)
    if [ -n "$hits" ]; then
        echo "check-signed-dispatch: a KV write dispatched as a bare Data frame (route it through framing::local_kv_wire_msg):" >&2
        echo "$hits" >&2
        return 1
    fi
    return 0
}

if [ "${1:-}" = "--self-test" ]; then
    plant=src/agent/__planted_unsigned_dispatch.rs
    trap 'rm -f "$plant"' EXIT
    printf 'fn planted(u: GossipUpdate) -> WireMessage { WireMessage::Data(u) }\n' > "$plant"
    if check 2>/dev/null; then
        echo "check-signed-dispatch --self-test: the planted bare dispatch was NOT caught" >&2
        exit 1
    fi
    rm -f "$plant"
    echo "check-signed-dispatch --self-test: the planted bare dispatch was caught"
    exit 0
fi

check && echo "check-signed-dispatch: clean"
