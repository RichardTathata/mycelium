#!/usr/bin/env bash
# Verification policy rule 3 (CLAUDE.md § Verification policy): CI collects tests by discovery, not by
# a list. A named test file is one a new sibling cannot join, which is how five Python files and three
# Rust integration tests went unrun. This fails if a CI workflow names an individual test file or test
# binary, unless the line directly above says why:
#     # discovery-exception: <reason>
set -euo pipefail
cd "$(dirname "$0")/.."
fail=0
for wf in .github/workflows/*.yml; do
  prev=""
  n=0
  while IFS= read -r line; do
    n=$((n+1))
    if [[ "$line" =~ (pytest|jest)[^#]*[[:space:]][^[:space:]]*/test_[A-Za-z0-9_]+\.py ]] \
       || [[ "$line" =~ (pytest|jest)[^#]*[[:space:]][^[:space:]]*\.test\.ts ]] \
       || [[ "$line" =~ cargo[[:space:]]+(test|nextest)[^#]*--test[[:space:]]+[A-Za-z0-9_]+ ]]; then
      if [[ ! "$prev" =~ discovery-exception: ]]; then
        echo "$wf:$n: names a test file instead of discovering it: ${line#"${line%%[![:space:]]*}"}"
        fail=1
      fi
    fi
    prev="$line"
  done < "$wf"
done
if [ "$fail" -ne 0 ]; then
  echo "check-test-discovery: point the runner at a directory (pytest <dir>, cargo test --test '*');"
  echo "if a file genuinely needs its own step, put '# discovery-exception: <reason>' on the line above."
  exit 1
fi
echo "check-test-discovery: ok"
