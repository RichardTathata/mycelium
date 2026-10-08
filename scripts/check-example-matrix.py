#!/usr/bin/env python3
"""The examples matrix's **CI** column, checked against where CI executes each example (verification policy
rule 3, the static half for demonstrations).

``examples/README.md``'s capability matrix marks an example in its CI column ``✓`` when it is *executed on every
change*, ``✓ᵖ`` when it is executed only by a **path-filtered** workflow (on the pull requests and pushes to main
that touch that workflow's paths, and on its schedule), and ``·`` when CI does not execute it. A workflow runs on
every change when it triggers on ``pull_request`` with no ``paths``/``paths-ignore`` filter and no ``branches``
filter that leaves out ``main``; any other trigger ``check-test-inventory.py``'s ``_triggers`` accepts is
path-filtered here (that check's own semantics are unchanged: it still counts such a workflow as running).

This check reads every row and finds, from the repository's own files, where CI executes it — only in steps that
run (``check-test-inventory.py``'s ``ci_commands``: no ``if:`` that can be false, not ``continue-on-error``, not
inside a loop or conditional, ``make <target>`` expanded one level):

  * **wrapped** — ``scripts/example-case.sh cargo [+toolchain] run … --example NAME`` as a step's command;
  * **a built example run as a process** — ``target/(debug|release)/examples/NAME`` as a command's program;
  * **a script a step runs** — a tracked ``.sh``/``.py``/Dockerfile in an *executing position* (a command's
    program, ``./path``, the script argument of ``bash``/``sh``/``zsh``/``python``/``python3``/``uv run``, or the
    ``-f`` of ``docker build``), and the same positions inside the scripts so reached. In a shell script: a
    command ``cargo [+tc] run … --example NAME`` (also inside ``$(…)``), or a built-example path as a command's
    program — directly, or through a variable assigned such a command or path and then used as a program
    (``RUN="cargo run … --example x"`` then ``$RUN …``). Heredoc bodies and comments are not read. In a
    Dockerfile: a built-example path or ``--example NAME`` in a ``RUN``/``CMD``/``ENTRYPOINT``/``COPY``
    instruction (the image is built by the ``docker build -f`` that reached it). In Python (triple-quoted strings
    and comments dropped): a ``["cargo", "run", …, "--example", "NAME"]`` argument list on one line, or a
    ``VAR = … "examples" / "NAME"`` path passed first to a ``subprocess`` call;
  * **a listed suite case** — every listing a workflow prints between ``@@test-universe@@ begin <source>`` and
    ``end`` (all but ``rust-python`` and ``typescript``) is run, and a case ``<suite>::NAME`` executes example
    NAME when a running step reached the suite (its script, or its ``Makefile:<target>``) and the suite runs
    ``cargo run|build`` with ``--example``/``--bin`` taking a variable (the case *is* the target: the coop and
    guardrails smokes); a suite ``examples/<dir>…`` that a running step reached executes the row ``<dir>``.

It fails naming each ✓ row with no every-change site, each ✓ᵖ row whose sites are not all path-filtered (or that
has none), each example CI executes whose row says ``·``, and each one with no row — except a harness binary the
README names in its *Harness binaries* paragraph that has no matrix row. It also fails on any workflow line with
an ``--example`` that is neither a wrapped ``cargo run`` nor a ``cargo build``, since the observed job cannot see
it. The observed half is the ``test-coverage`` job: the wrapper prints ``@@case@@ examples::NAME`` as it runs,
and ``--list`` (below) puts each wrapped call in the universe. A wrapped example that fails to *compile* has still
executed in that sense — the step is red, so the run is red anyway. An example run from inside a script or as a
process (``compare_stem_observations``, ``reason_node``, ``reheal_node``, ``wiki_chat``, ``three_node_demo``,
``confined_fleet_node``) is covered here only, not as an observed case of its own; its suite's cases are observed.

**An approximation**, like the test inventory. Read as run although they may not run: a command in a shell
``if false``/function body that is never called, or in a loop (in a reached script — the workflow steps
themselves are read with control flow); an assignment whose variable is used as a program on a line that never
runs; a Python ``print``/f-string continuation line that happens to hold a cargo argument list. Missed (a false
alarm on a ✓ row, never a silent pass): an example run under a computed name, a script reached only through a
wrapper other than those listed (``with-pyyaml.sh``), a compose file's ``dockerfile:``, a Makefile target whose
recipe is only a loop.

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
NAME = re.compile(r"^[A-Za-z0-9_][A-Za-z0-9_-]*$")
SUITE_CASE = re.compile(r"@@case-list@@ ([A-Za-z0-9_./:-]+)::([A-Za-z0-9_./:-]+)")
UNIVERSE_BEGIN = re.compile(r"@@test-universe@@ begin ([A-Za-z0-9_-]+)")
NOT_SCRIPT_SOURCES = {"rust-python", "typescript"}
BUILT_EXAMPLE = re.compile(r"(?:^|/)target/(?:debug|release)/examples/([A-Za-z0-9_][A-Za-z0-9_-]*)$")
BUILT_ANYWHERE = re.compile(r"target/(?:debug|release)/examples/([A-Za-z0-9_][A-Za-z0-9_-]*)")
EXAMPLE_ANYWHERE = re.compile(r"--example(?:=|\s+)[\"']?([A-Za-z0-9_][A-Za-z0-9_-]*)")
PY_CARGO_RUN = re.compile(r"[\"']cargo[\"']\s*,\s*(?:[\"']\+[^\"']*[\"']\s*,\s*)?[\"']run[\"'](.*)$")
PY_EXAMPLE = re.compile(r"[\"']--example[\"']\s*,\s*[\"']([A-Za-z0-9_][A-Za-z0-9_-]*)[\"']")
PY_PATH_ASSIGN = re.compile(r"^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=.*[\"']examples[\"']\s*/\s*[\"']([A-Za-z0-9_][A-Za-z0-9_-]*)[\"']")
HEREDOC = re.compile(r"<<-?\s*(['\"]?)([A-Za-z_][A-Za-z0-9_]*)\1")
OPERATORS = {";", "&&", "||", "|", "&", "(", ")", "{", "}", ";;", "|&"}
KEYWORDS = {"if", "then", "elif", "else", "do", "while", "until", "!", "time", "exec", "command", "fi", "done",
            "esac"}
INTERPRETERS = {"bash", "sh", "zsh", "python", "python3"}
EVERY, FILTERED = "every change", "path-filtered"


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


def cargo_sub(argv: list[str]) -> str | None:
    """`run` / `build` / … for `cargo [+toolchain] <sub> …`, else None."""
    if not argv or os.path.basename(argv[0]) != "cargo":
        return None
    rest = argv[1:]
    if rest[:1] and rest[0].startswith("+"):
        rest = rest[1:]
    return rest[0] if rest else None


def wrapped_name(argv: list[str]) -> tuple[str | None, str | None]:
    """For the wrapper's arguments: (NAME, None), or (None, why it is refused). Only `cargo [+tc] run`."""
    if cargo_sub(argv) != "run":
        return None, "the wrapper fronts only `cargo [+toolchain] run …`"
    name = example_arg(argv)
    if name is None or not NAME.match(name):
        return None, "a wrapper call without a literal --example NAME"
    return name, None


def logical_lines(text: str, heredocs: bool = True):
    """(line number, text): `\\`-continuations joined, and — for shell — heredoc bodies skipped."""
    lines = text.split("\n")
    i = 0
    while i < len(lines):
        start, line = i + 1, lines[i]
        while line.rstrip().endswith("\\") and i + 1 < len(lines):
            i += 1
            line = line.rstrip()[:-1] + " " + lines[i]
        i += 1
        yield start, line
        m = HEREDOC.search(line) if heredocs else None
        if m:
            strip = line[m.start():m.start() + 3] == "<<-"
            while i < len(lines) and (lines[i].strip() if strip else lines[i]) != m.group(2):
                i += 1
            i += 1


def tokens(line: str) -> list[str] | None:
    lex = shlex.shlex(line, posix=True, punctuation_chars=";&|()")
    lex.whitespace_split = True
    lex.commenters = "#"
    try:
        return list(lex)
    except ValueError:
        return None


def segments(toks: list[str]):
    """Each simple command of a token list: (assignments {var: value}, argv)."""
    cur: list[str] = []
    for t in toks + [";"]:
        if t in OPERATORS or (not cur and t in KEYWORDS):
            if cur:
                env = {}
                while cur and re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", cur[0]):
                    k, _, v = cur.pop(0).partition("=")
                    env[k] = v
                yield env, cur
            cur = []
            continue
        cur.append(t)


def list_wrapped(path: str) -> tuple[list[str], list[str]]:
    """Every wrapper call in a workflow's text, by name; and every line this listing cannot read."""
    cases, errors = [], []
    for n, line in logical_lines(open(path, encoding="utf-8").read(), heredocs=False):
        if line.strip().startswith("#") or ("--example" not in line and WRAPPER not in line):
            continue
        toks = tokens(line)
        if toks is None:
            errors.append(f"{path}:{n}: a line with an example the listing cannot read: {line.strip()}")
            continue
        for _, argv in segments(toks):
            w = next((i for i, t in enumerate(argv) if os.path.basename(t) == WRAPPER), None)
            if w is not None:
                if argv[w + 1:w + 2] == ["--list"]:
                    continue  # the listing itself
                name, why = wrapped_name(argv[w + 1:])
                if why:
                    errors.append(f"{path}:{n}: {why}: {line.strip()}")
                else:
                    cases.append(name)
            elif any(a == "--example" or a.startswith("--example=") for a in argv):
                c = next((i for i, t in enumerate(argv) if os.path.basename(t) == "cargo"), None)
                if c is None or cargo_sub(argv[c:]) != "build":
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


def trigger_class(inv, on) -> str | None:
    """EVERY for an unfiltered pull_request trigger; FILTERED for any other trigger the inventory counts."""
    if isinstance(on, str):
        on = {on: None}
    elif isinstance(on, list):
        on = {k: None for k in on}
    on = on or {}
    if "pull_request" in on:
        pr = on["pull_request"] or {}
        branches = pr.get("branches")
        if not pr.get("paths") and not pr.get("paths-ignore") and (branches is None or "main" in branches):
            return EVERY
    return FILTERED if inv._triggers(on) else None


CELLS = {"✓": EVERY, "✓ᵖ": FILTERED, "·": None}


def readme_rows(path: str) -> tuple[dict, set, list[str]]:
    """{name: (EVERY | FILTERED | None, line)} for the matrix's rows, the harness names, and parse errors."""
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
        if cell not in CELLS:
            errors.append(f"{path}:{n}: `{m.group(1)}`'s CI cell is {cell!r}, not ✓, ✓ᵖ or ·")
        rows[m.group(1)] = (CELLS.get(cell), n)
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
            if f.endswith((".sh", ".py")) or f.startswith("Dockerfile"):
                out.add(os.path.relpath(os.path.join(d, f), root))
    return out


def resolve(arg: str, cwd: str, files: set[str]) -> str | None:
    """A tracked file an argument names: `$VAR/` / `${VAR}/` prefixes dropped, then relative to cwd, then by
    the longest repository-path suffix."""
    a = re.sub(r"^(?:\$\{?[A-Za-z_][A-Za-z0-9_]*\}?/)+", "", arg)
    rel = os.path.normpath(os.path.join(cwd, a))
    if rel in files:
        return rel
    parts = os.path.normpath(a).split("/")
    for i in range(len(parts)):
        cand = "/".join(parts[i:])
        if cand in files:
            return cand
    return None


def executed_files(argv: list[str], cwd: str, files: set[str]) -> list[str]:
    """The tracked scripts a command runs: its program, an interpreter's script argument, `uv run`'s, and
    `docker build -f`'s Dockerfile."""
    if not argv:
        return []
    out = []
    prog = os.path.basename(argv[0])
    cand = []
    if prog in INTERPRETERS:
        rest = argv[1:]
        if "-c" not in rest:
            cand = [next((a for a in rest if not a.startswith("-")), "")]
    elif prog == "uv" and argv[1:2] == ["run"]:
        rest = [a for a in argv[2:] if not a.startswith("-")]
        if rest and os.path.basename(rest[0]) in INTERPRETERS:
            rest = rest[1:]
        cand = rest[:1]
    elif prog == "docker" and "build" in argv:
        cand = [argv[i + 1] for i, a in enumerate(argv[:-1]) if a in ("-f", "--file")]
    else:
        cand = [argv[0]]
    for c in cand:
        r = resolve(c, cwd, files) if c else None
        if r:
            out.append(r)
    return out


def shell_commands(text: str):
    """Every simple command in a shell script, `$(…)` bodies included: (assignments, argv)."""
    for _, line in logical_lines(text):
        if line.strip().startswith("#"):
            continue
        toks = tokens(line)
        if toks is None:
            continue
        for env, argv in segments(toks):
            yield env, argv
            for word in list(env.values()) + argv:
                for inner in re.findall(r"\$\(([^()]*)\)", word):
                    yield from shell_commands(inner)


def shell_sites(text: str) -> tuple[set[str], list[list[str]]]:
    """(examples a shell script runs, every command's argv)."""
    found, cmds, assigned = set(), [], {}
    for env, argv in shell_commands(text):
        cmds.append(argv)
        for k, v in env.items():
            inner = tokens(v) or []
            if cargo_sub(inner) == "run" and example_arg(inner) and NAME.match(example_arg(inner)):
                assigned[k] = example_arg(inner)
            m = BUILT_ANYWHERE.search(v)
            if m and BUILT_EXAMPLE.search(v):
                assigned[k] = m.group(1)
        if cargo_sub(argv) == "run":
            name = example_arg(argv)
            if name and NAME.match(name):
                found.add(name)
        if argv:
            m = BUILT_EXAMPLE.search(re.sub(r"^\$\{?[A-Za-z_][A-Za-z0-9_]*\}?", "", argv[0]))
            if m:
                found.add(m.group(1))
    for argv in cmds:
        if argv:
            m = re.match(r"^\$\{?([A-Za-z_][A-Za-z0-9_]*)\}?$", argv[0])
            if m and m.group(1) in assigned:
                found.add(assigned[m.group(1)])
    return found, cmds


def dockerfile_sites(text: str) -> set[str]:
    found = set()
    for _, line in logical_lines(text, heredocs=False):
        if re.match(r"^\s*(RUN|CMD|ENTRYPOINT|COPY)\b", line):
            found |= set(BUILT_ANYWHERE.findall(line)) | set(EXAMPLE_ANYWHERE.findall(line))
    return found


def python_sites(text: str) -> set[str]:
    text = re.sub(r'("""|\'\'\')[\s\S]*?\1', "", text)
    found, paths = set(), {}
    for line in text.split("\n"):
        line = line.split("#", 1)[0] if line.lstrip().startswith("#") else line
        m = PY_CARGO_RUN.search(line)
        if m:
            found |= set(PY_EXAMPLE.findall(m.group(1)))
        m = PY_PATH_ASSIGN.match(line)
        if m:
            paths[m.group(1)] = m.group(2)
    for var, name in paths.items():
        if re.search(rf"subprocess\.\w+\(\s*\[\s*(?:str\(\s*)?{re.escape(var)}\b", text):
            found.add(name)
    return found


def makefile_recipe(root: str, target: str) -> str:
    try:
        mk = open(os.path.join(root, "Makefile"), encoding="utf-8").read()
    except OSError:
        return ""
    m = re.search(rf"^{re.escape(target)}:[^\n]*\n((?:\t[^\n]*\n?)*)", mk, re.M)
    return m.group(1) if m else ""


def variable_target(text: str) -> bool:
    for _, argv in shell_commands(text):
        if cargo_sub(argv) in ("run", "build"):
            for i, a in enumerate(argv[:-1]):
                if a in ("--example", "--bin") and argv[i + 1].startswith("$"):
                    return True
            if any(re.match(r"^--(?:example|bin)=\$", a) for a in argv):
                return True
    return False


def check(root: str) -> list[str]:
    inv = load_inventory()
    import yaml  # the inventory already required it
    errors: list[str] = []
    rows, harness, errs = readme_rows(os.path.join(root, "examples", "README.md"))
    errors += errs
    files = tracked_files(root)
    sites: dict[str, list[tuple[str, str]]] = defaultdict(list)    # name -> [(class, where)]
    dir_sites: dict[str, list[tuple[str, str]]] = defaultdict(list)
    seen: set[tuple[str, str]] = set()                              # (file, class) reached
    targets: set[tuple[str, str]] = set()                           # (make target, class) reached
    wf_dir = os.path.join(root, ".github", "workflows")

    workflows = sorted(os.path.join(wf_dir, f) for f in os.listdir(wf_dir) if f.endswith((".yml", ".yaml")))
    klass = {}
    for wf in workflows:
        errors += list_wrapped(wf)[1]   # an unfronted run is invisible to the observed job
        doc = yaml.safe_load(open(wf, encoding="utf-8")) or {}
        klass[os.path.basename(wf)] = trigger_class(inv, doc.get("on", doc.get(True, {})))

    queue: list[tuple[str, str, str]] = []
    for cmd in inv.ci_commands(root, wf_dir):
        k = klass.get(cmd.where.split(":", 1)[0])
        if k is None or not cmd.argv:
            continue
        for t in re.findall(r"\(make ([^)\s]+)\)", cmd.where):
            targets.add((t, k))
        w = next((i for i, t in enumerate(cmd.argv) if os.path.basename(t) == WRAPPER), None)
        if w == 0:
            name, _ = wrapped_name(cmd.argv[1:])  # a refused call is reported by list_wrapped
            if name:
                sites[name].append((k, f"wrapped in {cmd.where}"))
            continue
        if "--list" in cmd.argv:
            continue  # a suite's listing runs none of its cases
        m = BUILT_EXAMPLE.search(cmd.argv[0])
        if m:
            sites[m.group(1)].append((k, f"run as a process in {cmd.where}"))
        for rel in executed_files(cmd.argv, cmd.cwd, files):
            queue.append((rel, k, cmd.where))
    while queue:
        rel, k, where = queue.pop()
        if (rel, k) in seen:
            continue
        seen.add((rel, k))
        try:
            text = open(os.path.join(root, rel), encoding="utf-8").read()
        except (OSError, UnicodeDecodeError):
            continue
        base = os.path.basename(rel)
        if base.startswith("Dockerfile"):
            found, nxt = dockerfile_sites(text), []
        elif rel.endswith(".py"):
            found, nxt = python_sites(text), []
        else:
            found, cmds = shell_sites(text)
            nxt = [r for argv in cmds for r in executed_files(argv, os.path.dirname(rel), files)]
        for name in found:
            sites[name].append((k, f"run by {rel} ({where})"))
        for r in nxt:
            if r != rel:
                queue.append((r, k, where))

    # The listings a workflow prints (the script-style suites' cases). A case counts only in the classes in which
    # a running step reached its suite.
    for wf in workflows:
        if klass[os.path.basename(wf)] is None:
            continue
        doc = yaml.safe_load(open(wf, encoding="utf-8")) or {}
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
                    m = re.match(r"examples/([^/:]+)(?:/|$)", suite)
                    if m:
                        prefix = suite.rstrip("/") + "/"
                        for k in sorted({k for f, k in seen if f == suite or f.startswith(prefix)}):
                            dir_sites[m.group(1)].append((k, f"suite {suite} ({where})"))
                    if suite.startswith("Makefile:"):
                        t = suite.split(":", 1)[1]
                        classes = {k for tt, k in targets if tt == t}
                        body = makefile_recipe(root, t) if classes else ""
                    else:
                        classes = {k for f, k in seen if f == suite}
                        body = open(os.path.join(root, suite), encoding="utf-8").read() if classes else ""
                    if body and variable_target(body):
                        for k in sorted(classes):
                            sites[case].append((k, f"case {suite}::{case} ({where})"))

    every = lambda name: [w for k, w in sites.get(name, []) + dir_sites.get(name, []) if k == EVERY]
    filtered = lambda name: [w for k, w in sites.get(name, []) + dir_sites.get(name, []) if k == FILTERED]
    if os.environ.get("CHECK_EXAMPLE_MATRIX_VERBOSE"):
        for name in sorted(set(sites) | set(dir_sites)):
            print(f"{name}: every change: {'; '.join(dict.fromkeys(every(name))) or '-'} | path-filtered: "
                  f"{'; '.join(dict.fromkeys(filtered(name))) or '-'}")
    for name, (cell, n) in sorted(rows.items(), key=lambda kv: kv[1][1]):
        e, f = every(name), filtered(name)
        if cell == EVERY and not e:
            why = (f"only a path-filtered workflow executes it ({f[0]}) — mark it ✓ᵖ" if f else
                   "no CI step executes it (no wrapped --example, no script or suite case that runs it)")
            errors.append(f"examples/README.md:{n}: `{name}` is ✓ in the CI column, but {why}")
        elif cell == FILTERED and e:
            errors.append(f"examples/README.md:{n}: `{name}` is ✓ᵖ in the CI column, but a workflow that runs on "
                          f"every change executes it ({e[0]}) — mark it ✓")
        elif cell == FILTERED and not f:
            errors.append(f"examples/README.md:{n}: `{name}` is ✓ᵖ in the CI column, but no path-filtered workflow "
                          f"executes it")
    for name in sorted(sites):
        where = sites[name][0][1]
        if name not in rows:
            if name not in harness:
                errors.append(f"CI executes example `{name}` ({where}) but examples/README.md's matrix has no row for "
                              f"it (or name it among the harness binaries)")
        elif rows[name][0] is None:
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
