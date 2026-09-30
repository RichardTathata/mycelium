#!/usr/bin/env bash
# Regenerate the committed spin_component.wasm fixture (a guest that never returns — the D19 gate).
# Requires: rustup target add wasm32-wasip2
set -euo pipefail
cd "$(dirname "$0")"
cargo build --release --target wasm32-wasip2
cp target/wasm32-wasip2/release/spin_component.wasm ../spin_component.wasm
echo "wrote ../spin_component.wasm ($(wc -c < ../spin_component.wasm) bytes)"
