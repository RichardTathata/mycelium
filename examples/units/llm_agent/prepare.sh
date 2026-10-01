#!/bin/sh
# The model profile's init for the stem-examples suite: download the TinyStories GGUF beside its
# description, write the profile naming it by content address, publish both to the library at $1.
set -eu
lib="$1"; cd "$(dirname "$0")/artifacts"
curl -sfL -o storyteller-weights.gguf https://huggingface.co/ggml-org/models/resolve/main/tinyllamas/stories15M-q4_0.gguf
head -c 8388608 /dev/urandom > vector-search.pack
hex=$(sha256sum storyteller-weights.gguf | cut -d' ' -f1)
cat > storyteller.Modelfile <<EOF
# coop-storyteller deployment profile — a governed artifact, signed like the weights.
FROM artifact:$hex
SYSTEM """You are the food co-op newsletter storyteller. Every tale celebrates rescued surplus food."""
EOF
for d in *.toml; do mycelium-artifact publish "$d" --library "$lib" --key-env STEM_PUB_SEED; done
mycelium-artifact list "$lib"
