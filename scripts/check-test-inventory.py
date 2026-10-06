#!/usr/bin/env python3
"""Verification policy rule 3, checked positively: every test in the repository runs in some CI step.

A gate that refuses *named* test files can always be bypassed by a spelling it does not know, and it
cannot see the failure that actually happened twice: a test file gated on a feature that no CI step
enables. So this script works the other way round. It **inventories** every test target —

  * each Rust integration test of each workspace crate (`tests/*.rs`, `tests/*/main.rs`, `[[test]]`),
    with the features it needs (`required-features`, or the file's top-level `#![cfg(feature ...)]`),
  * each crate's binary unit tests (`#[cfg(test)]` under `src/bin/`) and doctests,
  * each Python and TypeScript test file,
  * each fuzz target,

— and for each finds a CI step that runs it **with the features it needs**, resolving feature
implications through `[features]`. A target no step covers fails the check, unless it is listed in
`scripts/test-inventory-exceptions.txt` with a reason.

Run: `python3 scripts/check-test-inventory.py` (in `make check` and CI).
"""
from __future__ import annotations

import glob
import os
import re
import shlex
import sys
import tomllib

ROOT = os.path.normpath(os.path.join(os.path.dirname(__file__), ".."))
os.chdir(ROOT)


# ── CI steps ──────────────────────────────────────────────────────────────────────────────────────

def ci_commands() -> list[tuple[str, dict[str, str]]]:
    """Every shell command CI runs: (command, env). Multi-line `run:` blocks are joined on
    continuations and split on `&&` / `;` / newlines; `make <target>` is expanded one level."""
    out: list[tuple[str, dict[str, str]]] = []
    for wf in sorted(glob.glob(".github/workflows/*.yml") + glob.glob(".github/workflows/*.yaml")):
        text = open(wf).read()
        lines = text.split("\n")
        i = 0
        while i < len(lines):
            line = lines[i]
            m = re.match(r"^(\s*)(?:- )?run:\s*(.*)$", line)
            if not m:
                i += 1
                continue
            indent, rest = len(m.group(1)), m.group(2)
            body: list[str] = []
            if rest.strip() in ("|", ">", "|-", ">-", "|+", ">+"):
                i += 1
                while i < len(lines) and (lines[i].strip() == "" or len(lines[i]) - len(lines[i].lstrip()) > indent):
                    body.append(lines[i].strip())
                    i += 1
            else:
                body.append(rest.strip())
                i += 1
            env = step_env(lines, i)
            script = "\n".join(body).replace("\\\n", " ")
            for cmd in re.split(r"\n|&&|;", script):
                cmd = cmd.strip()
                if cmd and not cmd.startswith("#"):
                    out.append((cmd, env))
    # one level of `make <target>` expansion
    makefile = open("Makefile").read() if os.path.exists("Makefile") else ""
    expanded: list[tuple[str, dict[str, str]]] = []
    for cmd, env in out:
        expanded.append((cmd, env))
        mm = re.match(r"^make\s+([\w-]+)", cmd)
        if mm:
            tm = re.search(rf"^{re.escape(mm.group(1))}:[^\n]*\n((?:\t[^\n]*\n?)*)", makefile, re.M)
            if tm:
                for mline in tm.group(1).split("\n"):
                    mline = mline.strip()
                    if mline:
                        expanded.append((re.sub(r"#.*$", "", mline).strip(), env))
    return expanded


def step_env(lines: list[str], after: int) -> dict[str, str]:
    """The `env:` mapping of the step whose `run:` ended just before `after` (best effort)."""
    env: dict[str, str] = {}
    j = after
    while j < len(lines) and lines[j].strip().startswith(("env:", "#")) is False and lines[j].strip() != "":
        break
    if j < len(lines) and lines[j].strip() == "env:":
        k = j + 1
        while k < len(lines) and re.match(r"^\s+[A-Z_]+:\s*", lines[k]):
            key, _, val = lines[k].strip().partition(":")
            env[key] = val.strip()
            k += 1
    return env


# ── crates and features ───────────────────────────────────────────────────────────────────────────

def crates() -> dict[str, str]:
    """package name → directory, for the root and every workspace member with a Cargo.toml."""
    root = tomllib.load(open("Cargo.toml", "rb"))
    out = {root["package"]["name"]: "."}
    for member in root.get("workspace", {}).get("members", []):
        path = os.path.join(member, "Cargo.toml")
        if os.path.exists(path):
            out[tomllib.load(open(path, "rb"))["package"]["name"]] = member
    return out


def feature_closure(pkg_dir: str, enabled: set[str], default: bool) -> set[str]:
    manifest = tomllib.load(open(os.path.join(pkg_dir, "Cargo.toml"), "rb"))
    table = manifest.get("features", {})
    todo = set(enabled) | ({"default"} if default and "default" in table else set())
    seen: set[str] = set()
    while todo:
        f = todo.pop()
        if f in seen:
            continue
        seen.add(f)
        for implied in table.get(f, []):
            if "/" not in implied and not implied.startswith("dep:"):
                todo.add(implied)
    return seen


# ── what a cargo command runs ─────────────────────────────────────────────────────────────────────

class CargoRun:
    def __init__(self, cmd: str, cwd_pkg: str | None = None) -> None:
        self.ok = False
        if "ci-retest.sh" in cmd:
            cmd = "cargo test " + cmd.split("ci-retest.sh", 1)[1]
        m = re.search(r"\bcargo\s+(?:\+\S+\s+)?(test|nextest\s+run)\b(.*)$", cmd)
        if not m:
            return
        try:
            args = shlex.split(m.group(2))
        except ValueError:
            return
        self.ok = True
        self.pkgs: list[str] = []
        self.features: set[str] = set()
        self.default = True
        self.all_features = False
        self.workspace = False
        self.selectors: set[str] = set()
        self.tests: set[str] = set()
        self.filtered = False
        i = 0
        while i < len(args):
            a = args[i]
            nxt = args[i + 1] if i + 1 < len(args) else None
            if a in ("-p", "--package") and nxt:
                self.pkgs.append(nxt); i += 2; continue
            if a.startswith("--package="):
                self.pkgs.append(a.split("=", 1)[1]); i += 1; continue
            if a in ("--features", "-F") and nxt:
                self.features |= set(re.split(r"[ ,]+", nxt.strip("\"'"))) - {""}; i += 2; continue
            if a.startswith("--features="):
                self.features |= set(re.split(r"[ ,]+", a.split("=", 1)[1])) - {""}; i += 1; continue
            if a == "--no-default-features":
                self.default = False; i += 1; continue
            if a == "--all-features":
                self.all_features = True; i += 1; continue
            if a in ("--workspace", "--all"):
                self.workspace = True; i += 1; continue
            if a in ("--lib", "--bins", "--tests", "--doc", "--examples", "--benches", "--all-targets"):
                self.selectors.add(a); i += 1; continue
            if a == "--test" and nxt:
                self.selectors.add("--test"); self.tests.add(nxt); i += 2; continue
            if a.startswith("--test="):
                self.selectors.add("--test"); self.tests.add(a.split("=", 1)[1]); i += 1; continue
            if a == "--bin" and nxt:
                self.selectors.add("--bin"); i += 2; continue
            if a == "--":
                rest = [x for x in args[i + 1:] if not x.startswith("-")]
                if rest:
                    self.filtered = True
                break
            if a.startswith("-"):
                i += 1; continue
            self.filtered = True  # a positional test-name filter
            i += 1

    def runs_package(self, pkg: str, root_pkg: str) -> bool:
        return self.workspace or pkg in self.pkgs or (not self.pkgs and pkg == root_pkg)

    def has_features(self, pkg_dir: str, need: set[str]) -> bool:
        if self.all_features:
            return True
        return need <= feature_closure(pkg_dir, self.features, self.default)

    def runs_integration(self, name: str) -> bool:
        if self.filtered:
            return False
        if not self.selectors or "--tests" in self.selectors or "--all-targets" in self.selectors:
            return True
        return "--test" in self.selectors and ("*" in self.tests or name in self.tests)

    def runs_bins(self) -> bool:
        return not self.filtered and (not self.selectors or bool({"--bins", "--tests", "--all-targets"} & self.selectors))

    def runs_doc(self) -> bool:
        return not self.filtered and (not self.selectors or "--doc" in self.selectors)


# ── the inventory ─────────────────────────────────────────────────────────────────────────────────

def cfg_features(path: str) -> set[str] | None:
    """Features a top-level `#![cfg(...)]` requires; `None` if the file is gated on something else
    (a target, `not(...)`), which this check cannot satisfy and reports."""
    head = open(path).read(4000)
    m = re.search(r"^#!\[cfg\((.*)\)\]\s*$", head, re.M)
    if not m:
        return set()
    expr = m.group(1)
    if "not(" in expr or "any(" in expr or "target" in expr:
        return None
    return set(re.findall(r'feature\s*=\s*"([^"]+)"', expr))


def rust_targets(crs: dict[str, str]) -> list[tuple[str, str, str, set[str] | None]]:
    """(kind, package, name, needed features) for integration tests, bin unit tests and doctests."""
    out = []
    for pkg, d in crs.items():
        manifest = tomllib.load(open(os.path.join(d, "Cargo.toml"), "rb"))
        declared = {t["name"]: t for t in manifest.get("test", [])}
        files = glob.glob(os.path.join(d, "tests", "*.rs")) + glob.glob(os.path.join(d, "tests", "*", "main.rs"))
        names = set()
        for f in files:
            name = os.path.basename(os.path.dirname(f)) if f.endswith("main.rs") else os.path.basename(f)[:-3]
            names.add(name)
            t = declared.get(name, {})
            if t.get("test") is False:
                continue
            need = set(t["required-features"]) if "required-features" in t else cfg_features(f)
            out.append(("integration", pkg, name, need))
        for name, t in declared.items():
            if name not in names and t.get("test") is not False:
                out.append(("integration", pkg, name, set(t.get("required-features", []))))
        bins_with_tests = [b for b in glob.glob(os.path.join(d, "src", "bin", "**", "*.rs"), recursive=True)
                           if "#[cfg(test)]" in open(b).read()]
        if bins_with_tests:
            out.append(("bin-tests", pkg, ",".join(sorted({os.path.relpath(b, d) for b in bins_with_tests})), set()))
        lib = os.path.join(d, "src", "lib.rs")
        if os.path.exists(lib):
            docs = [f for f in glob.glob(os.path.join(d, "src", "**", "*.rs"), recursive=True)
                    if re.search(r"^\s*//[/!]\s*```(?!text|toml|json|sh|bash|console|ignore|mermaid|yaml|python|ts|typescript|js|http|diff|ini|dot)", open(f).read(), re.M)]
            if docs:
                out.append(("doctests", pkg, "lib", set()))
    return out


def exceptions() -> dict[str, str]:
    path = "scripts/test-inventory-exceptions.txt"
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


def main() -> int:
    crs = crates()
    root_pkg = tomllib.load(open("Cargo.toml", "rb"))["package"]["name"]
    cmds = ci_commands()
    runs = [CargoRun(c) for c, _ in cmds]
    runs = [r for r in runs if r.ok]
    exc = exceptions()
    missing: list[str] = []

    for kind, pkg, name, need in rust_targets(crs):
        key = f"{kind} {pkg}::{name}"
        d = crs[pkg]
        if need is None:
            covered = False
        elif kind == "integration":
            covered = any(r.runs_package(pkg, root_pkg) and r.runs_integration(name) and r.has_features(d, need) for r in runs)
        elif kind == "bin-tests":
            covered = any(r.runs_package(pkg, root_pkg) and r.runs_bins() for r in runs)
        else:
            covered = any(r.runs_package(pkg, root_pkg) and r.runs_doc() for r in runs)
        if not covered and key not in exc:
            missing.append(f"{key}  (needs features: {sorted(need) if need is not None else 'an unsupported cfg'})")

    # Python: each test file must sit under a directory some pytest invocation collects.
    py_dirs = []
    for c, _ in cmds:
        if re.search(r"\bpytest\b", c):
            py_dirs += [a.rstrip("/") for a in shlex.split(c) if not a.startswith("-") and "/" in a and not a.endswith(".py")]
    for f in sorted(glob.glob("**/tests/**/test_*.py", recursive=True) + glob.glob("**/tests/test_*.py", recursive=True)):
        if "node_modules" in f or f.startswith(("target", ".venv")):
            continue
        key = f"python {f}"
        if not any(f.startswith(d + "/") for d in py_dirs) and key not in exc:
            missing.append(key)

    # TypeScript: a bare `npx jest` (or `npm test`) in mycelium-ts collects every *.test.ts.
    ts_bare = any(re.search(r"\b(npx jest|npm test)\s*$", c) for c, _ in cmds)
    for f in sorted(glob.glob("mycelium-ts/tests/**/*.test.ts", recursive=True)):
        if not ts_bare and f"typescript {f}" not in exc:
            missing.append(f"typescript {f}")

    # Fuzz targets: each must be run by name or by a loop over `cargo fuzz list`.
    fuzz_loop = any("cargo fuzz list" in c or "fuzz list" in c for c, _ in cmds)
    fuzz_named = " ".join(c for c, _ in cmds if "fuzz run" in c)
    for f in sorted(glob.glob("fuzz/fuzz_targets/*.rs")):
        name = os.path.basename(f)[:-3]
        if not fuzz_loop and not re.search(rf"\b{re.escape(name)}\b", fuzz_named) and f"fuzz {name}" not in exc:
            missing.append(f"fuzz {name}")

    if missing:
        print("check-test-inventory: these tests run in no CI step (verification policy rule 3):")
        for m in missing:
            print(f"  - {m}")
        print("Add a CI step that runs them with the features they need, or list them in "
              "scripts/test-inventory-exceptions.txt as '<key> — <reason>'.")
        return 1
    print(f"check-test-inventory: ok — every test target runs in CI ({len(exc)} stated exception(s))")
    return 0


if __name__ == "__main__":
    sys.exit(main())
