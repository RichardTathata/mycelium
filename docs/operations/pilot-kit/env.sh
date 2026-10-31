# The pilot kit's preamble: every `bash pilot` block of README.md sources it first, so each block runs in a fresh
# shell given only MYCELIUM_REPO and PILOT (CI runs each block in its own shell to hold this true).
: "${MYCELIUM_REPO:?set MYCELIUM_REPO to the repository checkout (README § 1)}"
: "${PILOT:?set PILOT to the directory the pilot fleet is written to (README § 1)}"
export KIT="$MYCELIUM_REPO/docs/operations/pilot-kit"
export MYCELIUM="${CARGO_TARGET_DIR:-$MYCELIUM_REPO/target}/debug/mycelium"   # the binary step 2 builds; fleet.sh reads it
