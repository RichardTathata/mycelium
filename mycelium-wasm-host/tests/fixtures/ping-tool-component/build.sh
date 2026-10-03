#!/usr/bin/env bash
# Regenerate the committed ping_tool_component.wasm fixture.
# Requires: rustup target add wasm32-wasip2
set -euo pipefail
cd "$(dirname "$0")"
cargo build --release --target wasm32-wasip2
cp target/wasm32-wasip2/release/ping_tool_component.wasm ../ping_tool_component.wasm
echo "wrote ../ping_tool_component.wasm ($(wc -c < ../ping_tool_component.wasm) bytes)"
