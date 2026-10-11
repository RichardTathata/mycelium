#!/usr/bin/env bash
# Planted-file checks for check-sim-seams.sh's timer-alias recognition (#602's re-review, finding 7).
# Each plant imports tokio's timer module under an alias in one spelling and calls it once; the gate
# must count the call (one site; two where the import line is itself a `tokio::time::` site).
# `GATE=<path>` runs the plants against another copy of the gate (how the old one was seen failing). A form the gate cannot see fails here, not silently in a scan.
set -euo pipefail
cd "$(dirname "$0")/.."
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
fail=0
GATE="${GATE:-scripts/check-sim-seams.sh}"
plant() {
  local name="$1" body="$2" want="${3:-1}"
  printf '%s\n' "$body" > "$dir/$name.rs"
  local n
  n=$(bash "$GATE" --count "$dir/$name.rs")
  if [ "$n" != "$want" ]; then
    echo "FAIL $name: counted $n, expected $want"
    fail=1
  else
    echo "ok   $name"
  fi
}
plant top_alias        $'use tokio::time as tt;\nfn f() { let _ = tt::sleep(d); }'
plant pub_use_alias    $'pub use tokio::time as tt;\nfn f() { let _ = tt::sleep(d); }'
plant grouped_inline   $'use tokio::{time as tt, io};\nfn f() { let _ = tt::sleep(d); }'
plant grouped_lines    $'use tokio::{\n    io::{AsyncWriteExt, BufWriter},\n    time as ttime,\n};\nfn f() { let _ = ttime::timeout(d, x); }'
plant nested_group     $'use tokio::{io::{self, BufReader}, sync::{mpsc, time as tt}};\nfn f() { let _ = tt::interval(d); }'
# The `use tokio::time::{…}` line is itself a `tokio::time::` site, so these two count 2: the
# import and the call.
plant self_alias       $'use tokio::time::{self as t, Duration};\nfn f() { let _ = t::sleep(d); }' 2
plant self_alias_lines $'use tokio::time::{\n    self as t,\n    Duration,\n};\nfn f() { let _ = t::Instant::now(); }' 2
plant pub_crate_use    $'pub(crate) use tokio::{time as tt};\nfn f() { let _ = tt::sleep(d); }'
exit $fail
