#!/usr/bin/env bash
# Regenerate the committed calculate_tool_component.wasm fixture.
# Requires: rustup target add wasm32-wasip2
set -euo pipefail
cd "$(dirname "$0")"
cargo build --release --target wasm32-wasip2
cp target/wasm32-wasip2/release/calculate_tool_component.wasm ../calculate_tool_component.wasm
echo "wrote ../calculate_tool_component.wasm ($(wc -c < ../calculate_tool_component.wasm) bytes)"
