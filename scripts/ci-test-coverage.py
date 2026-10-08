#!/usr/bin/env python3
"""Verification policy rule 3, observed: every test CI knows of *executed* in this CI run.

The static check (check-test-inventory.py) infers from source and workflow text which step would run a
test; it is a fast pre-push lint and it approximates. This is the ground truth. It reads the logs of
the run's own jobs and the test universe the compiler and runners list, and fails on any test that never
executed — passed or failed, not skipped, not ignored, not filtered out, not in a step that did not run.

  * **Executed:** a Rust ``test <name> ... ok|FAILED`` line under a ``Running <target> (deps/<bin>-…)``
    or ``Doc-tests <crate>`` header (cargo's own lines); a pytest ``-v`` ``<nodeid> PASSED|FAILED|XPASS|XFAIL|ERROR`` line;
    a jest ``--verbose`` ``✓``/``✕`` line under its ``PASS|FAIL <file>`` header; a script-style suite's
    ``@@case@@ <suite>::<case>`` line, printed as the case starts (anywhere on the line: a Docker runner's
    output arrives prefixed with its container's name).
  * **Universe:** everything executed, plus every test any log names as skipped or ignored, plus the
    lists the ``test-universe`` job prints (``cargo test --all-features -- --list`` per crate, the
    no-default-features builds, ``pytest --collect-only`` over every test directory, and each script-style
    suite's ``--list``: one ``@@case-list@@ <suite>::<case>`` line per case, printed without running
    anything), so a test gated on a feature no step enables is in the universe and not executed.

A test key is ``rust <target>::<binary>::<name>`` (target is ``src/lib.rs``, ``tests/x.rs``,
``doc:<crate>``…), ``python <nodeid>``, ``typescript <file>::<title>`` or ``script <suite>::<case>`` (the suite is the
script's repository path, or the name of the step or make target that drives it). Two crates' same-named
integration files share a key, so a same-named test in both counts once — stated, and rare.

``scripts/test-coverage-exceptions.txt`` lists ``<key-glob> — <reason>`` for tests that legitimately
never run in CI (``#[ignore]``d benchmarks, hardware). An exception that matches nothing fails too, so
the list cannot rot.

Usage: ci-test-coverage.py <dir of job logs>      (each file one job's log, as the API returns it)
       ci-test-coverage.py --fetch <dir>          (GITHUB_REPOSITORY, GITHUB_RUN_ID, GH_TOKEN: fetch, then check)
       ci-test-coverage.py [--fetch] --scripts-only --require <src,src…> <dir>
                                                  (a workflow with no Rust/Python/TypeScript universe — the
                                                   Docker cluster suites: check only ``script`` cases, and
                                                   require each named job's listing, non-empty)
Each listing is bracketed ``@@test-universe@@ begin <source>`` … ``end``; without --require the sources required
are REQUIRED_SOURCES. CI_TEST_COVERAGE_EXCEPTIONS names another exceptions file (the self-test's).
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
JEST_LISTED = re.compile(r"(?:^|/)mycelium-ts/(\S+\.test\.ts)\s*$")
TS_LISTED = re.compile(r"^@@ts-test@@ (\S+\.test\.ts)::(.+)$")
JEST_TEST = re.compile(r"^\s+(✓|✕|○ skipped|○ todo|○)\s+(.+?)(?: \(\d+ m?s\))?$")
UNIVERSE_MARK = "@@test-universe@@"
# Script-style suites (scripts/test-*, ci_smoke.sh, the Docker runners): searched anywhere on the line, and the
# key's characters are restricted so a shell trace's quoting (`+ echo '@@case@@ a::b'`) is not part of it.
CASE_KEY = r"([A-Za-z0-9_./:-]+::[A-Za-z0-9_./:-]+)"
CASE_RAN = re.compile(r"@@case@@ " + CASE_KEY)
CASE_LISTED = re.compile(r"@@case-list@@ " + CASE_KEY)
# libtest names a should-panic test "<name> - should panic" and rustdoc a `no_run` / `compile_fail` doctest
# "<name> - compile" / "- compile fail" when running them; `--list` names them without. (A `no_run` doctest
# compiling *is* its run.)
RUST_SUFFIX = re.compile(r" - (?:should panic(?: with .*)?|compile(?: fail)?)$")


def canonical_py(nodeid: str, tracked: list[str]) -> str:
    """pytest names a test relative to its rootdir, which differs between invocations (each package's
    pyproject is a rootdir), so the same test can appear as `tests/x.py::t` and `pkg/tests/x.py::t`. Resolve
    the file to the one tracked file it can be — the path as a suffix, then with leading components dropped."""
    path, sep, rest = nodeid.partition("::")
    parts = path.split("/")
    for i in range(len(parts)):
        suffix = "/".join(parts[i:])
        hits = [f for f in tracked if f == suffix or f.endswith("/" + suffix)]
        if len(hits) == 1:
            return hits[0] + sep + rest
        if len(hits) > 1:
            break
    return nodeid


def clean(line: str) -> str:
    return ANSI.sub("", STAMP.sub("", line.rstrip("\r\n")))


TRACKED: list[str] = []


LISTED_BLOCKS: list[tuple[str, int]] = []   # (source, entries listed) per complete universe block (begin…end)
# Each listing names its source — `@@test-universe@@ begin <source>` — and the check requires each expected source
# present and non-empty, so a dropped listing cannot hide behind the others (a bare `begin` is read, as before,
# but satisfies no requirement). The CI workflow's three; the Docker workflow passes its own with --require.
REQUIRED_SOURCES = ["rust-python", "typescript", "scripts"]


def scan(text: str, executed: set, universe: set):
    target = None
    jest_file = None
    universe_mode = False
    in_step_source = False
    listed = 0
    source = ""
    for raw in text.split("\n"):
        line = clean(raw)
        # GitHub echoes a `run:` block's source between `##[group]Run …` and `##[endgroup]` before running it. A
        # marker written literally there (even under a branch never taken) is not a run, and a mark opens nothing.
        if line.startswith("##[group]Run "):
            in_step_source = True
            continue
        if in_step_source:
            if line.startswith("##[endgroup]"):
                in_step_source = False
            continue
        if UNIVERSE_MARK in line:
            mark = line.split(UNIVERSE_MARK, 1)[1].split()
            if mark[:1] == ["begin"] and len(mark) <= 2:
                universe_mode, listed = True, 0
                source = mark[1] if len(mark) == 2 else ""
            elif mark == ["end"] and universe_mode:
                universe_mode = False
                LISTED_BLOCKS.append((source, listed))
            continue
        m = CASE_LISTED.search(line)
        if m:
            if universe_mode:
                universe.add(f"script {m.group(1)}")
                listed += 1
            continue
        m = CASE_RAN.search(line)
        if m:
            key = f"script {m.group(1)}"
            universe.add(key)
            if not universe_mode:
                executed.add(key)
            continue
        if universe_mode:
            m = TS_LISTED.match(line.strip())
            if m:
                universe.add(f"typescript {m.group(1)}::{m.group(2).strip()}")
                listed += 1
                continue
            m = JEST_LISTED.search(line.strip())
            if m:
                universe.add(f"typescript-file {m.group(1)}")
                listed += 1
                continue
        if line.startswith("Test Suites:"):
            jest_file = None
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
                key = f"rust {target}::{RUST_SUFFIX.sub('', m.group(1))}"
                universe.add(key)
                if m.group(2) in ("ok", "FAILED") and not universe_mode:
                    executed.add(key)
                continue
            m = RUST_LISTED.match(line)
            if m and universe_mode:
                universe.add(f"rust {target}::{m.group(1)}")
                listed += 1
                continue
        m = PY_RESULT.match(line.strip())
        if m:
            key = f"python {canonical_py(m.group(1), TRACKED)}"
            universe.add(key)
            if m.group(2) != "SKIPPED" and not universe_mode:
                executed.add(key)
            continue
        if universe_mode:
            m = PY_COLLECTED.match(line.strip())
            if m:
                universe.add(f"python {canonical_py(m.group(1), TRACKED)}")
                listed += 1
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
                    # The file ran: jest lists files, not tests, so a file is known by its listing and
                    # covered when any of its tests executed somewhere.
                    executed.add(f"typescript-file {jest_file}")


def exceptions(root: str) -> list[tuple[str, str]]:
    path = os.environ.get("CI_TEST_COVERAGE_EXCEPTIONS") or os.path.join(root, "scripts", "test-coverage-exceptions.txt")
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
    unfinished = []
    for row in rows:
        jid, status, name = row.split("\t", 2)
        if name.startswith("Test coverage") or name.startswith("Fuzz"):
            continue
        if status != "completed":
            unfinished.append(name)
            continue
        # The jobs-log endpoint answers with a redirect to the log blob. `gh api` does not follow it (the
        # first CI run fetched empty bodies, and this script failed on that, as it should), and
        # `gh run view --log` refuses while the run is in progress — which it always is, from inside it.
        # curl follows the redirect and drops the token on the cross-host hop.
        log = subprocess.run(
            ["curl", "-sSfL", "--retry", "3", "-H", f"Authorization: Bearer {os.environ['GH_TOKEN']}",
             "-H", "Accept: application/vnd.github+json",
             f"https://api.github.com/repos/{repo}/actions/jobs/{jid}/logs"],
            capture_output=True, text=True, encoding="utf-8", errors="replace")
        if log.returncode != 0 or not log.stdout.strip():
            sys.exit(f"ci-test-coverage: could not fetch the log of job {name!r} ({jid}): {log.stderr.strip()[:300]}")
        open(os.path.join(dest, f"{jid}.log"), "w", encoding="utf-8").write(log.stdout)
    if unfinished:
        sys.exit("ci-test-coverage: these jobs had not finished, so their tests cannot be observed — add them to "
                 "the test-coverage job's `needs`: " + ", ".join(unfinished))


def main() -> int:
    args = sys.argv[1:]
    fetching = "--fetch" in args
    scripts_only = "--scripts-only" in args
    require: list[str] = []
    if "--require" in args:
        i = args.index("--require")
        require = [r for r in args[i + 1].split(",") if r]
        del args[i:i + 2]
    args = [a for a in args if a not in ("--fetch", "--scripts-only")]
    if scripts_only and not require:
        sys.exit("ci-test-coverage: --scripts-only needs --require <source,…>: the listings each job must print")
    require = require or REQUIRED_SOURCES
    logs = args[0]
    if fetching:
        fetch(logs)
    root = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
    try:
        TRACKED[:] = [f for f in subprocess.run(["git", "-C", root, "ls-files", "*.py"], capture_output=True,
                                                 text=True, check=True).stdout.split()]
    except (OSError, subprocess.CalledProcessError):
        pass
    executed, universe = set(), set()
    for f in sorted(glob.glob(os.path.join(logs, "*"))):
        scan(open(f, encoding="utf-8", errors="replace").read(), executed, universe)
    if scripts_only:
        # The Docker workflow: each job lists its own suite's cases before it starts Docker; only script cases.
        executed = {k for k in executed if k.startswith("script ")}
        universe = {k for k in universe if k.startswith("script ")}
    # The universe must have been listed, every part of it: a listing job that failed or was cancelled, or a
    # listing step dropped, would otherwise leave only what ran, and a test that never ran anywhere would be
    # unknown rather than missing.
    sizes: dict[str, int] = {}
    for src, n in LISTED_BLOCKS:
        sizes[src] = sizes.get(src, 0) + n
    unlisted = [r for r in require if not sizes.get(r)]
    if unlisted:
        print(f"ci-test-coverage: the test universe was not listed: no non-empty listing from {', '.join(unlisted)} "
              f"(listed: {sizes or 'nothing'})")
        return 1
    # One exceptions file serves two workflows, so an exception is judged (used, or stale) only where it is in
    # scope: a `script` one where its suite is listed, a Rust/Python/TypeScript one only outside --scripts-only.
    listed_suites = {k[len("script "):].split("::", 1)[0] for k in universe if k.startswith("script ")}

    def in_scope(pat: str) -> bool:
        if pat.startswith("script "):
            suite = pat[len("script "):].split("::", 1)[0]
            return any(fnmatch.fnmatchcase(s, suite) for s in listed_suites)
        return not scripts_only

    exc = [(p, r) for p, r in exceptions(root) if in_scope(p)]
    used = set()
    missing = []
    for key in sorted(universe - executed):
        # Exact unless the pattern has a `*`: a pytest id's `[param]` is not a character class.
        hit = next((p for p, _ in exc if (fnmatch.fnmatchcase(key, p.replace("[", "[[]")) if "*" in p else key == p)), None)
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
