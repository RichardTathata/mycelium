#!/usr/bin/env python3
"""Verification policy rule 3, observed: every test CI knows of *executed* in this CI run.

The static check (check-test-inventory.py) infers from source and workflow text which step would run a
test; it is a fast pre-push lint and it approximates. This is the ground truth. It reads the logs of
the run's own jobs and the test universe the compiler and runners list, and fails on any test that never
executed — passed or failed, not skipped, not ignored, not filtered out, not in a step that did not run.

  * **Executed:** a Rust ``test <name> ... ok|FAILED`` line under a ``Running <target> (deps/<bin>-…)``
    or ``Doc-tests <crate>`` header (cargo's own lines); a pytest ``-v`` ``<nodeid> PASSED|FAILED|XPASS|XFAIL|ERROR`` line;
    a jest ``--verbose`` ``✓``/``✕`` line under its ``PASS|FAIL <file>`` header.
  * **Universe:** everything executed, plus every test any log names as skipped or ignored, plus the
    lists the ``test-universe`` job prints (``cargo test --all-features -- --list`` per crate, the
    no-default-features builds, ``pytest --collect-only`` over every test directory), so a test gated
    on a feature no step enables is in the universe and not executed.

A test key is ``rust <target>::<binary>::<name>`` (target is ``src/lib.rs``, ``tests/x.rs``,
``doc:<crate>``…), ``python <nodeid>`` or ``typescript <file>::<title>``. Two crates' same-named
integration files share a key, so a same-named test in both counts once — stated, and rare.

``scripts/test-coverage-exceptions.txt`` lists ``<key-glob> — <reason>`` for tests that legitimately
never run in CI (``#[ignore]``d benchmarks, hardware). An exception that matches nothing fails too, so
the list cannot rot.

Usage: ci-test-coverage.py <dir of job logs>      (each file one job's log, as the API returns it)
       ci-test-coverage.py --fetch <dir>          (GITHUB_REPOSITORY, GITHUB_RUN_ID, GH_TOKEN: fetch, then check)
"""
from __future__ import annotations

import fnmatch
import glob
import json
import os
import re
import subprocess
import sys

ANSI = re.compile(r"(?:\x1b|\^\[)\[[0-9;]*[A-Za-z]")  # the raw byte, or `gh run view`'s caret rendering
STAMP = re.compile(r"^﻿?\d{4}-\d\d-\d\dT[\d:.]+Z ?")
RUNNING = re.compile(r"^\s*Running (?:unittests )?(\S+) \((?:\S*/)?deps/([A-Za-z0-9_]+)-[0-9a-f]+(?:\.exe)?\)")
DOCTESTS = re.compile(r"^\s*Doc-tests (\S+)")
RUST_RESULT = re.compile(r"^test (.+?) \.\.\. (ok|FAILED|ignored(?:, .*)?)$")
RUST_LISTED = re.compile(r"^(.+): (test|bench)$")
PY_RESULT = re.compile(r"^(\S+\.py::\S.*?) (PASSED|FAILED|SKIPPED|XFAIL|XPASS|ERROR)\b")
PY_COLLECTED = re.compile(r"^(\S+\.py::\S+)\s*$")
JEST_FILE = re.compile(r"^(PASS|FAIL)\s+(\S+\.test\.ts)")
JEST_TEST = re.compile(r"^\s+(✓|✕|○ skipped|○ todo|○)\s+(.+?)(?: \(\d+ m?s\))?$")
UNIVERSE_MARK = "@@test-universe@@"


def clean(line: str) -> str:
    return ANSI.sub("", STAMP.sub("", line.rstrip("\r\n")))


def scan(text: str, executed: set, universe: set):
    target = None
    jest_file = None
    universe_mode = False
    for raw in text.split("\n"):
        line = clean(raw)
        if UNIVERSE_MARK in line:
            universe_mode = line.split(UNIVERSE_MARK, 1)[1].strip() == "begin"
            continue
        m = RUNNING.match(line)
        if m:
            target = f"{m.group(1)}::{m.group(2)}"
            continue
        m = DOCTESTS.match(line)
        if m:
            target = f"doc:{m.group(1)}"
            continue
        if target:
            m = RUST_RESULT.match(line)
            if m:
                key = f"rust {target}::{m.group(1)}"
                universe.add(key)
                if m.group(2) in ("ok", "FAILED") and not universe_mode:
                    executed.add(key)
                continue
            m = RUST_LISTED.match(line)
            if m and universe_mode:
                universe.add(f"rust {target}::{m.group(1)}")
                continue
        m = PY_RESULT.match(line.strip())
        if m:
            key = f"python {m.group(1)}"
            universe.add(key)
            if m.group(2) != "SKIPPED" and not universe_mode:
                executed.add(key)
            continue
        if universe_mode:
            m = PY_COLLECTED.match(line.strip())
            if m:
                universe.add(f"python {m.group(1)}")
                continue
        m = JEST_FILE.match(line.strip())
        if m:
            jest_file = m.group(2)
            continue
        if jest_file:
            m = JEST_TEST.match(line)
            if m:
                key = f"typescript {jest_file}::{m.group(2).strip()}"
                universe.add(key)
                if m.group(1) in ("✓", "✕") and not universe_mode:
                    executed.add(key)


def exceptions(root: str) -> list[tuple[str, str]]:
    path = os.path.join(root, "scripts", "test-coverage-exceptions.txt")
    out = []
    if os.path.exists(path):
        for line in open(path, encoding="utf-8"):
            line = line.strip()
            if line and not line.startswith("#"):
                pat, _, reason = line.partition(" — ")
                if not reason.strip():
                    sys.exit(f"{path}: an exception needs a reason after ' — ': {line}")
                out.append((pat.strip(), reason.strip()))
    return out


def fetch(dest: str):
    repo, run = os.environ["GITHUB_REPOSITORY"], os.environ["GITHUB_RUN_ID"]
    os.makedirs(dest, exist_ok=True)
    rows = subprocess.run(
        ["gh", "api", "--paginate", f"repos/{repo}/actions/runs/{run}/jobs?per_page=100",
         "--jq", '.jobs[] | "\\(.id)\\t\\(.status)\\t\\(.name)"'],
        capture_output=True, text=True, check=True).stdout.splitlines()
    # The coverage job itself, and the fuzz job (push only, no libtest output, ~25 min), are not waited for.
    skip = {"Test coverage (observed)"}
    unfinished = []
    for row in rows:
        jid, status, name = row.split("\t", 2)
        if name in skip or name.startswith("Fuzz"):
            continue
        if status != "completed":
            unfinished.append(name)
            continue
        # `gh run view --log` follows the API's redirect to the log blob (`gh api …/logs` does not, and
        # returned an empty body on the first CI run — which this script then failed on, as it should).
        log = subprocess.run(["gh", "run", "view", run, "-R", repo, "--job", jid, "--log"],
                             capture_output=True, text=True)
        if log.returncode != 0 or not log.stdout.strip():
            sys.exit(f"ci-test-coverage: could not fetch the log of job {name!r} ({jid}): {log.stderr.strip()[:300]}")
        # Each line is `job<TAB>step<TAB>timestamp text`; keep the last field.
        text = "\n".join(l.split("\t", 2)[-1] for l in log.stdout.splitlines())
        open(os.path.join(dest, f"{jid}.log"), "w", encoding="utf-8").write(text)
    if unfinished:
        sys.exit("ci-test-coverage: these jobs had not finished, so their tests cannot be observed — add them to "
                 "the test-coverage job's `needs`: " + ", ".join(unfinished))


def main() -> int:
    args = sys.argv[1:]
    if args[:1] == ["--fetch"]:
        fetch(args[1])
        args = args[1:]
    logs = args[0]
    root = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
    executed, universe = set(), set()
    for f in sorted(glob.glob(os.path.join(logs, "*"))):
        scan(open(f, encoding="utf-8", errors="replace").read(), executed, universe)
    exc = exceptions(root)
    used = set()
    missing = []
    for key in sorted(universe - executed):
        hit = next((p for p, _ in exc if fnmatch.fnmatchcase(key, p)), None)
        if hit:
            used.add(hit)
        else:
            missing.append(key)
    stale = [p for p, _ in exc if p not in used]
    print(f"ci-test-coverage: {len(universe)} tests known, {len(executed)} executed, "
          f"{len(universe - executed) - len(missing)} excepted")
    rc = 0
    if missing:
        print(f"{len(missing)} test(s) never executed in this run (verification policy rule 3):")
        for k in missing:
            print(f"  - {k}")
        print("Run each in a CI step that has what it needs, or list it in scripts/test-coverage-exceptions.txt "
              "as '<key-glob> — <reason>'.")
        rc = 1
    if stale:
        print("Exceptions that match no unexecuted test (remove them):")
        for p in stale:
            print(f"  - {p}")
        rc = 1
    if not executed:
        print("no executed test found in the logs: the parser or the log format changed")
        rc = 1
    return rc


if __name__ == "__main__":
    sys.exit(main())
