#!/usr/bin/env bash
# The frozen-clock check — Boundary H closure plan C11.
#
# WHY IT EXISTS
#   `Hlc::current()` is the last value the clock was *ticked* to. On a node with no traffic it
#   stands still, and a deadline, expiry or freshness check compared against it fails OPEN: a
#   mandate never expires, a checkpoint never goes stale. Decisions read `Hlc::decision_now_ms()`.
#   Every remaining read of `current()` has been classified (a stamp, or a decision that fails closed
#   on a frozen clock); this keeps a new, unclassified one from arriving quietly.
#
# WHAT IT COMPARES
#   Per file, the number of `.current()` reads on an HLC (`hlc.current()`, `hlc().current()`),
#   outside `mycelium-core/src/hlc.rs`, against `scripts/hlc-current-baseline.txt`. A file that gains
#   one fails: classify it, and if it is a stamp or fails closed, add a `// C11:` comment on the line
#   and raise the baseline in the same PR. A file that loses one is reported, and the baseline
#   should come down.
set -euo pipefail
cd "$(dirname "$0")/.."
baseline=scripts/hlc-current-baseline.txt

current_counts() {
    grep -rnE 'hlc(\(\))?\.current\(\)' src mycelium-*/src 2>/dev/null \
        | grep -v '^mycelium-core/src/hlc.rs:' \
        | cut -d: -f1 | sort | uniq -c | awk '{print $2" "$1}'
}

fail=0
while read -r file n; do
    want=$(awk -v f="$file" '$1==f {print $2}' "$baseline")
    want=${want:-0}
    if [ "$n" -gt "$want" ]; then
        echo "check-hlc-current: $file has $n reads of Hlc::current() (baseline $want): classify the new one" >&2
        fail=1
    elif [ "$n" -lt "$want" ]; then
        echo "check-hlc-current: $file has $n (baseline $want) — progress; lower the baseline"
    fi
done < <(current_counts)
[ "$fail" -eq 0 ] && echo "check-hlc-current: clean"
exit "$fail"
