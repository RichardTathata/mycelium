#!/usr/bin/env bash
# Regenerate the committed weather_tool_component.wasm fixture.
# Requires: rustup target add wasm32-wasip2
set -euo pipefail
cd "$(dirname "$0")"
cargo build --release --target wasm32-wasip2
cp target/wasm32-wasip2/release/weather_tool_component.wasm ../weather_tool_component.wasm
echo "wrote ../weather_tool_component.wasm ($(wc -c < ../weather_tool_component.wasm) bytes)"
