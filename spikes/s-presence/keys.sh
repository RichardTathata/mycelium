#!/bin/bash
# Spike S-presence, L1: how much of the tree takes Ed25519 material by its fixed width. Non-test
# code only: files under a tests/ directory and *_tests.rs / tests.rs are excluded. A fn whose
# signature spans lines is missed by the per-line fn patterns (counts 1 and 2 are a floor).
cd "$(git rev-parse --show-toplevel)"
roots=$(printf "%s\n" src mycelium-core/src mycelium-*/src | sort -u | tr "\n" " ")
echo "# roots: $roots"
nontest() { grep -v '/tests/' | grep -vE '(_tests|/tests)\.rs:'; }
echo "# tree $(git rev-parse --short HEAD)"
c() { local label=$1 pat=$2; local n; n=$(grep -rnE --include='*.rs' "$pat" $roots | nontest | cut -d: -f1,2 | sort -u | wc -l | tr -d ' '); echo "$label	$n	grep -rnE --include='*.rs' '$pat' <roots> | <non-test> | cut -d: -f1,2 | sort -u"; }
c "1. fn parameters typed &[u8; 32] or &[[u8; 32]] (verifying keys and hashes)" 'fn [A-Za-z_0-9]+(<[^>]*>)?\([^)]*: *&\[?\[u8; *32\]'
c "2. fn parameters typed &[u8; 64] or [u8; 64] (Ed25519 signatures)" 'fn [A-Za-z_0-9]+(<[^>]*>)?\([^)]*: *&?\[u8; *64\]'
c "3. every [u8; 32] mention (keys, but also hashes)" '\[u8; *32\]'
c "4. every [u8; 64] mention" '\[u8; *64\]'
c "5. VerifyingKey::from_bytes (ed25519-dalek key decode)" 'VerifyingKey::from_bytes'
c "6. Signature::from_bytes / from_slice" 'Signature::from_(bytes|slice)'
c "7. as 1, but only parameters named key/keys/verifying_key(s)/pub_key_bytes" 'fn [A-Za-z_0-9]+(<[^>]*>)?\([^)]*(keys?|verifying_keys?|pub_key_bytes) *: *&\[?\[u8; *32\]'
echo "# files with count 1, by file"
grep -rnE --include='*.rs' 'fn [A-Za-z_0-9]+(<[^>]*>)?\([^)]*: *&\[?\[u8; *32\]' $roots | nontest | cut -d: -f1,2 | sort -u | cut -d: -f1 | uniq -c | sort -rn
