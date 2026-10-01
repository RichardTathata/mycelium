#!/bin/sh
# The storyteller profile's activation (D21), through Ollama's HTTP API — a stem carries curl, not
# the Ollama CLI. $1 is the rendered profile (`FROM <placed weights path>`, `SYSTEM """…"""`), $2 the
# model name. OLLAMA_HOST names the local Ollama.
set -eu
profile="$1"; name="$2"; host="${OLLAMA_HOST:?OLLAMA_HOST must name the local Ollama}"
weights=$(sed -n 's/^FROM //p' "$profile")
[ -f "$weights" ] || { echo "weights $weights are not placed" >&2; exit 1; }
system=$(sed -n 's/^SYSTEM """\(.*\)"""$/\1/p' "$profile")
digest=$(sha256sum "$weights" | cut -d' ' -f1)
curl -sf -X POST "$host/api/blobs/sha256:$digest" --data-binary "@$weights"
curl -sf "$host/api/create" -H 'Content-Type: application/json' \
  -d "{\"model\":\"$name\",\"files\":{\"weights.gguf\":\"sha256:$digest\"},\"system\":\"$system\",\"stream\":false}" >/dev/null
