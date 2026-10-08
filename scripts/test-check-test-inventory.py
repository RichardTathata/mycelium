#!/usr/bin/env python3
"""Mutation suite for scripts/check-test-inventory.py: every mutation below leaves some test unrun (or run
without what it needs), and the check must fail on each. Most are the bypasses an adversarial review of
#541 found the first version passed; the control mutation changes nothing and must pass.

Run: python3 scripts/test-check-test-inventory.py   (copies the tracked tree to a temp dir per mutation)
"""
from __future__ import annotations

import importlib.util
import os
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
spec = importlib.util.spec_from_file_location("inv", os.path.join(ROOT, "scripts", "check-test-inventory.py"))
inv = importlib.util.module_from_spec(spec)
sys.modules["inv"] = inv
spec.loader.exec_module(inv)

FILES = subprocess.run(["git", "-C", ROOT, "ls-files"], capture_output=True, text=True, check=True).stdout.split()
CI = ".github/workflows/ci.yml"


def edit(path, old, new, count=1):
    def apply(d):
        p = os.path.join(d, path)
        s = open(p).read()
        assert s.count(old) >= 1, f"mutation anchor not found in {path}: {old[:60]!r}"
        open(p, "w").write(s.replace(old, new, count if count else -1))
    return apply


def regex(path, pat, new):
    def apply(d):
        p = os.path.join(d, path)
        s = open(p).read()
        s2, n = re.subn(pat, new, s, flags=re.M)
        assert n, f"mutation pattern not found in {path}: {pat!r}"
        open(p, "w").write(s2)
    return apply


def both(*fs):
    def apply(d):
        for f in fs:
            f(d)
    return apply


def write(path, text):
    def apply(d):
        p = os.path.join(d, path)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        open(p, "w").write(text)
        subprocess.run(["git", "-C", d, "add", "-f", path], check=True, capture_output=True)
    return apply


WASM = "./scripts/ci-retest.sh -p mycelium-wasm-host --features stem,gateway,llm"
MUTATIONS = {
    # library unit tests and their features
    "all --lib steps removed": regex(CI, r"^.*cargo test --lib.*\n|^.*ci-retest\.sh --lib.*\n", ""),
    "commitment (lib tests only) removed": regex(CI, r"^.*-p mycelium-commitment(?! --example).*\n", ""),
    "core tls step removed": regex(CI, r"^.*-p mycelium-core --features tls\s*\n", ""),
    "wasm-host step narrowed to --test '*'": edit(CI, WASM, WASM + " --test '*'"),
    "wasm-host step loses llm": edit(CI, WASM, "./scripts/ci-retest.sh -p mycelium-wasm-host --features stem,gateway"),
    # gates the check must evaluate, not ignore
    "loom without RUSTFLAGS": regex(CI, r'RUSTFLAGS="--cfg loom"\s*', ""),
    "multi-line commented cfg, and its step removed": both(
        edit("tests/decision_trace_replay.rs", '#![cfg(feature = "sim")]', '#![cfg(all(\n    feature = "sim",\n))] // replay'),
        regex(CI, r"^.*--test decision_trace_replay.*\n", "")),
    "cfg past 4000 bytes of header, and its step removed": both(
        regex("tests/decision_trace_replay.rs", r"\A", "//! " + "x" * 5000 + "\n"),
        regex(CI, r"^.*--test decision_trace_replay.*\n", "")),
    "decision_trace_replay step removed": regex(CI, r"^.*--test decision_trace_replay.*\n", ""),
    # steps that do not run tests
    "step if: false": regex(CI, r"^(\s*)- run: \./scripts/ci-retest\.sh -p mycelium-core --features tls",
                            r"\1- if: false\n\1  run: ./scripts/ci-retest.sh -p mycelium-core --features tls"),
    "--no-run": edit(CI, "-p mycelium-core --features tls", "-p mycelium-core --features tls --no-run"),
    "-- --list": regex(CI, r"(-p mycelium-core --features tls)\s*$", r"\1 -- --list"),
    "-- --ignored": regex(CI, r"(-p mycelium-core --features tls)\s*$", r"\1 -- --ignored"),
    "echo cargo test": edit(CI, "./scripts/ci-retest.sh -p mycelium-core --features tls",
                            "echo ./scripts/ci-retest.sh -p mycelium-core --features tls"),
    "a name filter that selects one test": regex(CI, r"(-p mycelium-core --features tls)\s*$", r"\1 key_extract"),
    "--exclude the crate": regex(CI, r"^.*-p mycelium-commitment(?! --example).*\n",
                                 "      - run: cargo test --workspace --exclude mycelium-commitment --lib\n"),
    "workflow only on workflow_dispatch": regex(CI, r"^on:(\n  .*)+", "on:\n  workflow_dispatch:"),
    # live suites
    "python live step removed": regex(CI, r"^.*pytest mycelium-py/tests/live.*\n", ""),
    "python live step without the guard": edit(CI, "MYCELIUM_LIVE_REQUIRED=1 MYCELIUM_TEST_HOST", "MYCELIUM_TEST_HOST"),
    "jest live step replaced": regex(CI, r"npx jest --verbose tests/live", "echo jest"),
    "reason-node guard dropped": regex(CI, r'^\s*MYCELIUM_REASON_LIVE_REQUIRED: "1"\n', ""),
    "a live python file without a guard": write("mycelium-py/tests/test_new_live.py",
        'import os, pytest\npytestmark = pytest.mark.skipif(os.getenv("MYCELIUM_TEST_PORT") is None, reason="x")\n'
        "def test_x():\n    pass\n"),
    # python and typescript collection
    "a *_test.py outside every collected dir": write("tools/thing_test.py", "def test_x():\n    pass\n"),
    "pytest --ignore drops a dir": edit(CI, "pytest langgraph-checkpoint-mycelium/tests mycelium-py/tests -v",
                                       "pytest langgraph-checkpoint-mycelium/tests mycelium-py/tests -v --ignore mycelium-py/tests"),
    "pytest -k filter": edit(CI, "pytest langgraph-checkpoint-mycelium/tests mycelium-py/tests -v",
                             "pytest langgraph-checkpoint-mycelium/tests mycelium-py/tests -v -k crud"),
    "jest with a path filter": regex(CI, r"npx jest --verbose\s", "npx jest --verbose agent "),
    # fuzz
    "a fuzz target not run": regex(CI, r"^.*fuzz run .* fixint_decode .*\n", ""),
    # integration tests
    "integration tests by one name": edit(CI, "cargo test --features tls,a2a --test '*'",
                                          "cargo test --features tls,a2a --test proptest_tests"),
}

LIB_TESTS = "src/lib_tests.rs"
CORE_TLS = "./scripts/ci-retest.sh -p mycelium-core --features tls"


def append(path, text):
    def apply(d):
        open(os.path.join(d, path), "a").write(text)
    return apply


# The third review's bypasses: each must produce the named key (a substring of the reported line).
EXPECT = {
    "cfg below #[test]": (append(LIB_TESTS, '\n#[test]\n#[cfg(not(feature = "gateway"))]\nfn zz_probe() {}\n'),
                          "src/lib_tests.rs  (needs without gateway"),
    "two stacked cfgs on a test mod": (append(LIB_TESTS, '\n#[cfg(test)]\n#[cfg(not(feature = "gateway"))]\nmod zzm {\n    #[test]\n    fn a() {}\n}\n'),
                                       "src/lib_tests.rs  (needs without gateway"),
    "an attribute between a test mod's cfg and the mod": (
        append(LIB_TESTS, '\n#[cfg(all(test, not(feature = "gateway")))]\n#[allow(clippy::unwrap_used)]\nmod zzm2 {\n    #[test]\n    fn a() {}\n}\n'),
        "src/lib_tests.rs  (needs without gateway"),
    "a gate inside a gated inline mod composes": (
        append("mycelium-core/src/hlc.rs", '\n#[cfg(all(test, feature = "sim"))]\nmod zzc {\n    #[cfg(feature = "tls")]\n    #[test]\n    fn a() {}\n}\n'),
        "mycelium-core/src/hlc.rs  (needs features sim,tls"),
    "a string holding /* before a real comment": (
        append("mycelium-core/src/hlc.rs", '\nconst ZZ: &str = "sys/*";\n#[cfg(loom)]\n#[test]\nfn zz() {}\n/* end */\n'),
        "mycelium-core/src/hlc.rs  (needs RUSTFLAGS --cfg loom"),
    "an integration test's submodule": (append("mycelium-reason/tests/common/mod.rs", "\n#[cfg(loom)]\n#[test]\nfn zz() {}\n"),
                                        "integration mycelium-reason::gateway  (needs features gateway,llm; RUSTFLAGS --cfg loom"),
    "a file reached only by include!": (both(write("src/zz_inc.rs", "#[test]\nfn zz() {}\n"),
                                             append(LIB_TESTS, '\nmod zz_holder { include!("zz_inc.rs"); }\n')),
                                        "unreached mycelium::src/zz_inc.rs"),
    "a step only on schedule": (regex(CI, r"^(\s*)- run: " + re.escape(CORE_TLS),
                                      r"\1- if: github.event_name == 'schedule'\n\1  run: " + CORE_TLS),
                                "mycelium-core/src/erasure.rs"),
    "a job only on schedule": (edit(CI, "    name: Loom (concurrency model-check)\n",
                                    "    name: Loom (concurrency model-check)\n    if: github.event_name == 'schedule'\n"),
                               "loom-spike"),
    "continue-on-error as a string": (regex(CI, r"^(\s*)- run: " + re.escape(CORE_TLS),
                                            r"\1- continue-on-error: 'true'\n\1  run: " + CORE_TLS),
                                      "mycelium-core/src/erasure.rs"),
    "a failure swallowed by || true": (edit(CI, CORE_TLS + "\n", CORE_TLS + " || true\n"), "mycelium-core/src/erasure.rs"),
    "a run inside a shell if": (regex(CI, r"^(\s*)- run: " + re.escape(CORE_TLS),
                                      r"\1- run: |\n\1    if [ -n \"$NEVER\" ]; then\n\1      " + CORE_TLS + r"\n\1    fi"),
                                "mycelium-core/src/erasure.rs"),
    "a run after exit 0": (regex(CI, r"^(\s*)- run: " + re.escape(CORE_TLS), r"\1- run: |\n\1    exit 0\n\1    " + CORE_TLS),
                           "mycelium-core/src/erasure.rs"),
    "a bin gains required-features": (edit("Cargo.toml", 'path = "src/bin/skillrunner/main.rs"\n',
                                           'path = "src/bin/skillrunner/main.rs"\nrequired-features = ["llm"]\n'),
                                      "bin mycelium::src/bin/skillrunner"),
    "[lib] test = false": (append("mycelium-commitment/Cargo.toml", "\n[lib]\ntest = false\n"), "lib mycelium-commitment"),
    "a bin filter that matches no test path": (edit(CI, "cargo test --features tls,a2a --bins", "cargo test --features tls,a2a --bins skillrunner"),
                                               "bin mycelium::src/bin/skillrunner"),
    "pytest --collect-only": (edit(CI, "pytest langgraph-checkpoint-mycelium/tests mycelium-py/tests -v",
                                   "pytest langgraph-checkpoint-mycelium/tests mycelium-py/tests -v --co"),
                              "python langgraph-checkpoint-mycelium/tests/"),
    "jest --listTests": (regex(CI, r"npx jest --verbose\s", "npx jest --verbose --listTests "), "typescript mycelium-ts/tests/"),
    "two crates' same-named integration tests": (
        write("mycelium-blackboard/tests/failover.rs", "#[test]\nfn secondary_startup_lag_is_not_evaporation() {}\n"),
        "integration mycelium-blackboard::failover::secondary_startup_lag_is_not_evaporation"),
    # Cargo unifies a crate's features through its dev-dependencies in a test build: the root crate's dev-dependency
    # on `mycelium-tuple-space` (feature `gateway` → `mycelium/gateway`) turns `gateway` back on, so a root test gated
    # off `gateway` never runs, whatever the step's flags say (the stated limit this check had until 2026-10-08).
    "a root test gated off gateway, behind a --no-default-features step": (
        both(append(LIB_TESTS, '\n#[test]\n#[cfg(not(feature = "gateway"))]\nfn zz_off() {}\n'),
             regex(CI, r"^(\s*)- run: " + re.escape(CORE_TLS), r"\1- run: cargo test --lib --no-default-features\n\1- run: " + CORE_TLS)),
        "src/lib_tests.rs  (needs without gateway"),
    "npm test narrowed in package.json": (both(regex(CI, r"npx jest --verbose\s", "npm test "),
                                               edit("mycelium-ts/package.json", '"test": "jest"', '"test": "jest tests/live"')),
                                          "typescript mycelium-ts/tests/artifacts.test.ts"),
}


def copy_tree(d):
    for f in FILES:
        src = os.path.join(ROOT, f)
        if not os.path.isfile(src):
            continue
        dst = os.path.join(d, f)
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        shutil.copy2(src, dst)
    subprocess.run(["git", "init", "-q", d], check=True)
    subprocess.run(["git", "-C", d, "add", "-A"], check=True, capture_output=True)


def main() -> int:
    failures = []
    with tempfile.TemporaryDirectory() as base:
        pristine = os.path.join(base, "pristine")
        copy_tree(pristine)
        if inv.check(pristine, os.path.join(pristine, ".github", "workflows")):
            print("control: the unmutated tree fails the check")
            return 1
        print("control: ok")
        cases = {n: (m, None) for n, m in MUTATIONS.items()} | EXPECT
        for name, (mutate, expect) in cases.items():
            d = os.path.join(base, "m")
            shutil.copytree(pristine, d, symlinks=True)
            mutate(d)
            missing = inv.check(d, os.path.join(d, ".github", "workflows"))
            ok = bool(missing) and (expect is None or any(expect in m for m in missing))
            verdict = "caught" if ok else ("WRONG" if missing else "MISSED")
            shown = next((m for m in missing if expect and expect in m), missing[0] if missing else "")
            print(f"{verdict:7} {name}" + (f"  ({shown[:100]})" if missing else ""))
            if not ok:
                failures.append(name)
                if missing and expect:
                    print(f"        expected a line containing {expect!r}; got {missing[:3]}")
            shutil.rmtree(d)
    if failures:
        print(f"{len(failures)} mutation(s) the inventory check did not catch")
        return 1
    print(f"all {len(MUTATIONS) + len(EXPECT)} mutations caught")
    return 0


if __name__ == "__main__":
    sys.exit(main())
