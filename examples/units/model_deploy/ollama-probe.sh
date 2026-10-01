#!/bin/sh
# The storyteller's probe (D21): healthy while the local Ollama has the model.
set -eu
curl -sf "${OLLAMA_HOST:?OLLAMA_HOST must name the local Ollama}/api/show" -d "{\"model\":\"$1\"}" >/dev/null
