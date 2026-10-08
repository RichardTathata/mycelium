#!/usr/bin/env python3
"""The examples matrix's **CI** column, checked against where CI executes each example (verification policy
rule 3, the static half for demonstrations).

``examples/README.md``'s capability matrix marks an example ✓ in its CI column when it is *executed on every
change*. This check reads every row and finds, from the repository's own files, where CI executes it:

  * **wrapped** — a ``scripts/example-case.sh <cargo command> --example NAME`` call in a workflow step that runs
    on every change (the steps ``check-test-inventory.py`` counts: a push/pull-request trigger, no ``if:`` that
    can be false, not ``continue-on-error``, ``make <target>`` expanded one level);
  * **a built example run as a process** — ``target/(debug|release)/examples/NAME`` in such a step;
  * **a script such a step runs** — a tracked ``.sh``/``.py``/Dockerfile/compose file named by a step's
    command, and every such file those name in turn — whose code (comments and ``echo``/``printf`` lines
    dropped) runs an example with cargo (the flag on the cargo line, or in a Python argument list), names its
    built binary under ``target/``, or builds that path with pathlib;
  * **a listed suite case** — every listing a workflow prints between ``@@test-universe@@ begin <source>`` and
    ``end`` (all but ``rust-python`` and ``typescript``, which list Rust/Python/TypeScript tests) is run, and a
    case ``<suite>::NAME`` executes example NAME when the suite's script runs ``cargo run|build`` with
    ``--example``/``--bin`` taking a variable (the case *is* the target: the coop smoke, the guardrails
    smoke); a suite ``examples/<dir>…`` executes the row ``<dir>`` (``community``, ``fluid_pipeline``,
    ``langgraph``).

It fails naming each ✓ row with no execution site, each example CI executes whose row says ``·``, and each one
with no row — except the harness binaries the README names in its *Harness binaries* paragraph. It also fails on
a ``cargo run … --example`` in any workflow that the wrapper does not front, since the observed job cannot see
it. The observed half is the ``test-coverage`` job: the wrapper prints ``@@case@@ examples::NAME`` as it runs,
and ``--list`` (below) puts each wrapped call in the universe.

**An approximation**, like the test inventory: a script that runs an example by a computed name the patterns
above do not read is missed (the example then needs a row-level site of another kind, or this check reports
the ✓ row as unbacked — a false alarm, never a silent pass); a script that only *mentions* an execution-shaped
path in a message counts it as run.

Usage: check-example-matrix.py [--root DIR]          the check (needs PyYAML: scripts/with-pyyaml.sh;
                                                      CHECK_EXAMPLE_MATRIX_VERBOSE=1 prints each site found)
       check-example-matrix.py --list <workflow>     one ``@@case-list@@ examples::NAME`` per wrapper call (no
                                                      PyYAML; what ``scripts/example-case.sh --list`` runs)
"""
from __future__ import annotations

import importlib.util
import os
import re
import shlex
import subprocess
import sys
from collections import defaultdict

WRAPPER = "example-case.sh"
NAME = re.compile(r"^[A-Za-z0-9_-]+$")
EXAMPLE_FLAG = re.compile(r"--example(?:[= ]|$)")
CARGO_RUN = re.compile(r"\bcargo\s+(?:\+\S+\s+)?run\b")
CARGO_BUILD = re.compile(r"\bcargo\s+(?:\+\S+\s+)?build\b")
SUITE_CASE = re.compile(r"@@case-list@@ ([A-Za-z0-9_./:-]+)::([A-Za-z0-9_./:-]+)")
UNIVERSE_BEGIN = re.compile(r"@@test-universe@@ begin ([A-Za-z0-9_-]+)")
NOT_SCRIPT_SOURCES = {"rust-python", "typescript"}
# Execution-shaped references in a script's code.
RUN_EXAMPLE = re.compile(r"--example(?:=|\s+)[\"']?([A-Za-z0-9_-]+)")
PY_RUN_EXAMPLE = re.compile(r"[\"']--example[\"']\s*,\s*[\"']([A-Za-z0-9_-]+)[\"']")
BUILT_EXAMPLE = re.compile(r"target/(?:debug|release)/examples/([A-Za-z0-9_-]+)")
PATHLIB_EXAMPLE = re.compile(r"[\"']examples[\"']\s*/\s*[\"']([A-Za-z0-9_-]+)[\"']")
VARIABLE_TARGET = re.compile(r"\bcargo\s+(?:run|build)\b[^\n]*--(?:example|bin)[= ]+\"?\$")
QUIET = re.compile(r"^\s*@?(?:echo|printf|print\(|fail|die|log)\b")


def example_arg(argv: list[str]) -> str | None:
    """The NAME of `--example NAME` / `--example=NAME`, up to a `--` (the example's own arguments)."""
    for i, a in enumerate(argv):
        if a == "--":
            return None
        if a == "--example":
            return argv[i + 1] if i + 1 < len(argv) else None
        if a.startswith("--example="):
            return a.split("=", 1)[1]
    return None


def logical_lines(text: str):
    lines = text.split("\n")
    i = 0
    while i < len(lines):
        start, line = i + 1, lines[i]
        while line.rstrip().endswith("\\") and i + 1 < len(lines):
            i += 1
            line = line.rstrip()[:-1] + " " + lines[i]
        i += 1
        yield start, line


def list_wrapped(path: str) -> tuple[list[str], list[str]]:
    """Every wrapper call in a workflow's text, by name; and every call this listing cannot read."""
    cases, errors = [], []
    for n, line in logical_lines(open(path, encoding="utf-8").read()):
        if line.strip().startswith("#"):
            continue
        if WRAPPER in line:
            rest = line.split(WRAPPER, 1)[1]
            if rest.split()[:1] == ["--list"]:
                continue  # the listing itself
            try:
                argv = shlex.split(rest, comments=True)
            except ValueError:
                errors.append(f"{path}:{n}: a wrapper call the listing cannot read: {line.strip()}")
                continue
            name = example_arg(argv)
            if name is None or not NAME.match(name):
                errors.append(f"{path}:{n}: a wrapper call without a literal --example NAME: {line.strip()}")
            else:
                cases.append(name)
        elif EXAMPLE_FLAG.search(line.split(" #", 1)[0]) and not CARGO_BUILD.search(line):
            errors.append(f"{path}:{n}: an example run that scripts/example-case.sh does not front — the "
                          f"test-coverage job cannot see it: {line.strip()}")
    return cases, errors


# ── the matrix check ──────────────────────────────────────────────────────────────────────────────

def load_inventory():
    spec = importlib.util.spec_from_file_location(
        "check_test_inventory", os.path.join(os.path.dirname(os.path.abspath(__file__)), "check-test-inventory.py"))
    mod = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = mod  # its dataclasses resolve their module by name
    spec.loader.exec_module(mod)
    return mod


def readme_rows(path: str) -> tuple[dict, set, list[str]]:
    """{name: (ci ✓?, line)} for the matrix's rows, the harness names, and parse errors."""
    rows, errors, harness = {}, [], set()
    ci_col = None
    text = open(path, encoding="utf-8").read()
    for n, line in enumerate(text.split("\n"), 1):
        if line.startswith("| Example |"):
            cells = [c.strip() for c in line.strip().strip("|").split("|")]
            ci_col = cells.index("CI") if "CI" in cells else None
            if ci_col is None:
                errors.append(f"{path}:{n}: the matrix header has no CI column")
            continue
        if not line.startswith("|"):
            ci_col = None  # the matrix ended; a later table is not it
            continue
        if ci_col is None or not line.startswith("| ["):
            continue
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        m = re.search(r"`([A-Za-z0-9_-]+)`", cells[0])
        if not m or len(cells) <= ci_col:
            errors.append(f"{path}:{n}: a matrix row this check cannot read: {line[:80]}")
            continue
        if m.group(1) in rows:
            errors.append(f"{path}:{n}: `{m.group(1)}` has two rows")
        cell = cells[ci_col]
        if cell not in ("✓", "·"):
            errors.append(f"{path}:{n}: `{m.group(1)}`'s CI cell is {cell!r}, not ✓ or ·")
        rows[m.group(1)] = (cell == "✓", n)
    m = re.search(r"\*\*Harness binaries[^\n]*(?:\n(?!\n)[^\n]*)*", text)
    if m:
        harness = set(re.findall(r"`([A-Za-z0-9_-]+)`", m.group(0)))
    if not rows:
        errors.append(f"{path}: no matrix rows found")
    return rows, harness, errors


def tracked_files(root: str) -> set[str]:
    out = set()
    for d, dirs, files in os.walk(root):
        dirs[:] = [x for x in dirs if x not in (".git", "target", "node_modules", "dist", "__pycache__")
                   and not x.startswith(".venv")]
        for f in files:
            if f.endswith((".sh", ".py")) or f.startswith("Dockerfile") or re.match(r"docker-compose.*\.ya?ml$", f):
                out.add(os.path.relpath(os.path.join(d, f), root))
    return out


def code_lines(text: str):
    for _, line in logical_lines(text):
        s = line.strip()
        if not s or s.startswith("#") or QUIET.match(s):
            continue
        yield re.sub(r"\s#\s.*$", "", line)


def referenced(line: str, files: set[str]) -> set[str]:
    hits = set()
    for tok in re.findall(r"[A-Za-z0-9_./-]+", line):
        parts = tok.split("/")
        for i in range(len(parts)):
            cand = "/".join(parts[i:])
            if cand in files:
                hits.add(cand)
                break
    return hits


def makefile_recipe(root: str, target: str) -> str:
    try:
        mk = open(os.path.join(root, "Makefile"), encoding="utf-8").read()
    except OSError:
        return ""
    m = re.search(rf"^{re.escape(target)}:[^\n]*\n((?:\t[^\n]*\n?)*)", mk, re.M)
    return m.group(1) if m else ""


def check(root: str) -> list[str]:
    inv = load_inventory()
    import yaml  # the inventory already required it
    errors: list[str] = []
    rows, harness, errs = readme_rows(os.path.join(root, "examples", "README.md"))
    errors += errs
    files = tracked_files(root)
    sites: dict[str, list[str]] = defaultdict(list)
    dir_sites: dict[str, list[str]] = defaultdict(list)
    seen: set[str] = set()
    wf_dir = os.path.join(root, ".github", "workflows")

    # Every workflow: a `cargo run --example` the wrapper does not front is invisible to the observed job.
    workflows = sorted(os.path.join(wf_dir, f) for f in os.listdir(wf_dir) if f.endswith((".yml", ".yaml")))
    for wf in workflows:
        errors += list_wrapped(wf)[1]

    # The steps that run on every change.
    queue: list[tuple[str, str]] = []
    for cmd in inv.ci_commands(root, wf_dir):
        if os.path.basename(cmd.argv[0]) == WRAPPER:
            name = example_arg(cmd.argv[1:])
            if name:
                sites[name].append(f"wrapped in {cmd.where}")
            continue
        if "--list" in cmd.argv:
            continue  # a suite's listing runs none of its cases
        for a in cmd.argv:
            for name in BUILT_EXAMPLE.findall(a):
                sites[name].append(f"run as a process in {cmd.where}")
            rel = os.path.normpath(os.path.join(cmd.cwd, a))
            if rel in files:
                queue.append((rel, cmd.where))
    while queue:
        rel, where = queue.pop()
        if rel in seen:
            continue
        seen.add(rel)
        try:
            text = open(os.path.join(root, rel), encoding="utf-8").read()
        except (OSError, UnicodeDecodeError):
            continue
        for line in code_lines(text):
            found = set(BUILT_EXAMPLE.findall(line)) | set(PATHLIB_EXAMPLE.findall(line))
            if CARGO_RUN.search(line):
                found |= set(RUN_EXAMPLE.findall(line))
            if re.search(r"[\"']cargo[\"']", line):
                found |= set(PY_RUN_EXAMPLE.findall(line))
            for name in found:
                sites[name].append(f"run by {rel} ({where})")
            for ref in referenced(line, files):
                if ref != rel:
                    queue.append((ref, where))

    # The listings a workflow prints (the script-style suites' cases).
    for wf in workflows:
        doc = yaml.safe_load(open(wf, encoding="utf-8")) or {}
        if not inv._triggers(doc.get("on", doc.get(True, {}))):
            continue
        for jname, job in (doc.get("jobs") or {}).items():
            for step in job.get("steps") or []:
                run = str(step.get("run", ""))
                srcs = UNIVERSE_BEGIN.findall(run)
                if not srcs or any(s in NOT_SCRIPT_SOURCES for s in srcs):
                    continue
                where = f"{os.path.basename(wf)}:{jname}"
                p = subprocess.run(["bash", "-c", run], cwd=root, capture_output=True, text=True)
                if p.returncode != 0:
                    errors.append(f"{where}: the listing step failed (exit {p.returncode}): {p.stderr.strip()[-400:]}")
                    continue
                for suite, case in SUITE_CASE.findall(p.stdout):
                    # Only a suite some step runs: its script, or a file under its directory, was reached above.
                    m = re.match(r"examples/([^/:]+)(?:/|$)", suite)
                    if m and any(f == suite or f.startswith(suite.rstrip("/") + "/") for f in seen):
                        dir_sites[m.group(1)].append(f"suite {suite} ({where})")
                    if suite.startswith("Makefile:"):
                        body = makefile_recipe(root, suite.split(":", 1)[1])
                    elif suite in seen:
                        body = open(os.path.join(root, suite), encoding="utf-8").read()
                    else:
                        continue
                    if any(VARIABLE_TARGET.search(l) for l in code_lines(body)):
                        sites[case].append(f"case {suite}::{case} ({where})")

    if os.environ.get("CHECK_EXAMPLE_MATRIX_VERBOSE"):
        for name in sorted(set(sites) | set(dir_sites)):
            print(f"{name}: {'; '.join(dict.fromkeys(sites.get(name, []) + dir_sites.get(name, [])))}")
    for name, (ci, n) in sorted(rows.items(), key=lambda kv: kv[1][1]):
        if ci and name not in sites and name not in dir_sites:
            errors.append(f"examples/README.md:{n}: `{name}` is ✓ in the CI column, but no CI step executes it "
                          f"(no wrapped --example, no script or suite case that runs it)")
    for name in sorted(sites):
        if name in harness:
            continue
        where = sites[name][0]
        if name not in rows:
            errors.append(f"CI executes example `{name}` ({where}) but examples/README.md's matrix has no row for "
                          f"it (or name it among the harness binaries)")
        elif not rows[name][0]:
            errors.append(f"examples/README.md:{rows[name][1]}: `{name}` says · in the CI column, but CI executes "
                          f"it ({where})")
    return errors


def main() -> int:
    args = sys.argv[1:]
    if args[:1] == ["--list"]:
        if len(args) != 2:
            print("usage: check-example-matrix.py --list <workflow>", file=sys.stderr)
            return 2
        cases, errors = list_wrapped(args[1])
        for e in errors:
            print(f"check-example-matrix: {e}", file=sys.stderr)
        if not cases and not errors:
            errors.append("none")
            print(f"check-example-matrix: {args[1]}: no wrapper call found", file=sys.stderr)
        if errors:
            return 1
        for c in cases:
            print(f"@@case-list@@ examples::{c}")
        return 0
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    if args[:1] == ["--root"] and len(args) == 2:
        root = os.path.abspath(args[1])
    elif args:
        print(__doc__, file=sys.stderr)
        return 2
    errors = check(root)
    for e in errors:
        print(f"check-example-matrix: {e}", file=sys.stderr)
    if errors:
        print(f"check-example-matrix: {len(errors)} problem(s)", file=sys.stderr)
        return 1
    print("check-example-matrix: every ✓ row has a CI execution site, and every example CI executes has a ✓ row")
    return 0


if __name__ == "__main__":
    sys.exit(main())
