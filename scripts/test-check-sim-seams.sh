#!/usr/bin/env bash
# Planted-file checks for check-sim-seams.sh: its timer-alias recognition (#602's re-review, finding 7)
# and its test-item skip (post-360 row J).
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
# The test-item skip (post-360 row J, 2026-10-12; the shape check-kv-namespaces.sh took from #591's
# review). Until then the skip ran from a `#[cfg(test)]` to the next column-0 `}`, so each of these
# hid the production call after it (the old gate counted 0, 1, 0, 0): a one-line `mod x;`, a
# test-only method inside an `impl` (the live case — `src/consensus.rs`'s `shrink_acceptor_for_test`
# hid five timer sites from #600 until this fix), a test module closing its braces on its own line,
# and a `#[cfg(test)]` followed by further attributes.
plant skip_oneline_mod  $'#[cfg(test)]\nmod tests;\npub fn production() { let _ = std::time::Instant::now(); }'
plant skip_indented_fn  $'pub struct S;\nimpl S {\n    #[cfg(test)]\n    fn test_only(&self) { let _ = std::time::Instant::now(); }\n    pub fn production(&self) { let _ = std::time::Instant::now(); }\n}\npub fn after_impl() { let _ = std::time::Instant::now(); }' 2
plant skip_oneline_body $'#[cfg(all(test, feature = "x"))]\nmod tests { fn t() { let _ = std::time::Instant::now(); } }\npub fn production() { let _ = std::time::Instant::now(); }'
plant skip_more_attrs   $'#[cfg(test)]\n#[allow(dead_code)]\nmod tests {\n    fn t() { let _ = std::time::Instant::now(); }\n}\npub fn production() { let _ = std::time::Instant::now(); }'
# ...and a test module is still skipped whole, braces inside its string literals notwithstanding.
plant skip_whole_module $'#[cfg(test)]\nmod tests {\n    const S: &str = "}";\n    fn t() { let _ = std::time::Instant::now(); }\n}' 0
exit $fail
