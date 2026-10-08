#!/usr/bin/env python3
"""Verification policy rule 3, the fast half: infer from source and workflow text that every test in the
repository has a CI step that would run it. **An approximation, and a pre-push lint.** The record is
scripts/ci-test-coverage.py, the CI job that observes which tests executed in the run itself; this check
exists so most gaps are caught before a push, in seconds, without a build.

It **inventories** test *requirements* and maps each to a CI step that satisfies it:

  * Rust, per crate: the library's unit tests, each integration test, each binary's unit tests, the
    doctests — and, inside each, every distinct ``cfg`` gate on test code (``#![cfg(...)]`` on a file, the
    ``#[cfg(...)]`` on the ``mod`` that declares it, ``#[cfg(all(test, feature = "x"))]`` on a test module,
    ``#[cfg(...)]`` on a test function, ``[[test]] required-features``). A gate becomes a requirement: the
    features a step must enable (resolved through ``[features]``), the features it must not, and any bare
    ``cfg`` its ``RUSTFLAGS`` must set (``--cfg loom``). A ``cfg`` this script cannot evaluate is
    **uncovered**, never "needs nothing". A step's features are the ones its **test build** enables:
    closed through ``[features]`` and through every path dependency, the selected packages'
    dev-dependencies included — cargo unifies those, so a root-crate test gated off ``gateway`` is
    uncovered even behind ``--no-default-features`` (the root's dev-dependencies turn ``gateway`` on).
  * Python and TypeScript: every tracked test file, against the pytest/jest invocations that collect it. A
    **live** file — one that skips without a node — is covered only by a step whose environment sets the
    file's ``*_LIVE_REQUIRED`` guard, so a step that forgot the node fails instead of skipping green.
  * Fuzz targets, run by name or by a loop over ``cargo fuzz list``.

A CI step counts only if it would run: its workflow triggers on ``push``/``pull_request``/``merge_group``;
neither it nor its job is ``if: false`` or ``continue-on-error: true``; and its command executes tests
(not ``--no-run``, ``-- --list``, ``-- --ignored``, an ``echo``, or a name filter). Steps are read from the
workflow YAML with their ``env`` (workflow, job, step, inline) and ``working-directory``, and ``cd`` is
followed within a script. ``make <target>`` is expanded one level.

What it does not claim: that a test *passes*, or that a step's environment brings up whatever node a live
test talks to — only that a step exists which would run it, with the features and guards it needs. Known
limits, each left to the observed job: macro-generated tests, live-skip idioms other than a skipif on a MYCELIUM_TEST_ variable or a `*_LIVE_REQUIRED` guard, pytest
and jest configuration files, and `#[ignore]`d tests (skipped here; the observed job holds each to a stated
exception).

Run: ``python3 scripts/check-test-inventory.py`` (needs Python ≥ 3.11 and PyYAML). The mutation suite is
``scripts/test-check-test-inventory.py``.
"""
from __future__ import annotations

import fnmatch
import glob
import json
import os
import re
import shlex
import subprocess
import sys
import tomllib
from dataclasses import dataclass, field

try:
    import yaml
except ImportError:  # pragma: no cover
    sys.exit("check-test-inventory: needs PyYAML (pip install pyyaml)")


# ── cfg expressions ───────────────────────────────────────────────────────────────────────────────

@dataclass(frozen=True)
class Req:
    """One way to satisfy a gate: features to enable, features to leave off, bare cfgs to set."""
    need: frozenset = frozenset()
    without: frozenset = frozenset()
    cfgs: frozenset = frozenset()
    impossible: str | None = None  # a cfg we cannot evaluate, or one false on the CI platform

    def both(self, o: "Req") -> "Req":
        return Req(self.need | o.need, self.without | o.without, self.cfgs | o.cfgs, self.impossible or o.impossible)


TRUE = (Req(),)
# Facts on the CI platform (ubuntu runners, test builds).
PLATFORM = {
    ("test", None): True, ("debug_assertions", None): True, ("unix", None): True, ("windows", None): False,
    ("target_os", "linux"): True, ("target_family", "unix"): True, ("target_family", "windows"): False,
}


def _tokens(s: str) -> list[str]:
    return re.findall(r'"[^"]*"|[A-Za-z_][A-Za-z0-9_]*|[(),=]', s)


def parse_cfg(expr: str) -> tuple[Req, ...]:
    """A cfg predicate as alternatives (a disjunction of conjunctions)."""
    toks = _tokens(expr)
    pos = 0

    def node():
        nonlocal pos
        name = toks[pos]; pos += 1
        if name in ("all", "any", "not"):
            assert toks[pos] == "("; pos += 1
            kids = []
            while toks[pos] != ")":
                kids.append(node())
                if toks[pos] == ",":
                    pos += 1
            pos += 1
            return (name, kids)
        if pos < len(toks) and toks[pos] == "=":
            pos += 1
            val = toks[pos].strip('"'); pos += 1
            return ("kv", name, val)
        return ("flag", name)

    def alts(n, negate=False):
        kind = n[0]
        if kind == "all":
            if negate:  # not(all(a,b)) = any(not a, not b)
                return tuple(r for k in n[1] for r in alts(k, True))
            out = TRUE
            for k in n[1]:
                out = tuple(a.both(b) for a in out for b in alts(k))
            return out
        if kind == "any":
            if negate:
                out = TRUE
                for k in n[1]:
                    out = tuple(a.both(b) for a in out for b in alts(k, True))
                return out
            return tuple(r for k in n[1] for r in alts(k))
        if kind == "not":
            return alts(n[1][0], not negate)
        if kind == "kv":
            key, val = n[1], n[2]
            if key == "feature":
                return (Req(without=frozenset({val})),) if negate else (Req(need=frozenset({val})),)
            fact = PLATFORM.get((key, val))
            if fact is None:
                if key.startswith("target_"):
                    fact = False  # another platform than the CI runner's
                else:
                    return (Req(impossible=f'{key} = "{val}"'),)
            return TRUE if fact != negate else (Req(impossible=f'{key} = "{val}" on the CI platform'),)
        name = n[1]
        fact = PLATFORM.get((name, None))
        if fact is not None:
            return TRUE if fact != negate else (Req(impossible=f"{'not ' if negate else ''}{name}"),)
        if negate:
            return TRUE  # a bare cfg nobody sets is off: `not(loom)` holds by default
        return (Req(cfgs=frozenset({name})),)

    try:
        tree = node()
        return alts(tree)
    except (AssertionError, IndexError):
        return (Req(impossible=f"unparsed cfg({expr})"),)


def conj(a: tuple[Req, ...], b: tuple[Req, ...]) -> tuple[Req, ...]:
    return tuple(x.both(y) for x in a for y in b)


# ── Rust source scanning ──────────────────────────────────────────────────────────────────────────

ATTR = re.compile(r"#(!?)\[\s*cfg\s*\(", re.S)


def attrs_in(text: str):
    """Yield (inner?, predicate, end_offset) for every cfg attribute, balancing parentheses (so a
    multi-line `#![cfg(all(\n…))]` or one followed by a comment parses)."""
    for m in ATTR.finditer(text):
        i, depth = m.end(), 1
        while i < len(text) and depth:
            depth += {"(": 1, ")": -1}.get(text[i], 0)
            i += 1
        yield m.group(1) == "!", text[m.end():i - 1], i


def strip_code(text: str) -> str:
    """Blank comments (nested block comments too) and the contents of string, byte-string, raw-string
    and char literals, keeping every offset and newline — so braces, attributes and `mod` items are
    found only in code, and a `"sys/*"` cannot open a comment."""
    out = list(text)
    n, i = len(text), 0

    def blank(a, b):
        for k in range(a, b):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        c = text[i]
        if text.startswith("//", i):
            j = text.find("\n", i)
            j = n if j < 0 else j
            blank(i, j); i = j
        elif text.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if text.startswith("/*", j):
                    depth += 1; j += 2
                elif text.startswith("*/", j):
                    depth -= 1; j += 2
                else:
                    j += 1
            blank(i, j); i = j
        elif (m := re.match(r'b?r(#*)"', text[i:i + 300])) and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")):
            close = '"' + m.group(1)
            j = text.find(close, i + m.end())
            j = n if j < 0 else j + len(close)
            blank(i + m.end(), j - len(close)); i = j
        elif c == '"':
            j = i + 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            blank(i + 1, min(j, n)); i = j + 1
        elif c == "'" and (m := re.match(r"'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]+\}|.)|[^\\'\n])'", text[i:i + 14])):
            blank(i + 1, i + m.end() - 1); i += m.end()
        else:
            i += 1
    return "".join(out)


strip_comments = strip_code


ANY_ATTR = re.compile(r"#!?\[")
TEST_ATTR = re.compile(r"^#\[\s*(?:\w+::)*test\s*[\](]")
FN_ITEM = re.compile(r'\s*(?:pub(?:\s*\([^)]*\))?\s+)?(?:(?:async|unsafe|const|extern(?:\s*"[^"]*")?)\s+)*fn\s+(\w+)')
MOD_ITEM = re.compile(r"\s*(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+(\w+)\s*([{;])")


def attributes(code: str) -> list[tuple[int, int, bool]]:
    """(start, end, inner?) for every attribute, brackets balanced."""
    out = []
    for m in ANY_ATTR.finditer(code):
        i, depth = m.end(), 1
        while i < len(code) and depth:
            depth += {"[": 1, "]": -1}.get(code[i], 0)
            i += 1
        out.append((m.start(), i, m.group(0) == "#!["))
    return out


def stacks(code: str, attrs) -> list[list[tuple[int, int, bool]]]:
    """Runs of outer attributes separated only by whitespace — the attributes of one item."""
    runs, cur = [], []
    for a in attrs:
        if a[2]:
            continue
        if cur and code[cur[-1][1]:a[0]].strip() == "":
            cur.append(a)
        else:
            if cur:
                runs.append(cur)
            cur = [a]
    if cur:
        runs.append(cur)
    return runs


def cfg_of(code: str, spans) -> tuple[Req, ...]:
    g = TRUE
    for s, e, _ in spans:
        for _, pred, _ in attrs_in(code[s:e]):
            g = conj(g, parse_cfg(pred))
    return g


def match_brace(code: str, i: int) -> int:
    depth = 0
    for j in range(i, len(code)):
        if code[j] == "{":
            depth += 1
        elif code[j] == "}":
            depth -= 1
            if depth == 0:
                return j
    return len(code)


@dataclass
class Scanned:
    file_gate: tuple
    tests: list            # (gate, full name relative to the file's module, ignored?)
    mod_decls: list        # (name, gate, inline module path prefix, #[path] value or None)


def scan_file(raw: str) -> Scanned:
    code = strip_code(raw)
    attrs = attributes(code)
    # inner attributes at the top of the file, before any item
    fgate, prev = TRUE, 0
    for st, e, inner in attrs:
        if not inner or code[prev:st].strip() != "":
            break
        fgate = conj(fgate, cfg_of(raw, [(st, e, inner)]))
        prev = e
    # inline modules: span, own gate (outer cfgs + inner cfgs at the top of the body), name
    mods = []
    tests, decls = [], []
    for st in stacks(code, attrs):
        tail = code[st[-1][1]:]
        mm = MOD_ITEM.match(tail)
        if mm:
            if mm.group(2) == "{":
                open_at = st[-1][1] + mm.end() - 1
                close_at = match_brace(code, open_at)
                inner = [a for a in attrs if a[2] and open_at < a[0] < close_at and code[open_at + 1:a[0]].strip() == ""]
                mods.append((open_at, close_at, mm.group(1), conj(cfg_of(raw, st), cfg_of(raw, inner))))
    # inline modules with no attribute stack
    for mm in re.finditer(r"(?<![\w:])(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+(\w+)\s*\{", code):
        open_at = mm.end() - 1
        if not any(o == open_at for o, *_ in mods):
            close_at = match_brace(code, open_at)
            inner = [a for a in attrs if a[2] and open_at < a[0] < close_at and code[open_at + 1:a[0]].strip() == ""]
            mods.append((open_at, close_at, mm.group(1), cfg_of(raw, inner)))

    def enclosing(p):
        return sorted((m for m in mods if m[0] < p < m[1]), key=lambda m: m[0])

    handled = set()   # offsets of the names of `mod x;` items that carry attributes
    for st in stacks(code, attrs):
        tail = code[st[-1][1]:]
        texts = [code[s:e] for s, e, _ in st]
        if any(TEST_ATTR.match(t) for t in texts):
            fm = FN_ITEM.match(tail)
            if not fm:
                continue
            g = cfg_of(raw, st)
            prefix = []
            for o, c, name, mg in enclosing(st[0][0]):
                g = conj(g, mg)
                prefix.append(name)
            ignored = any(re.match(r"^#\[\s*ignore\b", t) for t in texts)
            tests.append((g, "::".join(prefix + [fm.group(1)]), ignored))
        mm = MOD_ITEM.match(tail)
        if mm and mm.group(2) == ";":
            handled.add(st[-1][1] + mm.start(1))
            g = cfg_of(raw, st)
            prefix = []
            for o, c, name, mg in enclosing(st[0][0]):
                g = conj(g, mg)
                prefix.append(name)
            pm = re.search(r'#\[\s*path\s*=\s*"([^"]+)"\s*\]', raw[st[0][0]:st[-1][1]])
            decls.append((mm.group(1), g, prefix, pm.group(1) if pm else None))
    # `mod x;` with no attributes
    for mm in re.finditer(r"(?<![\w:])(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+(\w+)\s*;", code):
        if mm.start(1) in handled:
            continue
        prefix, g = [], TRUE
        for o, c, name, mg in enclosing(mm.start()):
            g = conj(g, mg)
            prefix.append(name)
        decls.append((mm.group(1), g, prefix, None))
    return Scanned(fgate, tests, decls)


def file_gate(text: str) -> tuple[Req, ...]:
    return scan_file(text).file_gate


@dataclass
class Walked:
    path: str
    gate: tuple
    modpath: str           # the module's real path inside its crate root ("" for the root)
    root: str              # the root file this module hangs from (lib.rs, a bin's main, a test file)


def walk_modules(root_file: str) -> list[Walked]:
    """Every module file reachable from one crate root, with its composed gate and real module path —
    through inline modules and `#[path]`, as rustc resolves them."""
    out: list[Walked] = []
    seen = set()

    def walk(path, gate, modpath, child_dir):
        if path in seen or not os.path.exists(path):
            return
        seen.add(path)
        raw = open(path, encoding="utf-8", errors="replace").read()
        sc = scan_file(raw)
        gate = conj(gate, sc.file_gate)
        out.append(Walked(path, gate, modpath, root_file))
        base = os.path.dirname(path)
        for name, g, prefix, pathattr in sc.mod_decls:
            sub = "::".join([p for p in [modpath] + prefix + [name] if p])
            if pathattr:
                # #[path] is relative to the directory of the current file (or of the inline module's)
                cands = [os.path.normpath(os.path.join(base if not prefix else os.path.join(child_dir, *prefix), pathattr))]
            else:
                d = os.path.join(child_dir, *prefix)
                cands = [os.path.join(d, name + ".rs"), os.path.join(d, name, "mod.rs")]
            for c in cands:
                if os.path.exists(c):
                    stem = os.path.basename(c)[:-3]
                    walk(c, conj(gate, g), sub, os.path.dirname(c) if stem == "mod" else os.path.join(os.path.dirname(c), stem))
                    break

    walk(root_file, TRUE, "", os.path.dirname(root_file))
    return out


# ── the inventory ─────────────────────────────────────────────────────────────────────────────────

@dataclass
class Target:
    kind: str           # lib | integration | bin | doc
    pkg: str
    name: str
    alts: tuple          # alternatives (Req, ...): any one satisfies it
    where: str = ""
    names: tuple = ()    # every test's full name under this gate: a name filter selects the gate only if it is in each


def crates(root: str) -> dict[str, str]:
    top = tomllib.load(open(os.path.join(root, "Cargo.toml"), "rb"))
    out = {top["package"]["name"]: "."}
    members = []
    for pat in top.get("workspace", {}).get("members", []):
        members += [os.path.relpath(p, root) for p in glob.glob(os.path.join(root, pat))] if any(c in pat for c in "*?[") else [pat]
    for m in members:
        p = os.path.join(root, m, "Cargo.toml")
        if os.path.exists(p):
            out[tomllib.load(open(p, "rb"))["package"]["name"]] = m
    return out


DOC_FENCE = re.compile(r"^\s*//[/!]\s*```(?:rust|no_run|should_panic|compile_fail|edition\d+|)\s*(?:,[^\n]*)?$", re.M)


def rust_targets(root: str, crs: dict[str, str]) -> list[Target]:
    out: list[Target] = []
    for pkg, d in crs.items():
        cdir = os.path.normpath(os.path.join(root, d))
        manifest = tomllib.load(open(os.path.join(cdir, "Cargo.toml"), "rb"))
        src = os.path.join(cdir, "src")
        lib = manifest.get("lib", {})
        roots: list[tuple[str, str, tuple, str | None]] = []   # (kind, root file, base gate, disabled-why)
        lib_path = os.path.join(cdir, lib.get("path", "src/lib.rs"))
        if os.path.exists(lib_path):
            roots.append(("lib", lib_path, TRUE, None if lib.get("test", True) else "[lib] test = false"))
        bins = {b.get("name"): b for b in manifest.get("bin", [])}
        bin_files = {}
        if manifest.get("package", {}).get("autobins", True):
            if os.path.exists(os.path.join(src, "main.rs")):
                bin_files[manifest["package"]["name"]] = os.path.join(src, "main.rs")
            for f in glob.glob(os.path.join(src, "bin", "*.rs")):
                bin_files[os.path.basename(f)[:-3]] = f
            for f in glob.glob(os.path.join(src, "bin", "*", "main.rs")):
                bin_files[os.path.basename(os.path.dirname(f))] = f
        for name, b in bins.items():
            if "path" in b:
                bin_files[name] = os.path.join(cdir, b["path"])
        for name, f in bin_files.items():
            b = bins.get(name, {})
            base = (Req(need=frozenset(b["required-features"])),) if "required-features" in b else TRUE
            roots.append(("bin", f, base, None if b.get("test", True) else "[[bin]] test = false"))
        reached = set()
        for kind, rf, base, disabled in roots:
            for w in walk_modules(rf):
                reached.add(os.path.normpath(w.path))
                raw = open(w.path, encoding="utf-8", errors="replace").read()
                sc = scan_file(raw)
                groups: dict = {}
                for g, name, ignored in sc.tests:
                    if ignored:
                        continue  # libtest skips it; scripts/ci-test-coverage.py holds ignored tests to an exception
                    full = "::".join(p for p in [w.modpath, name] if p)
                    groups.setdefault(conj(conj(base, w.gate), g), []).append(full)
                for g, names in groups.items():
                    if disabled:
                        g = (Req(impossible=disabled),)
                    out.append(Target(kind, pkg, os.path.relpath(w.path, root), g, names=tuple(names)))
                if kind == "lib" and DOC_FENCE.search(raw):
                    dg = w.gate if lib.get("doctest", True) else (Req(impossible="[lib] doctest = false"),)
                    out.append(Target("doc", pkg, os.path.relpath(w.path, root), dg))
        # integration tests
        declared = {t["name"]: t for t in manifest.get("test", [])}
        files = {}
        if manifest.get("package", {}).get("autotests", True):
            for f in glob.glob(os.path.join(cdir, "tests", "*.rs")):
                files[os.path.basename(f)[:-3]] = f
            for f in glob.glob(os.path.join(cdir, "tests", "*", "main.rs")):
                files[os.path.basename(os.path.dirname(f))] = f
        for name, t in declared.items():
            files[name] = os.path.join(cdir, t["path"]) if "path" in t else os.path.join(cdir, "tests", name + ".rs")
        for name, f in files.items():
            t = declared.get(name, {})
            if not os.path.exists(f):
                continue
            base = (Req(need=frozenset(t["required-features"])),) if "required-features" in t else TRUE
            for w in walk_modules(f):
                reached.add(os.path.normpath(w.path))
                sc = scan_file(open(w.path, encoding="utf-8", errors="replace").read())
                groups = {}
                for g, tname, ignored in sc.tests:
                    if not ignored:
                        groups.setdefault(conj(conj(base, w.gate), g), []).append("::".join(p for p in [w.modpath, tname] if p))
                for g, names in groups.items():
                    if t.get("test") is False:
                        g = (Req(impossible="[[test]] test = false"),)
                    out.append(Target("integration", pkg, name, g, os.path.relpath(w.path, root), tuple(names)))
        # a file with tests that no root reaches: an unusual layout this check cannot follow (include!,
        # a macro-generated module, autotests = false) — uncovered until it is reachable or excepted
        for f in glob.glob(os.path.join(cdir, "src", "**", "*.rs"), recursive=True) + \
                 glob.glob(os.path.join(cdir, "tests", "**", "*.rs"), recursive=True):
            if os.path.normpath(f) in reached or "/fixtures/" in f:
                continue
            sc = scan_file(open(f, encoding="utf-8", errors="replace").read())
            if any(not ig for _, _, ig in sc.tests):
                out.append(Target("unreached", pkg, os.path.relpath(f, root),
                                  (Req(impossible="not reached from any crate root by the module walk"),)))
    return out


# ── CI steps ──────────────────────────────────────────────────────────────────────────────────────

@dataclass
class Cmd:
    argv: list[str]
    env: dict[str, str]
    cwd: str            # relative to the repo root
    where: str


# A step or job counts only when its condition is absent or one of these: each holds on every pull request
# or push to main (the fuzz job's `!= 'pull_request'` holds on push). Anything else — a schedule, a
# dispatch, a label — is treated as not running (allow-list, not deny-list).
RUNS_ON_EVERY_CHANGE = {"always()", "success()", "!cancelled()", "github.event_name != 'pull_request'"}


def _runs(v) -> bool:
    if v is None:
        return True
    t = str(v).strip()
    if t.startswith("${{") and t.endswith("}}"):
        t = t[3:-2].strip()
    return t in RUNS_ON_EVERY_CHANGE


def _truthy(v) -> bool:
    return v is True or (isinstance(v, str) and v.strip().lower() in ("true", "${{ true }}"))


def _triggers(on) -> bool:
    """Does the workflow run on pull requests (any) or on pushes to main?"""
    if isinstance(on, str):
        on = {on: None}
    elif isinstance(on, list):
        on = {k: None for k in on}
    on = on or {}
    if "pull_request" in on or "merge_group" in on:
        return True
    push = on.get("push", False)
    if push is False:
        return False
    push = push or {}
    branches = push.get("branches")
    return (branches is None and "tags" not in push) or (branches is not None and "main" in branches)


def ci_commands(root: str, workflows: str) -> list[Cmd]:
    out: list[Cmd] = []
    makefile = open(os.path.join(root, "Makefile")).read() if os.path.exists(os.path.join(root, "Makefile")) else ""
    for wf in sorted(glob.glob(os.path.join(workflows, "*.yml")) + glob.glob(os.path.join(workflows, "*.yaml"))):
        doc = yaml.safe_load(open(wf)) or {}
        if not _triggers(doc.get("on", doc.get(True, {}))):
            continue
        wenv = {k: str(v) for k, v in (doc.get("env") or {}).items()}
        jobs = doc.get("jobs") or {}

        def job_runs(name, seen=()):
            job = jobs.get(name) or {}
            if name in seen or not _runs(job.get("if")) or _truthy(job.get("continue-on-error")):
                return False
            needs = job.get("needs") or []
            needs = [needs] if isinstance(needs, str) else needs
            # a job that waits on one that never runs is skipped, unless it runs `if: always()`
            return str(job.get("if", "")).strip().strip("${} ") == "always()" or all(job_runs(n, seen + (name,)) for n in needs)

        for jname, job in jobs.items():
            if not job_runs(jname):
                continue
            jenv = {**wenv, **{k: str(v) for k, v in (job.get("env") or {}).items()}}
            jdir = ((job.get("defaults") or {}).get("run") or {}).get("working-directory", ".")
            for i, step in enumerate(job.get("steps") or []):
                if "run" not in step or not _runs(step.get("if")) or _truthy(step.get("continue-on-error")):
                    continue
                env = {**jenv, **{k: str(v) for k, v in (step.get("env") or {}).items()}}
                cwd = step.get("working-directory", jdir)
                where = f"{os.path.basename(wf)}:{jname}:{step.get('name', i)}"
                out += split_script(str(step["run"]), env, cwd, where, makefile)
    return out


FAIL_FAST = re.compile(r"\|\|\s*(?:exit(?:\s+[1-9]\d*|\s+\$\?)?|false)\s*$")


def split_script(script: str, env: dict, cwd: str, where: str, makefile: str, depth: int = 0) -> list[Cmd]:
    """The commands a shell script runs unconditionally. A command inside `if/for/while/case` or a
    heredoc, after an `exit`, or whose failure is swallowed (`|| true`, `|| :`), does not count — only
    `|| exit N` and `|| false` keep it. A `${{ … }}` expression in a command makes it unknowable."""
    out: list[Cmd] = []
    script = script.replace("\\\n", " ")
    nest = 0
    heredoc = None
    for line in script.split("\n"):
        stripped = line.strip()
        if heredoc is not None:
            if stripped == heredoc:
                heredoc = None
            continue
        hm = re.search(r"<<-?\s*['\"]?(\w+)['\"]?", line)
        if hm:
            heredoc = hm.group(1)
        # the first word of each command segment, past `then`/`do`/`else`
        words = []
        for seg in re.split(r";|&&|\|\||\|", stripped):
            w = seg.strip().split()
            while w and w[0] in ("then", "do", "else", "elif"):
                w = w[1:]
            if w:
                words.append(w[0])
        opens = sum(w in ("if", "for", "while", "until", "case", "select") for w in words)
        opens += bool(re.match(r"^\w+\s*\(\)\s*\{", stripped))
        closes = sum(w in ("fi", "done", "esac", "}") for w in words)
        was = nest
        nest = max(0, nest + opens - closes)
        if was or opens or hm:
            continue
        if words[:1] == ["exit"]:
            break
        swallowed = "||" in line and not FAIL_FAST.search(line)
        for part in re.split(r"&&|;|\|\|", line):
            part = part.strip()
            if not part or part.startswith("#") or swallowed or "${{" in part:
                continue
            try:
                argv = shlex.split(part, comments=True)
            except ValueError:
                continue
            local = dict(env)
            while argv and re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", argv[0]):
                k, _, v = argv.pop(0).partition("=")
                local[k] = v
            if not argv or argv[0] in ("echo", "printf", "true", ":"):
                continue
            if argv[0] == "cd" and len(argv) > 1:
                cwd = os.path.normpath(os.path.join(cwd, argv[1]))
                continue
            if argv[0] == "make" and depth == 0 and len(argv) > 1 and not argv[1].startswith("-"):
                tm = re.search(rf"^{re.escape(argv[1])}:[^\n]*\n((?:\t[^\n]*\n?)*)", makefile, re.M)
                if tm:
                    recipe = "\n".join(l.strip() for l in tm.group(1).split("\n"))
                    out += split_script(recipe, local, ".", where + f" (make {argv[1]})", makefile, depth + 1)
                continue
            out.append(Cmd(argv, local, cwd, where))
    return out


# cargo flags that take a value (the value is not a positional test filter)
CARGO_VALUED = {"-p", "--package", "--features", "-F", "--exclude", "-j", "--jobs", "--profile", "--target",
                "--manifest-path", "--target-dir", "--color", "--message-format", "--config", "-Z", "--bin",
                "--test", "--bench", "--example", "--lockfile-path"}
LIBTEST_VALUED = {"--test-threads", "--skip", "--format", "-Z", "--logfile", "--report-time", "--shuffle-seed", "--color"}


@dataclass
class CargoRun:
    pkgs: list = field(default_factory=list)
    excluded: set = field(default_factory=set)
    workspace: bool = False
    features: set = field(default_factory=set)
    default: bool = True
    all_features: bool = False
    selectors: set = field(default_factory=set)
    tests: set = field(default_factory=set)
    filtered: bool = False
    filters: list = field(default_factory=list)
    cfgs: set = field(default_factory=set)
    nextest: bool = False
    manifest_dir: str | None = None
    cwd: str = "."
    where: str = ""


def cargo_run(cmd: Cmd) -> CargoRun | None:
    a = list(cmd.argv)
    if os.path.basename(a[0]) in ("ci-retest.sh",):
        a = ["cargo", "test"] + a[1:]
    if os.path.basename(a[0]) != "cargo":
        return None
    a = a[1:]
    if a and a[0].startswith("+"):
        a = a[1:]
    if a[:1] == ["test"]:
        a, nextest = a[1:], False
    elif a[:2] == ["nextest", "run"]:
        a, nextest = a[2:], True
    else:
        return None
    r = CargoRun(nextest=nextest, cwd=cmd.cwd, where=cmd.where)
    r.cfgs = set(re.findall(r"--cfg[= ]+([A-Za-z_][A-Za-z0-9_]*)", cmd.env.get("RUSTFLAGS", "")))
    i = 0
    while i < len(a):
        x = a[i]
        key, eq, val = x.partition("=")
        has = (lambda: val) if eq else (lambda: a[i + 1] if i + 1 < len(a) else "")
        step = 1 if eq else 2
        if x == "--":
            rest = a[i + 1:]
            j = 0
            while j < len(rest):
                y = rest[j]
                yk = y.partition("=")[0]
                if yk in ("--list", "--ignored", "--bench"):
                    return None  # lists, or runs only the ignored ones
                if yk in LIBTEST_VALUED:
                    if yk == "--skip":
                        r.filters.append(None)  # a skip can drop anything
                    j += 1 if "=" in y else 2
                    continue
                if yk == "--exact":
                    r.filters.append(None)  # exact names: this check does not model them
                elif not y.startswith("-"):
                    r.filtered = True
                    r.filters.append(y)
                j += 1
            break
        if key == "--no-run":
            return None
        if key in ("-p", "--package"):
            r.pkgs.append(has()); i += step; continue
        if key == "--exclude":
            r.excluded.add(has()); i += step; continue
        if key in ("--features", "-F"):
            r.features |= set(re.split(r"[ ,]+", has().strip("\"'"))) - {""}; i += step; continue
        if key == "--manifest-path":
            r.manifest_dir = os.path.dirname(os.path.normpath(os.path.join(cmd.cwd, has()))) or "."; i += step; continue
        if key == "--test":
            r.selectors.add("--test"); r.tests.add(has()); i += step; continue
        if key in ("--bin", "--bench", "--example"):
            r.selectors.add(key); i += step; continue
        if key in CARGO_VALUED:
            i += step; continue
        if x == "--no-default-features":
            r.default = False
        elif x == "--all-features":
            r.all_features = True
        elif x in ("--workspace", "--all"):
            r.workspace = True
        elif x in ("--lib", "--bins", "--tests", "--doc", "--examples", "--benches", "--all-targets"):
            r.selectors.add(x)
        elif not x.startswith("-"):
            r.filtered = True
            r.filters.append(x)
        i += 1
    return r


def feature_closure(cdir: str, enabled: set, default: bool) -> set:
    table = tomllib.load(open(os.path.join(cdir, "Cargo.toml"), "rb")).get("features", {})
    todo = set(enabled) | ({"default"} if default and "default" in table else set())
    seen: set = set()
    while todo:
        f = todo.pop()
        if f in seen:
            continue
        seen.add(f)
        for implied in table.get(f, []):
            if "/" not in implied and not implied.startswith("dep:"):
                todo.add(implied)
    return seen


def _manifest(cdir: str) -> dict:
    return tomllib.load(open(os.path.join(cdir, "Cargo.toml"), "rb"))


def _path_deps(root: str, crs: dict, pkg: str, dev: bool) -> list[tuple[str, str, set, bool, bool]]:
    """`(key, workspace package, features, default-features, optional)` for each path dependency of `pkg`."""
    cdir = os.path.join(root, crs[pkg])
    m = _manifest(cdir)
    by_dir = {os.path.normpath(os.path.join(root, d)): p for p, d in crs.items()}
    out = []
    for table in ("dependencies",) + (("dev-dependencies",) if dev else ()):
        for key, spec in m.get(table, {}).items():
            if not isinstance(spec, dict) or "path" not in spec:
                continue
            q = by_dir.get(os.path.normpath(os.path.join(cdir, spec["path"])))
            if q:
                out.append((key, q, set(spec.get("features", [])), spec.get("default-features", True),
                            bool(spec.get("optional")) and table == "dependencies"))
    return out


def unified_features(root: str, crs: dict, r: CargoRun, root_pkg: str) -> dict[str, set]:
    """The features cargo enables on each workspace crate in this run's **test build**: the flags on the
    selected packages, closed through `[features]`, then through every path dependency — a dependency's
    `features = [...]` and default, and a feature's `dep/feature` entries — including the selected packages'
    **dev-dependencies**, which a test build compiles and resolver 2 unifies into the crates they reach. So
    the root crate's dev-dependency on a companion whose `gateway` feature is `mycelium/gateway` turns
    `gateway` back on under `--no-default-features`; a test gated off it never runs there."""
    if r.workspace:
        selected = [p for p in crs if p not in r.excluded]
    elif r.pkgs:
        selected = [p for p in r.pkgs if p in crs]
    else:
        home = os.path.normpath(r.manifest_dir if r.manifest_dir is not None else r.cwd)
        selected = [next((p for p, d in crs.items() if os.path.normpath(d) == home), root_pkg)]
    want: dict[str, set] = {p: set() for p in crs}
    default: dict[str, bool] = {p: False for p in crs}
    for p in selected:
        default[p] = r.default
        for f in r.features:
            if "/" in f:
                q, _, g = f.partition("/")
                if q in crs:
                    want[q].add(g)
            else:
                want[p].add(f)
    deps = {p: _path_deps(root, crs, p, dev=p in selected) for p in crs}
    tables = {p: _manifest(os.path.join(root, d)).get("features", {}) for p, d in crs.items()}
    built = set(selected)
    changed = True
    while changed:
        changed = False
        for p in sorted(built):
            on = feature_closure(os.path.join(root, crs[p]), want[p], default[p])
            entries = [e for f in on for e in tables[p].get(f, [])]
            for key, q, feats, dflt, optional in deps[p]:
                if optional and not any(e in (f"dep:{key}", key) or e.split("/")[0].rstrip("?") == key for e in entries + list(on)):
                    continue
                add = set(feats) | {e.split("/", 1)[1] for e in entries if "/" in e and e.split("/")[0].rstrip("?") == key}
                if q not in built or not add <= want[q] or (dflt and not default[q]):
                    built.add(q)
                    want[q] |= add
                    default[q] = default[q] or dflt
                    changed = True
    return {p: feature_closure(os.path.join(root, crs[p]), want[p], default[p]) for p in built}


def runs_package(r: CargoRun, pkg: str, crs: dict, root_pkg: str) -> bool:
    if pkg in r.excluded:
        return False
    if r.workspace or pkg in r.pkgs:
        return True
    if r.pkgs:
        return False
    home = r.manifest_dir if r.manifest_dir is not None else r.cwd
    home = os.path.normpath(home)
    owner = next((p for p, d in crs.items() if os.path.normpath(d) == home), None)
    return pkg == (owner or root_pkg)


def selects(r: CargoRun, t: Target) -> bool:
    """No filter, or filters that select every test under the gate: cargo runs a test whose full path
    contains one of its filters, and the scan knows each test's full path (through `#[path]`, inline
    modules and binary roots), so this is exact up to the scan."""
    if None in r.filters:
        return False
    return not r.filters or (bool(t.names) and all(any(f in n for f in r.filters) for n in t.names))


def runs_kind(r: CargoRun, t: Target) -> bool:
    if not selects(r, t):
        return False
    s = r.selectors
    if t.kind == "doc":
        return not r.nextest and (not s or "--doc" in s)
    if not s or "--all-targets" in s:
        return True
    if t.kind == "lib":
        return bool({"--lib", "--tests"} & s)
    if t.kind == "bin":
        return bool({"--bins", "--tests"} & s)
    return "--tests" in s or ("--test" in s and ("*" in r.tests or t.name in r.tests))


def satisfies(r: CargoRun, on: set | None, req: Req) -> bool:
    """`on`: the features the run's test build enables on the crate ([`unified_features`]); `None` under
    `--all-features`."""
    if req.impossible:
        return False
    if on is not None and not req.need <= on:
        return False
    if req.without and (r.all_features or req.without & on):
        return False
    return req.cfgs <= r.cfgs


# ── Python, TypeScript, fuzz ──────────────────────────────────────────────────────────────────────

def tracked(root: str, *patterns: str) -> list[str]:
    try:
        files = subprocess.run(["git", "-C", root, "ls-files"], capture_output=True, text=True, check=True).stdout.split()
    except (OSError, subprocess.CalledProcessError):
        files = [os.path.relpath(p, root) for p in glob.glob(os.path.join(root, "**", "*"), recursive=True)]
    return [f for f in files if any(fnmatch.fnmatch(os.path.basename(f), p) for p in patterns)]


def live_guard(root: str, path: str) -> str | None:
    """The `*_LIVE_REQUIRED` variable that guards a live test file (in it or a conftest above it), or ""
    for a live file with no guard, or None for a node-free file."""
    text = open(os.path.join(root, path)).read()
    guards = re.findall(r"\b([A-Z_]*LIVE_REQUIRED)\b", text)
    live = bool(guards) or bool(re.search(r"skipif\([^)]*MYCELIUM_TEST_|describe\.skip|pytest\.skip\(", text)) \
        or "/live/" in "/" + path
    d = os.path.dirname(path)
    while not guards and d and d != ".":
        conf = os.path.join(root, d, "conftest.py")
        if os.path.exists(conf):
            guards = re.findall(r"\b([A-Z_]*LIVE_REQUIRED)\b", open(conf).read())
            live = live or bool(guards)
        d = os.path.dirname(d)
    if not live:
        return None
    return guards[0] if guards else ""


def pytest_collects(cmd: Cmd, path: str) -> bool:
    a = cmd.argv
    if a[0] in ("python", "python3") and a[1:3] == ["-m", "pytest"]:
        a = ["pytest"] + a[3:]
    if os.path.basename(a[0]) != "pytest":
        return False
    targets, ignored, i = [], [], 1
    while i < len(a):
        x = a[i]
        k, eq, v = x.partition("=")
        if k in ("-k", "-m", "--co", "--collect-only", "--lf", "--last-failed", "--sw", "--stepwise", "-x", "--exitfirst"):
            return False  # a selection, a listing, or a run that may stop early: not the whole file
        if k in ("--ignore", "--ignore-glob", "--deselect"):
            ignored.append(v if eq else (a[i + 1] if i + 1 < len(a) else "")); i += 1 if eq else 2; continue
        if k in ("-p", "-c", "--rootdir", "-o", "--junitxml", "--basetemp", "--maxfail", "--tb", "-W"):
            i += 1 if eq else 2; continue
        if not x.startswith("-"):
            targets.append(os.path.normpath(os.path.join(cmd.cwd, x.split("::")[0])))
            if "::" in x:
                return False
        i += 1
    targets = targets or [os.path.normpath(cmd.cwd)]
    ignored = [os.path.normpath(os.path.join(cmd.cwd, g)) for g in ignored]
    hit = any(path == t or path.startswith(t.rstrip("/") + "/") or t == "." for t in targets)
    miss = any(path == g or path.startswith(g.rstrip("/") + "/") for g in ignored)
    return hit and not miss


def jest_collects(root: str, cmd: Cmd, path: str) -> bool:
    a = cmd.argv
    if a[:2] in (["npm", "test"], ["npm", "run"]) and (a[:2] == ["npm", "test"] or a[2:3] == ["test"]):
        pj0 = os.path.join(root, os.path.normpath(cmd.cwd), "package.json")
        script = json.load(open(pj0)).get("scripts", {}).get("test", "") if os.path.exists(pj0) else ""
        rest = a[2:] if a[1] == "test" else a[3:]
        a = shlex.split(script) + [x for x in rest if x != "--"]
        if a[:1] == ["npx"]:
            a = a[1:]
    elif a[:2] == ["npx", "jest"]:
        a = ["jest"] + a[2:]
    if a[:1] != ["jest"]:
        return False
    pkgdir = os.path.normpath(cmd.cwd)
    pj = os.path.join(root, pkgdir, "package.json")
    if not os.path.exists(pj) or not path.startswith(pkgdir + "/"):
        return False
    conf = json.load(open(pj)).get("jest", {})
    rel = path[len(pkgdir) + 1:]
    matches = [fnmatch.fnmatch(rel, m.replace("**/", "*")) or fnmatch.fnmatch(rel, m) for m in conf.get("testMatch", ["**/*.test.ts"])]
    if not any(matches) or any(re.search(p, rel) for p in conf.get("testPathIgnorePatterns", [])):
        return False
    filters, i = [], 1
    while i < len(a):
        x = a[i]
        k = x.partition("=")[0]
        if k in ("-t", "--testNamePattern", "--listTests", "-o", "--onlyChanged", "--shard", "--findRelatedTests",
                 "--changedSince", "--lastCommit", "--onlyFailures", "-f", "--bail", "-b"):
            return False  # a name filter, a listing, or a subset
        if k in ("--testPathPattern", "--testPathPatterns"):
            filters.append(x.partition("=")[2] or (a[i + 1] if i + 1 < len(a) else "")); i += 1 if "=" in x else 2; continue
        if k in ("--testPathIgnorePatterns",):
            pat = x.partition("=")[2] or (a[i + 1] if i + 1 < len(a) else "")
            if re.search(pat, rel):
                return False
            i += 1 if "=" in x else 2; continue
        if not x.startswith("-"):
            filters.append(x)
        i += 1
    return not filters or any(re.search(f, rel) for f in filters)


def exceptions(root: str) -> dict[str, str]:
    path = os.path.join(root, "scripts", "test-inventory-exceptions.txt")
    out = {}
    if os.path.exists(path):
        for line in open(path):
            line = line.strip()
            if line and not line.startswith("#"):
                key, _, reason = line.partition(" — ")
                if not reason.strip():
                    sys.exit(f"{path}: an exception needs a reason after ' — ': {line}")
                out[key.strip()] = reason.strip()
    return out


def describe(alt: Req) -> str:
    bits = []
    if alt.need: bits.append("features " + ",".join(sorted(alt.need)))
    if alt.without: bits.append("without " + ",".join(sorted(alt.without)))
    if alt.cfgs: bits.append("RUSTFLAGS --cfg " + ",".join(sorted(alt.cfgs)))
    if alt.impossible: bits.append("a cfg this check cannot evaluate: " + alt.impossible)
    return "; ".join(bits) or "no features"


def check(root: str, workflows: str) -> list[str]:
    crs = crates(root)
    root_pkg = tomllib.load(open(os.path.join(root, "Cargo.toml"), "rb"))["package"]["name"]
    cmds = ci_commands(root, workflows)
    runs = [r for r in (cargo_run(c) for c in cmds) if r]
    exc = exceptions(root)
    missing: list[str] = []

    targets = rust_targets(root, crs)
    # The observed job (ci-test-coverage.py) keys a test by its target file and name, not its crate — a log
    # does not say which crate a `tests/x.rs` belongs to. Two crates' same-named integration tests would mask
    # each other there, so they are refused here (blackboard's `failover.rs` masked tuple-space's).
    seen: dict = {}
    for t in targets:
        if t.kind == "integration":
            for n in t.names:
                k = (t.name, n)
                if k in seen and seen[k] != t.pkg:
                    missing.append(f"integration {t.pkg}::{t.name}::{n}  (the same file and test name as in {seen[k]} — "
                                   f"the observed coverage job cannot tell them apart; rename one)")
                seen.setdefault(k, t.pkg)
    unified = [None if r.all_features else unified_features(root, crs, r, root_pkg) for r in runs]
    for t in targets:
        ok = any(runs_package(r, t.pkg, crs, root_pkg) and runs_kind(r, t)
                 and satisfies(r, None if u is None else u.get(t.pkg, set()), alt)
                 for r, u in zip(runs, unified) for alt in t.alts)
        key = f"{t.kind} {t.pkg}::{t.name}"
        if not ok and key not in exc:
            missing.append(f"{key}  (needs {' OR '.join(describe(a) for a in t.alts)})")

    for f in sorted(set(tracked(root, "test_*.py", "*_test.py"))):
        if not f.endswith(".py"):
            continue
        guard = live_guard(root, f)
        key = f"python {f}"
        if guard == "":
            if key not in exc:
                missing.append(f"{key}  (a live test with no *_LIVE_REQUIRED guard: a step that forgot its node would skip it green)")
            continue
        ok = any(pytest_collects(c, f) and (guard is None or c.env.get(guard)) for c in cmds)
        if not ok and key not in exc:
            missing.append(f"{key}" + (f"  (a live test: needs a step that collects it with {guard} set)" if guard else ""))

    for f in sorted(tracked(root, "*.test.ts")):
        guard = live_guard(root, f)
        key = f"typescript {f}"
        ok = any(jest_collects(root, c, f) and (not guard or c.env.get(guard)) for c in cmds)
        if guard == "":
            ok = False
        if not ok and key not in exc:
            missing.append(key + (f"  (a live test: needs a step that runs it with {guard} set)" if guard else ""))

    named = set()
    for c in cmds:
        a = c.argv
        if os.path.basename(a[0]) != "cargo" or "fuzz" not in a or "run" not in a:
            continue
        rest, i = a[a.index("run") + 1:], 0
        while i < len(rest):
            x = rest[i]
            if x == "--":
                break
            if x.startswith("-"):
                i += 1 if "=" in x or x in ("-O", "--release", "--debug-assertions", "-a", "--dev") else 2
                continue
            named.add(x)
            break
    looped = any("$" in n for n in named) and any(
        os.path.basename(c.argv[0]) == "cargo" and "fuzz" in c.argv and "list" in c.argv for c in cmds)
    for f in sorted(glob.glob(os.path.join(root, "fuzz", "fuzz_targets", "*.rs"))):
        name = os.path.basename(f)[:-3]
        if not (looped or name in named) and f"fuzz {name}" not in exc:
            missing.append(f"fuzz {name}")
    return missing


def main() -> int:
    root = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
    workflows = os.path.join(root, ".github", "workflows")
    if len(sys.argv) == 3:
        root, workflows = sys.argv[1], sys.argv[2]
    missing = check(root, workflows)
    exc = exceptions(root)
    if missing:
        print("check-test-inventory: these tests run in no CI step that would run them (verification policy rule 3):")
        for m in missing:
            print(f"  - {m}")
        print("Add a CI step that runs them with what they need, or list them in "
              "scripts/test-inventory-exceptions.txt as '<key> — <reason>'.")
        return 1
    print(f"check-test-inventory: ok — every inventoried test requirement has a CI step that runs it "
          f"({len(exc)} stated exception(s))")
    return 0


if __name__ == "__main__":
    sys.exit(main())
