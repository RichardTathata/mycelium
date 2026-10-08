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
    command ``cargo [+tc] run … --example NAME`` (also inside a ``$(…)`` the shell expands — unquoted or
    double-quoted, never single-quoted), or a built-example path as a command's program — directly, or through a
    variable or array assigned such a command or path and then used as a program (``RUN="cargo run … --example
    x"`` then ``$RUN …``; ``CMD=(cargo run … --example x)`` then ``"${CMD[@]}"``; an assignment alone is not a
    run; an array runs whole only as ``"${A[@]}"``/``${A[*]}`` — a plain ``$A`` runs its first element). A line a
    quote spans is joined to the line that closes it. Control flow is read, without evaluating anything but a single
    literal command: a branch whose literal condition cannot take it (``if false``, ``if ! true``, ``while false``,
    ``until true``) never counts, nor any branch of the chain after one whose condition is ``true`` (``else``
    included) — a compound condition (``a || b``) is not literal; nor the right side of a literal ``false &&`` /
    ``true ||``; nor anything after an unconditional top-level ``exit``. A function body (``f() { … }``,
    ``f() ( … )``) counts only when the function is called from a command that counts, or named first in a
    ``trap`` string. A ``case`` pattern is not a command; ``{``/``}`` group only at command position.
    Heredoc bodies and comments are not read. In a Dockerfile: a built-example path or ``--example NAME`` in a
    ``RUN``/``CMD``/``ENTRYPOINT``/``COPY`` instruction (the image is built by the ``docker build -f`` that
    reached it). In Python (parsed, so strings and comments are not code): an argument vector of a
    process-starting call — ``subprocess.run``/``call``/``check_call``/``check_output``/``Popen`` (also imported
    by name), ``os.exec*``/``os.spawn*``, ``asyncio.create_subprocess_exec`` — that is ``["cargo", "run", …,
    "--example", "NAME"]`` (a literal, across lines, or a variable assigned one), or whose program is a ``VAR``
    assigned ``… "examples" / "NAME"``; the same list anywhere else (a ``print``, an f-string) is not a run;
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

**An approximation**, like the test inventory. **Read as run although it may not run** (the unsafe direction — each
can pass a ✓ row nothing executes): a command in a conditional or loop whose condition is not a single literal
(nothing is evaluated: ``if [ -n "$X" ]``, ``while read``), in any ``case`` arm, or after a call to a function that
exits; a function body reached through a ``return``, through a function that shadows a command (``cargo() { :; }``
then ``cargo run …`` credits the example), or through a function defined inside another's body and called before
that one runs; a quoted ``"$RUN"`` (one program named by the whole string — it fails, but is read as the scalar's
words); in Python, control flow and scope — an ``if False:`` block or an uncalled ``def`` still counts, and a
variable's last assignment anywhere in the file is the one read. A Python file the check cannot parse is reported by
name, not read as running nothing. **Missed** (the safe direction): an example run under a computed name, a script
reached only through a wrapper other than those listed (``with-pyyaml.sh``), a ``$(…)`` in an unquoted heredoc body
or a backtick substitution, a function named in a ``trap`` string after its first word, a run behind a command
prefix (``timeout``, ``env X=1``, ``bash -c "…"``, ``nohup``, ``sudo``, ``xargs``, ``eval``, ``python -m``) or a
function called only through one, a Python vector built other than as a literal (``[*base, "--example", x]``), a
reusable workflow (``on: workflow_call``) and the jobs that call it, a compose file's ``dockerfile:``, a Makefile
target whose recipe is only a loop. A miss is a false alarm on a ✓ row — or, on a ``·`` row CI in fact executes, a
silent pass: the column then under-claims, the safe direction.
"Every change" is a ``pull_request`` trigger without ``paths``/``paths-ignore``, without a ``branches`` that omits
or a ``branches-ignore`` that names ``main``, without ``types`` that omit ``synchronize``, and a job or step whose
``if:`` does not keep it off pull requests; anything else the inventory counts is ✓ᵖ.

Usage: check-example-matrix.py [--root DIR]          the check (needs PyYAML: scripts/with-pyyaml.sh;
                                                      CHECK_EXAMPLE_MATRIX_VERBOSE=1 prints each site found)
       check-example-matrix.py --list <workflow>     one ``@@case-list@@ examples::NAME`` per wrapper call (no
                                                      PyYAML; what ``scripts/example-case.sh --list`` runs)
"""
from __future__ import annotations

import ast
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


def _open_quote(line: str) -> bool:
    """Whether a shell line ends inside a quote it opened (a `#` comment at a word start ends the scan)."""
    q, i = None, 0
    while i < len(line):
        c = line[i]
        if q == "'":
            q = None if c == "'" else q
        elif c == "\\":
            i += 1
        elif q is None and c == "#" and (i == 0 or line[i - 1].isspace()):
            break
        elif q is None and c == "'":
            q = "'"
        elif c == '"':
            q = None if q == '"' else '"'
        i += 1
    return q is not None


def logical_lines(text: str, heredocs: bool = True, quotes: bool = False):
    """(line number, text): `\\`-continuations joined, with `quotes` the lines a quote spans joined too, and — for
    shell — heredoc bodies skipped."""
    lines = text.split("\n")
    i = 0
    while i < len(lines):
        start, line = i + 1, lines[i]
        while i + 1 < len(lines) and (line.rstrip().endswith("\\") or (quotes and _open_quote(line))):
            i += 1
            line = (line.rstrip()[:-1] + " " if line.rstrip().endswith("\\") and not (quotes and _open_quote(line))
                    else line + "\n") + lines[i]
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
        branches, ignored, types = pr.get("branches"), pr.get("branches-ignore") or [], pr.get("types")
        if (not pr.get("paths") and not pr.get("paths-ignore") and (branches is None or "main" in branches)
                and "main" not in ignored and (types is None or "synchronize" in types)):
            return EVERY
    return FILTERED if inv._triggers(on) else None


CELLS = {"✓": EVERY, "✓ᵖ": FILTERED, "·": None}


NOT_ON_PR = "github.event_name != 'pull_request'"


def _cond(v) -> str:
    return str(v or "").strip().removeprefix("${{").removesuffix("}}").strip()


def pr_excluded(doc: dict, wf: str) -> set[str]:
    """`workflow:job` and `workflow:job:step` locations whose `if:` (a job's, or one it needs) keeps them off pull
    requests — the inventory counts those as running (they run on every push), but not on every change (#566 review 2)."""
    jobs = doc.get("jobs") or {}

    def off(name, seen=()):
        job = jobs.get(name) or {}
        if name in seen:
            return False
        if _cond(job.get("if")) == NOT_ON_PR:
            return True
        needs = job.get("needs") or []
        return any(off(n, seen + (name,)) for n in ([needs] if isinstance(needs, str) else needs))

    out = set()
    for jname, job in jobs.items():
        if off(jname):
            out.add(f"{wf}:{jname}")
        for i, step in enumerate((job or {}).get("steps") or []):
            if _cond(step.get("if")) == NOT_ON_PR:
                out.add(f"{wf}:{jname}:{step.get('name', i)}")
    return out


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


SUBST = "__check_example_matrix_subst_{}__"
SUBST_ANY = re.compile(r"__check_example_matrix_subst_(\d+)__")
ARRAY_OPEN = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*)\+?=$")
VAR_PROGRAM = re.compile(r"^\$\{?([A-Za-z_][A-Za-z0-9_]*)(\[[^\]]*\])?\}?$")


def _close_paren(line: str, j: int) -> int | None:
    """The index of the `)` that closes a `$(` whose body starts at j (quotes and nesting respected)."""
    depth, q, n = 1, None, len(line)
    while j < n:
        c = line[j]
        if q == "'":
            q = None if c == "'" else q
        elif c == "\\":
            j += 1
        elif c == '"':
            q = None if q == '"' else '"'
        elif q is None and c == "'":
            q = "'"
        elif q is None and c == "(":
            depth += 1
        elif q is None and c == ")":
            depth -= 1
            if depth == 0:
                return j
        j += 1
    return None


def substitutions(line: str) -> tuple[str, list[str]]:
    """The line with each `$(…)` the shell expands — unquoted or double-quoted, never single-quoted — replaced by
    a placeholder word, and the bodies, by placeholder index."""
    out, bodies, i, q, n = [], [], 0, None, len(line)
    while i < n:
        c = line[i]
        if q == "'":
            q = None if c == "'" else q
        elif c == "\\":
            out.append(line[i:i + 2])
            i += 2
            continue
        elif q is None and c == "#" and (i == 0 or line[i - 1].isspace()):
            break  # a comment
        elif q is None and c == "'":
            q = "'"
        elif c == '"':
            q = None if q == '"' else '"'
        elif line.startswith("$(", i) and not line.startswith("$((", i):
            end = _close_paren(line, i + 2)
            if end is not None:
                bodies.append(line[i + 2:end])
                out.append("$" + SUBST.format(len(bodies) - 1))
                i = end + 1
                continue
        out.append(c)
        i += 1
    return "".join(out), bodies


def _ops(toks: list[str]) -> list[str]:
    """shlex groups adjacent punctuation (`()`, `);`): split each such run into the shell's operators."""
    out = []
    for t in toks:
        if t in OPERATORS or not re.fullmatch(r"[;&|()]+", t):
            out.append(t)
            continue
        while t:
            op = next(o for o in ("&&", "||", ";;", "|&", ";", "&", "|", "(", ")") if t.startswith(o))
            out.append(op)
            t = t[len(op):]
    return out


def _literal(argv: list[str], neg: bool) -> bool | None:
    """A command whose status is fixed: `true`/`:` (True), `false` (False), `!`-negated; else None."""
    v = True if argv in (["true"], [":"]) else False if argv == ["false"] else None
    return None if v is None else v != neg


def _walk(text: str, dead0: bool, func0: str | None, sink: list, top: bool = True) -> None:
    """Append (assignments, argv, function or None) for every simple command of a shell text that can run.

    Dropped: a branch whose condition is a single literal command that cannot take it (`if false`, `if ! true`,
    `while false`, `until true`; every branch after one whose condition is a literal `true`, `else` included); the
    right side of a literal `false &&` / `true ||`; everything after an unconditional top-level `exit`. A compound
    condition (`a || b`) is not literal. A command in a function body (`f() { … }`, `f() ( … )`, `function f`) is
    tagged with the function; a `$(…)` body inherits its command's context; a `case` pattern is not a command (every
    arm counts); `{`/`}` group only at command position. An array assignment `X=(…)` is an assignment (its value a
    list), not a command."""
    stack: list[dict] = []   # {"kind": if|loop|brace|paren|case, "dead": bool, "func": name|None, …}
    cur: list[str] = []
    arrays: dict[str, list[str]] = {}
    arr: tuple[str, list[str]] | None = None
    cond: dict | None = None         # the condition being read: {"frame", "segs": [(argv, neg)], "compound"}
    pending_func: str | None = None
    header: int | None = None        # paren depth while skipping a `for`/`select` header
    short: str | None = None         # "&&" after a literal false `&&`, "||" after a literal true `||`
    neg = False                      # the current command is `!`-negated
    exited = False
    prev_op: str | None = None
    bodies: list[str] = []

    def dead() -> bool:
        return dead0 or exited or short is not None or any(e["dead"] for e in stack)

    def func() -> str | None:
        return next((e["func"] for e in reversed(stack) if e.get("func")), func0)

    def nearest(*kinds: str) -> int | None:
        return next((i for i in range(len(stack) - 1, -1, -1) if stack[i]["kind"] in kinds), None)

    def flush(op: str | None) -> None:
        nonlocal cur, arrays, short, neg, exited, prev_op
        if cur or arrays:
            env: dict = {}
            while cur and re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", cur[0]):
                k, _, v = cur.pop(0).partition("=")
                env[k] = v
            env.update(arrays)
            argv, cur, arrays = cur, [], {}
            was_dead = dead()
            if not was_dead:
                sink.append((env, argv, func()))
                for word in [w for v in env.values() for w in (v if isinstance(v, list) else [v])] + argv:
                    for idx in SUBST_ANY.findall(word):
                        if int(idx) < len(bodies):
                            _walk(bodies[int(idx)], False, func(), sink, top=False)
            if cond is not None and argv:
                cond["segs"].append((argv, neg))
            lit = _literal(argv, neg)
            if short is not None:
                if op in ("&&", "||") and op != short:
                    short = None     # `false && a || b`: b runs
            elif not was_dead and cond is None and lit is not None and (op, lit) in (("&&", False), ("||", True)):
                short = op
            if (top and func0 is None and not stack and not was_dead and cond is None and argv[:1] == ["exit"]
                    and prev_op in (None, ";") and op in (None, ";")):
                exited = True
        if cond is not None and op in ("&&", "||", "|", "|&"):
            cond["compound"] = True
        if op not in ("&&", "||", "|", "|&"):
            short = None
        neg = False
        prev_op = op

    def resolve() -> None:
        nonlocal cond
        f, segs = cond["frame"], cond["segs"]
        lit = _literal(*segs[0]) if len(segs) == 1 and not cond["compound"] else None
        if f["kind"] == "if":
            f["dead"] = f["taken"] or lit is False
            f["taken"] = f["taken"] or lit is True
        else:
            f["dead"] = lit is (False if f["mode"] == "while" else True)
        cond = None

    for _, line in logical_lines(text, quotes=True):
        if line.strip().startswith("#"):
            continue
        line, bodies = substitutions(line)
        toks = tokens(line)
        if toks is None:
            continue
        toks = _ops(toks)
        i = 0
        while i < len(toks):
            t = toks[i]
            i += 1
            if arr is not None:
                if t == ")":
                    arrays[arr[0]] = arr[1]
                    arr = None
                else:
                    arr[1].append(t)
                continue
            if header is not None:
                if t == "(":
                    header += 1
                elif t == ")":
                    header -= 1
                elif header <= 0 and t in (";", "&", "do"):
                    header = None
                continue
            c = stack[-1] if stack and stack[-1]["kind"] == "case" else None
            if c is not None and c["hdr"]:
                if t == "in":
                    c["hdr"], c["pat"] = False, True
                continue
            if c is not None and c["pat"]:
                if t == "esac":
                    stack.pop()
                elif t == ")":
                    c["pat"] = False
                continue     # a pattern word, `|`, a leading `(`
            if t == "{" and (not cur or (len(cur) == 2 and cur[0] == "function")):
                if cur:
                    pending_func, cur = cur[1], []
                flush(None)
                stack.append({"kind": "brace", "dead": False, "func": pending_func})
                pending_func = None
                continue
            if t == "}" and not cur:
                flush(None)
                j = nearest("brace")
                if j is not None:
                    del stack[j:]
                continue
            if t in OPERATORS and t not in ("{", "}"):
                if t == "(" and cur and ARRAY_OPEN.match(cur[-1]):
                    arr = (ARRAY_OPEN.match(cur.pop()).group(1), [])
                    continue
                if t == "(" and toks[i:i + 1] == [")"] and (len(cur) == 1 or (len(cur) == 2 and cur[0] == "function")):
                    pending_func, cur = cur[-1], []
                    i += 1
                    continue
                if t == "(":
                    flush(None)
                    stack.append({"kind": "paren", "dead": False, "func": pending_func})
                    pending_func = None
                    continue
                if t == ")":
                    flush(None)
                    j = nearest("paren")
                    if j is not None:
                        del stack[j:]
                    continue
                flush(t)
                if t == ";;" and c is not None:
                    c["pat"] = True
                continue
            if not cur and t in KEYWORDS | {"for", "select", "case"}:
                if t == "!":
                    neg = not neg
                elif t == "if":
                    stack.append({"kind": "if", "dead": False, "taken": False})
                    cond = {"frame": stack[-1], "segs": [], "compound": False}
                elif t in ("while", "until"):
                    stack.append({"kind": "loop", "dead": False, "mode": t})
                    cond = {"frame": stack[-1], "segs": [], "compound": False}
                elif t in ("for", "select"):
                    stack.append({"kind": "loop", "dead": False, "mode": t})
                    header = 0
                elif t == "case":
                    stack.append({"kind": "case", "dead": False, "hdr": True, "pat": False})
                elif t in ("then", "do"):
                    if cond is not None:
                        resolve()
                elif t in ("elif", "else"):
                    j = nearest("if")
                    if j is not None:
                        stack[j]["dead"] = stack[j]["taken"]
                        if t == "elif":
                            cond = {"frame": stack[j], "segs": [], "compound": False}
                elif t in ("fi", "done", "esac"):
                    j = nearest({"fi": "if", "done": "loop", "esac": "case"}[t])
                    if j is not None:
                        del stack[j:]
                continue
            cur.append(t)
        if arr is None:
            flush(None)
        if header is not None and header <= 0:
            header = None


def shell_commands(text: str):
    """Every simple command a shell script runs, `$(…)` bodies included: (assignments, argv). A function body's
    commands count only when the function is called from a command that counts (or named by `trap`)."""
    sink: list = []
    _walk(text, False, None, sink)
    funcs = {f for _, _, f in sink if f}
    called, frontier = set(), {None}
    while frontier:
        nxt = set()
        for env, argv, f in sink:
            if f not in frontier or not argv:
                continue
            names = [argv[0]]
            if argv[0] == "trap" and len(argv) > 1:
                names.append((tokens(argv[1]) or [""])[0])
            for name in names:
                if name in funcs and name not in called:
                    called.add(name)
                    nxt.add(name)
        frontier = nxt
    for env, argv, f in sink:
        if f is None or f in called:
            yield env, argv


def shell_sites(text: str) -> tuple[set[str], list[list[str]]]:
    """(examples a shell script runs, every command's argv). A variable used as a program runs what it was
    assigned: a scalar its words; an array all of it as `${A[@]}`/`${A[*]}`, only its first element as `$A`."""
    found, cmds, scalar, whole, first = set(), [], {}, {}, {}
    for env, argv in shell_commands(text):
        cmds.append(argv)
        for k, v in env.items():
            words = v if isinstance(v, list) else (tokens(v) or [])
            name = example_arg(words) if cargo_sub(words) == "run" else None
            head = (words[0] if words else "") if isinstance(v, list) else v
            built = BUILT_ANYWHERE.search(head)
            target = whole if isinstance(v, list) else scalar
            if name and NAME.match(name):
                target[k] = name
            if built and BUILT_EXAMPLE.search(head):
                target[k] = built.group(1)
                if isinstance(v, list):
                    first[k] = built.group(1)
        if cargo_sub(argv) == "run":
            name = example_arg(argv)
            if name and NAME.match(name):
                found.add(name)
        if argv:
            m = BUILT_EXAMPLE.search(re.sub(r"^\$\{?[A-Za-z_][A-Za-z0-9_]*\}?", "", argv[0]))
            if m:
                found.add(m.group(1))
    for argv in cmds:
        m = VAR_PROGRAM.match(argv[0]) if argv else None
        if m:
            var, sub = m.group(1), m.group(2)
            hit = (whole if sub in ("[@]", "[*]") else first).get(var) or scalar.get(var)
            if hit:
                found.add(hit)
    return found, cmds


def dockerfile_sites(text: str) -> set[str]:
    found = set()
    for _, line in logical_lines(text, heredocs=False):
        if re.match(r"^\s*(RUN|CMD|ENTRYPOINT|COPY)\b", line):
            found |= set(BUILT_ANYWHERE.findall(line)) | set(EXAMPLE_ANYWHERE.findall(line))
    return found


SUBPROCESS_CALLS = {"run", "call", "check_call", "check_output", "Popen"}
OS_EXEC = re.compile(r"^(?:exec|spawn)([lv])p?e?$")
PY_EXAMPLE_PATH = re.compile(r"[\"']examples[\"']\s*/\s*[\"']([A-Za-z0-9_][A-Za-z0-9_-]*)[\"']")


def _py_vectors(call: ast.Call, from_subprocess: set[str]) -> list[list[ast.expr] | ast.expr]:
    """The argument vectors a process-starting call runs: `subprocess.run/call/check_call/check_output/Popen`'s
    first argument (or `args=`), `os.exec*`/`os.spawn*`'s list or trailing arguments, and
    `asyncio.create_subprocess_exec`'s positional arguments. Anything else is not a run."""
    f = call.func
    owner = f.value.id if isinstance(f, ast.Attribute) and isinstance(f.value, ast.Name) else None
    name = f.attr if isinstance(f, ast.Attribute) else f.id if isinstance(f, ast.Name) else None
    if (owner == "subprocess" and name in SUBPROCESS_CALLS) or (owner is None and name in from_subprocess):
        return call.args[:1] + [k.value for k in call.keywords if k.arg == "args"]
    m = OS_EXEC.match(name or "") if owner == "os" else None
    if m:
        skip = 2 if name.startswith("spawn") else 1  # spawn*'s mode, then the file
        return call.args[skip:skip + 1] if m.group(1) == "v" else [call.args[skip:]]
    if owner == "asyncio" and name == "create_subprocess_exec":
        return [call.args]
    return []


def python_sites(text: str) -> tuple[set[str], str | None]:
    """Examples a Python script runs: a `cargo [+tc] run … --example NAME` vector, or a path `VAR = … "examples" /
    "NAME"` as the vector's program, in a process-starting call (``_py_vectors``) — the vector a literal list or
    tuple, or a variable assigned one. A list anywhere else (a `print`, a docstring) is not a run. Second: why the
    script could not be parsed, if it could not (the caller reports it — its runs are unread)."""
    try:
        tree = ast.parse(text)
    except (SyntaxError, ValueError) as e:
        return set(), f"{type(e).__name__}: {e}"
    lists, paths, from_subprocess = {}, {}, set()
    for node in ast.walk(tree):
        if isinstance(node, ast.ImportFrom) and node.module == "subprocess":
            from_subprocess |= {a.asname or a.name for a in node.names if a.name in SUBPROCESS_CALLS}
        elif isinstance(node, (ast.Assign, ast.AnnAssign)) and node.value is not None:
            targets = node.targets if isinstance(node, ast.Assign) else [node.target]
            for t in targets:
                if isinstance(t, ast.Name):
                    if isinstance(node.value, (ast.List, ast.Tuple)):
                        lists[t.id] = node.value.elts
                    m = PY_EXAMPLE_PATH.search(ast.unparse(node.value))
                    if m:
                        paths[t.id] = m.group(1)

    def word(e: ast.expr) -> str:
        if isinstance(e, ast.Constant) and isinstance(e.value, str):
            return e.value
        if isinstance(e, ast.Call) and isinstance(e.func, ast.Name) and e.func.id == "str" and len(e.args) == 1:
            e = e.args[0]
        return f"\0{e.id}" if isinstance(e, ast.Name) else ""

    found = set()
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        for vec in _py_vectors(node, from_subprocess):
            if isinstance(vec, ast.Name):
                vec = lists.get(vec.id)
            elif isinstance(vec, (ast.List, ast.Tuple)):
                vec = vec.elts
            if not isinstance(vec, list) or not vec:
                continue
            argv = [word(e) for e in vec]
            if cargo_sub(argv) == "run" and example_arg(argv) and NAME.match(example_arg(argv)):
                found.add(example_arg(argv))
            if argv[0].startswith("\0") and argv[0][1:] in paths:
                found.add(paths[argv[0][1:]])
    return found, None


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
    klass, off_pr = {}, set()
    for wf in workflows:
        errors += list_wrapped(wf)[1]   # an unfronted run is invisible to the observed job
        doc = yaml.safe_load(open(wf, encoding="utf-8")) or {}
        klass[os.path.basename(wf)] = trigger_class(inv, doc.get("on", doc.get(True, {})))
        off_pr |= pr_excluded(doc, os.path.basename(wf))

    queue: list[tuple[str, str, str]] = []
    for cmd in inv.ci_commands(root, wf_dir):
        k = klass.get(cmd.where.split(":", 1)[0])
        if k is None or not cmd.argv:
            continue
        parts = cmd.where.split(" (make", 1)[0].split(":")
        if k == EVERY and (":".join(parts[:2]) in off_pr or ":".join(parts[:3]) in off_pr):
            k = FILTERED  # it runs on pushes, not on pull requests
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
            (found, why), nxt = python_sites(text), []
            if why:
                errors.append(f"{rel}: a Python script CI runs ({where}) that this check cannot parse ({why}) — the "
                              f"example runs in it are unread")
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
    print("check-example-matrix: every ✓ / ✓ᵖ row has a CI execution site of its class, and every example CI executes has its row")
    return 0


if __name__ == "__main__":
    sys.exit(main())
