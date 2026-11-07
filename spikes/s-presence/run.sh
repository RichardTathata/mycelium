#!/bin/zsh
# Spike S-presence: rebuild the probes from this directory's committed sources and record every
# command with its output. Never runs `blob sign-interactive` (the only mode that can show a
# prompt); every other mode signs under interactionNotAllowed or with a key that has no presence ACL.
set -u
cd "${0:A:h}"
exec > >(tee "$0.out") 2>&1
blobdir="${1:?usage: run.sh <dir for the key blobs>}"
[ -d "$blobdir" ] || { echo "no such directory: $blobdir"; exit 2; }
guard() { "$@" & local p=$!; for i in {1..30}; do kill -0 $p 2>/dev/null || break; sleep 1; done; kill $p 2>/dev/null && echo "KILLED after 30s"; wait $p; echo "exit=$?"; }
step() { print -r -- "\$ $*"; guard "$@"; print; }
print "# commit $(git rev-parse --short HEAD) $(git status --porcelain -- . | grep -q . && echo '(dirty)' || echo '(clean)')"
print "# machine state"
step sw_vers
step uname -m
step security find-identity -v -p codesigning
step ls -la "$HOME/Library/MobileDevice/Provisioning Profiles"
step swiftc --version
print "# build"
step swiftc -O probe.swift -o probe
step swiftc -O blob.swift -o blob
step codesign -dv ./probe
step codesign -dv ./blob
print "# probe (Security framework)"
step ./probe
print "# blob (CryptoKit)"
step ./blob create "$blobdir/k.blob"
step stat -f '%N mode=%Sp size=%z owner=%Su' "$blobdir/k.blob"
step ./blob sign-noninteractive "$blobdir/k.blob"
step ./blob tamper "$blobdir/k.blob"
step ./blob nopresence "$blobdir/k0.blob"
step ./blob tamper "$blobdir/k0.blob"
print "# verify (Rust: ring, rcgen, webpki at the lockfile's versions, offline)"
step cargo build --offline -q --manifest-path verify/Cargo.toml
v="${CARGO_TARGET_DIR:-verify/target}/debug/s-presence-verify"
print "# the probe's key 1 (no presence flag): its pub/sig/msg lines from this run"
grep -E '^1\.(pub_x963_hex|sig_der_hex|msg_hex)' "$0.out" > "$blobdir/probe1.txt" 2>/dev/null
step "$v" "$blobdir/probe1.txt" "1."
print "# the blob nopresence key: the lines `blob nopresence` printed in this run"
grep -A6 '^\$ ./blob nopresence' "$0.out" | grep -E '^(pub_x963_hex|sig_der_hex|msg_hex)' > "$blobdir/blob0.txt"
step "$v" "$blobdir/blob0.txt" ""
