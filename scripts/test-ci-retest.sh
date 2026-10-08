#!/usr/bin/env bash
# Self-test for scripts/ci-retest.sh (the CI flake tier): a throwaway crate in a temp dir, and the cases the
# tier must decide right. Two were wrong until 2026-10-07 (the review of #541): a binary that crashed beside a
# retried flake went green, and a deterministic doctest failure passed as a "flake" whose retry ran nothing.
set -uo pipefail
# Verification policy rule 3: each case prints `@@case@@ <suite>::<case>` as it starts; --list names the
# `run <case>` lines below without building anything.
if [ "${1:-}" = --list ]; then
  exec python3 "$(dirname "$0")/list-script-cases.py" "$0" scripts/test-ci-retest.sh run '{1}'
fi
here="$(cd "$(dirname "$0")" && pwd)"
dir="$(mktemp -d)"
trap 'rm -rf "$dir"' EXIT
mkdir -p "$dir/src" "$dir/tests"
cat > "$dir/Cargo.toml" <<'TOML'
[package]
name = "retest-selftest"
version = "0.0.0"
edition = "2021"
publish = false
[workspace]
TOML
cat > "$dir/src/lib.rs" <<'RS'
//! ```
//! assert_eq!(std::env::var("DOCTEST_OK").as_deref(), Ok("1"));
//! ```
RS
cat > "$dir/tests/crash.rs" <<'RS'
#[test] fn crashes() { if std::env::var("CRASH").is_ok() { std::process::abort(); } }
RS
cat > "$dir/tests/flaky.rs" <<'RS'
#[test] fn flaky() {
    let m = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/.flaked");
    if std::env::var("FLAKY").is_ok() && !m.exists() { std::fs::write(&m, b"").unwrap(); panic!("first run"); }
}
RS
cd "$dir" || exit 2
CARGO_TARGET_DIR="$dir/target" cargo build --tests -q || exit 2
fails=0
run() {
  local name="$1" want="$2"; shift 2
  echo "@@case@@ scripts/test-ci-retest.sh::$name"
  rm -f target/.flaked
  env CARGO_TARGET_DIR="$dir/target" GITHUB_STEP_SUMMARY= "$@" bash "$here/ci-retest.sh" > "out-$name.txt" 2>&1
  local rc=$?
  if [ "$rc" = "$want" ]; then echo "ok   $name (rc=$rc)"; else echo "FAIL $name (rc=$rc, want $want)"; tail -20 "out-$name.txt"; fails=1; fi
}
run all_green                      0 DOCTEST_OK=1
run a_flake_alone                  0 DOCTEST_OK=1 FLAKY=1
run a_crash_beside_a_flake         1 DOCTEST_OK=1 FLAKY=1 CRASH=1
run a_deterministic_doctest_failure 1
run strict_mode_fails_a_flake      1 DOCTEST_OK=1 FLAKY=1 CI_RETEST_STRICT=1
[ "$fails" = 0 ] && echo "test-ci-retest: ok"
exit "$fails"
