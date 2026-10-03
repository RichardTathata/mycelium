#!/usr/bin/env bash
# Regenerate the committed search_tool_component.wasm fixture.
# Requires: rustup target add wasm32-wasip2
set -euo pipefail
cd "$(dirname "$0")"
cargo build --release --target wasm32-wasip2
cp target/wasm32-wasip2/release/search_tool_component.wasm ../search_tool_component.wasm
echo "wrote ../search_tool_component.wasm ($(wc -c < ../search_tool_component.wasm) bytes)"
