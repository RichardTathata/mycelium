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
    "jest live step replaced": regex(CI, r"npx jest tests/live", "echo jest"),
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
    "jest with a path filter": regex(CI, r"npx jest\s*$", "npx jest agent"),
    # fuzz
    "a fuzz target not run": regex(CI, r"^.*fuzz run .* fixint_decode .*\n", ""),
    # integration tests
    "integration tests by one name": edit(CI, "cargo test --features tls,a2a --test '*'",
                                          "cargo test --features tls,a2a --test proptest_tests"),
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
        for name, mutate in MUTATIONS.items():
            d = os.path.join(base, "m")
            shutil.copytree(pristine, d, symlinks=True)
            try:
                mutate(d)
                missing = inv.check(d, os.path.join(d, ".github", "workflows"))
            finally:
                pass
            verdict = "caught" if missing else "MISSED"
            print(f"{verdict:7} {name}" + (f"  ({missing[0][:90]})" if missing else ""))
            if not missing:
                failures.append(name)
            shutil.rmtree(d)
    if failures:
        print(f"{len(failures)} mutation(s) the inventory check did not catch")
        return 1
    print(f"all {len(MUTATIONS)} mutations caught")
    return 0


if __name__ == "__main__":
    sys.exit(main())
