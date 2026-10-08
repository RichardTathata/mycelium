#!/usr/bin/env python3
"""Self-test for check-example-matrix.py and example-case.sh: each mutation of a small fixture repository must
fail the check (or the listing) naming what it broke, and each edit that only *mentions* an example must leave it
passing; the unmutated fixture must pass. --list: its cases.

The fixture's command text is assembled from pieces (CR, BUILT), never written whole on one line: this file is
itself a script CI runs, so the check reads it, and a literal example run here would read as one.
"""
import os
import shutil
import subprocess
import sys
import tempfile

SUITE = "scripts/test-check-example-matrix.py"
CASES = ["baseline", "tick-without-run", "run-with-dot-row", "run-without-row", "harness-needs-its-name",
         "unwrapped-run", "step-that-never-runs", "suite-case-needs-variable-target", "list-reads-calls",
         "list-refuses-non-literal", "wrapper-marks-and-runs", "wrapper-refuses-no-name",
         # review round 1 (#566)
         "path-filtered-is-not-every-change", "every-change-is-not-path-filtered", "wrapper-fronts-only-cargo-run",
         "build-then-run-on-one-line", "name-starting-with-dash", "heredoc-is-not-code",
         "python-docstring-is-not-code", "mention-is-not-execution", "built-path-mention-is-not-execution",
         "variable-program-is-execution", "harness-name-with-a-row", "makefile-suite-must-be-reached",
         # review round 2 (#566)
         "pr-excluded-job-is-not-every-change", "pr-excluded-step-is-not-every-change",
         "pr-types-filter-is-not-every-change", "pr-branches-ignore-main-is-not-every-change",
         # the unsafe-direction approximations closed
         "single-quoted-substitution-is-not-a-run", "unused-array-is-not-a-run",
         "dead-branch-and-uncalled-function-are-not-runs", "python-list-outside-a-subprocess-call-is-not-a-run",
         # review round 1 (#571)
         "quote-spanning-lines-is-one-word", "brace-as-argument-is-not-a-closer", "subshell-bodied-function",
         "if-chain-after-a-true-branch-is-dead", "plain-array-variable-runs-its-first-word",
         "short-circuit-and-exit-are-dead", "unparseable-python-is-named", "negated-and-compound-conditions",
         "case-pattern-is-not-a-command"]
if sys.argv[1:] == ["--list"]:
    for c in CASES:
        print(f"@@case-list@@ {SUITE}::{c}")
    sys.exit(0)

HERE = os.path.dirname(os.path.abspath(__file__))
CHECK = os.environ.get("CHECK_EXAMPLE_MATRIX", os.path.join(HERE, "check-example-matrix.py"))
WRAPPER = os.environ.get("EXAMPLE_CASE_SH", os.path.join(HERE, "example-case.sh"))
CR = "cargo" + " run"
BUILT = "target/" + "debug/examples/"
TQ = '"' * 3
C = "car" + "go"
PYCR = '"car' + 'go", "run"'

WORKFLOW = f"""name: fixture
on: [pull_request]
jobs:
  t:
    runs-on: ubuntu-latest
    steps:
      - run: scripts/example-case.sh {CR} --example alpha
      - run: bash smoke.sh
      - run: ./{BUILT}harness_x --serve
      - name: List
        run: |
          echo "@@test-universe@@ begin scripts"
          bash examples/suite/ci_smoke.sh --list
          echo "@@test-universe@@ end"
      - run: bash examples/suite/ci_smoke.sh
"""
FILTERED_WF = f"""name: docker
on:
  push:
    branches: [main]
    paths: ['docker/**']
  pull_request:
    paths: ['docker/**']
jobs:
  d:
    runs-on: ubuntu-latest
    steps:
      - run: scripts/example-case.sh {CR} --example epsilon
"""
SMOKE = f"#!/usr/bin/env bash\n{CR} -q --example beta\n"
OTHER = f"#!/usr/bin/env bash\n{CR} -q --example delta\n"
SUITE_SH = f"""#!/usr/bin/env bash
if [[ "${{1:-}}" == --list ]]; then echo "@@case-list@@ examples/suite/ci_smoke.sh::gamma"; exit 0; fi
for b in gamma; do
  {CR} -q --bin "$b"
done
"""
README = """# Examples

| Example | Level | CI |
|---|:-:|:-:|
| **A group** | | |
| [`alpha`](a.md) · [src](alpha.rs) | Adv | ✓ |
| [`beta`](b.md) · [src](beta.rs) | Adv | ✓ |
| [`gamma`](g.md) · [src](gamma.rs) | Adv | ✓ |
| [`suite`](s.md) · [src](suite) | Adv | ✓ |
| [`delta`](d.md) · [src](delta.rs) | Adv | · |
| [`epsilon`](e.md) · [src](epsilon.rs) | Adv | ✓ᵖ |

**Harness binaries — deliberately not rows above.** `harness_x` is a fixture a suite starts.
"""


def case(name):
    assert name in CASES, name
    print(f"@@case@@ {SUITE}::{name}", flush=True)


def fixture(edit=None):
    root = tempfile.mkdtemp(prefix="example-matrix-")
    files = {".github/workflows/ci.yml": WORKFLOW, ".github/workflows/docker.yml": FILTERED_WF, "smoke.sh": SMOKE,
             "other.sh": OTHER, "examples/suite/ci_smoke.sh": SUITE_SH, "examples/README.md": README}
    if edit:
        files = edit(dict(files))
    for rel, text in files.items():
        os.makedirs(os.path.dirname(os.path.join(root, rel)) or root, exist_ok=True)
        with open(os.path.join(root, rel), "w", encoding="utf-8") as f:
            f.write(text)
    return root


def run_check(edit=None):
    root = fixture(edit)
    try:
        p = subprocess.run([sys.executable, CHECK, "--root", root], capture_output=True, text=True)
        return p.returncode, p.stdout + p.stderr
    finally:
        shutil.rmtree(root)


def fails(edit, *expected):
    code, out = run_check(edit)
    assert code == 1, f"the check passed a mutation it must fail:\n{out}"
    for e in expected:
        assert e in out, f"expected {e!r} in:\n{out}"


def passes(edit):
    code, out = run_check(edit)
    assert code == 0, f"the check failed an edit it must pass:\n{out}"


def sub(rel, old, new):
    def edit(files):
        assert old in files[rel], f"fixture no longer matches: {rel}: {old!r}"
        files[rel] = files[rel].replace(old, new)
        return files
    return edit


def add(rel, text):
    def edit(files):
        files[rel] = text
        return files
    return edit


def both(*edits):
    def edit(files):
        for e in edits:
            files = e(files)
        return files
    return edit


def step(cmd):
    """Append a step to the every-change workflow."""
    return sub(".github/workflows/ci.yml", "      - run: bash examples/suite/ci_smoke.sh\n",
               f"      - run: bash examples/suite/ci_smoke.sh\n      - run: {cmd}\n")


def listing(text):
    d = tempfile.mkdtemp(prefix="example-list-")
    path = os.path.join(d, "ci.yml")
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)
    try:
        p = subprocess.run([sys.executable, CHECK, "--list", path], capture_output=True, text=True)
        return p.returncode, p.stdout, p.stderr
    finally:
        shutil.rmtree(d)


FAKE = tempfile.mkdtemp(prefix="fake-cargo-")
with open(os.path.join(FAKE, "cargo"), "w") as f:
    f.write('#!/usr/bin/env bash\necho "ran $*"\nexit 3\n')
os.chmod(os.path.join(FAKE, "cargo"), 0o755)
# CI's BASH_ENV (scripts/ci-merge-stderr.sh) merges stderr into stdout in every bash it starts; the wrapper's streams
# are what these cases assert on, so it runs without it.
ENV = {**{k: v for k, v in os.environ.items() if k != "BASH_ENV"}, "PATH": FAKE + os.pathsep + os.environ["PATH"]}


def wrap(*argv):
    return subprocess.run(["bash", WRAPPER, *argv], capture_output=True, text=True, env=ENV)


case("baseline")
code, out = run_check()
assert code == 0, out

case("tick-without-run")
fails(sub("examples/README.md", "| [`delta`](d.md) · [src](delta.rs) | Adv | · |",
          "| [`delta`](d.md) · [src](delta.rs) | Adv | ✓ |"), "`delta` is ✓ in the CI column")

case("run-with-dot-row")
fails(sub("examples/README.md", "| [`alpha`](a.md) · [src](alpha.rs) | Adv | ✓ |",
          "| [`alpha`](a.md) · [src](alpha.rs) | Adv | · |"), "`alpha` says · in the CI column")

case("run-without-row")
fails(sub("examples/README.md", "| [`beta`](b.md) · [src](beta.rs) | Adv | ✓ |\n", ""),
      "example `beta`", "has no row")

case("harness-needs-its-name")
fails(sub("examples/README.md", "`harness_x` is", "it is"), "example `harness_x`", "has no row")

case("unwrapped-run")
fails(sub(".github/workflows/ci.yml", f"scripts/example-case.sh {CR} --example alpha", f"{CR} --example alpha"),
      "does not front", "`alpha` is ✓ in the CI column")

case("step-that-never-runs")
fails(sub(".github/workflows/ci.yml", f"      - run: scripts/example-case.sh {CR} --example alpha\n",
          f"      - if: false\n        run: scripts/example-case.sh {CR} --example alpha\n"),
      "`alpha` is ✓ in the CI column")

case("suite-case-needs-variable-target")
fails(sub("examples/suite/ci_smoke.sh", '--bin "$b"', "--bin fixed"), "`gamma` is ✓ in the CI column")

case("list-reads-calls")
code, out, err = listing(f"""jobs:
  t:
    steps:
      # a comment naming {CR} --example nope is not a call
      - run: scripts/example-case.sh {CR} -p x --example=one --features a
      - run: |
          scripts/example-case.sh {CR} \\
            --example two -- --example not-this
      - run: cargo build --example built_only
      - run: bash scripts/example-case.sh --list
""")
assert code == 0, err
assert out.split() == ["@@case-list@@", "examples::one", "@@case-list@@", "examples::two"], out

case("list-refuses-non-literal")
code, out, err = listing(f'      - run: scripts/example-case.sh {CR} --example "$name"\n')
assert code == 1 and "without a literal --example NAME" in err, (code, out, err)
code, out, err = listing(f"      - run: {CR} --example plain\n")
assert code == 1 and "does not front" in err, (code, out, err)
code, out, err = listing("      - run: echo nothing\n")
assert code == 1 and "no wrapper call" in err, (code, out, err)

case("wrapper-marks-and-runs")
p = wrap(C, "run", "--example", "seven")
assert p.returncode == 3 and p.stdout == "@@case@@ examples::seven\nran run --example seven\n", p
p = wrap(C, "+1.96.0", "run", "--example=eight", "--", "--example", "nine")
assert p.returncode == 3 and p.stdout.startswith("@@case@@ examples::eight\n"), p

case("wrapper-refuses-no-name")
for argv in ([C, "run"], [C, "run", "--example"], [C, "run", "--", "--example", "x"],
             [C, "run", "--example", "$bad"]):
    p = wrap(*argv)
    assert p.returncode == 2 and p.stdout == "" and "no literal --example NAME" in p.stderr, (argv, p)

case("path-filtered-is-not-every-change")
fails(sub("examples/README.md", "| Adv | ✓ᵖ |", "| Adv | ✓ |"),
      "`epsilon` is ✓ in the CI column, but only a path-filtered workflow executes it")
fails(sub(".github/workflows/docker.yml", "--example epsilon", "--example alpha"), "`epsilon` is ✓ᵖ",
      "no path-filtered workflow executes it")

case("every-change-is-not-path-filtered")
fails(sub("examples/README.md", "| [`alpha`](a.md) · [src](alpha.rs) | Adv | ✓ |",
          "| [`alpha`](a.md) · [src](alpha.rs) | Adv | ✓ᵖ |"), "`alpha` is ✓ᵖ", "mark it ✓")

case("wrapper-fronts-only-cargo-run")
fails(sub(".github/workflows/ci.yml", f"scripts/example-case.sh {CR} --example alpha",
          "scripts/example-case.sh sh -c true --example alpha"), "fronts only", "`alpha` is ✓ in the CI column")
fails(sub(".github/workflows/ci.yml", f"scripts/example-case.sh {CR} --example alpha",
          "scripts/example-case.sh cargo build --example alpha"), "fronts only")
for argv in (["sh", "-c", "true", "--example", "x"], [C, "build", "--example", "x"], ["true"]):
    p = wrap(*argv)
    assert p.returncode == 2 and p.stdout == "" and "fronts only" in p.stderr, (argv, p)

case("build-then-run-on-one-line")
code, out, err = listing(f"      - run: cargo build --example a && {CR} --example b\n"
                         f"      - run: scripts/example-case.sh {CR} --example c\n")
assert code == 1 and "does not front" in err, (code, out, err)

case("name-starting-with-dash")
code, out, err = listing(f"      - run: scripts/example-case.sh {CR} --example -q\n")
assert code == 1 and "without a literal --example NAME" in err, (code, out, err)
p = wrap(C, "run", "--example", "-q")
assert p.returncode == 2 and "no literal --example NAME" in p.stderr, p

# Text in a reached file that is not code, or a file a step only mentions, must not read as an example run: the
# fixture's `delta` row says ·, so a false site fails the check.
case("heredoc-is-not-code")
passes(sub("smoke.sh", f"{CR} -q --example beta\n",
           f"{CR} -q --example beta\ncat <<EOF\n{CR} --example delta\nEOF\n"))

case("python-docstring-is-not-code")
passes(both(step("python3 runner.py"),
            add("runner.py", f'{TQ}Run it as [{PYCR}, "--example", "delta"].\n{TQ}\nprint("hi")\n')))

case("mention-is-not-execution")
passes(step("git diff --stat other.sh"))
passes(step("cp other.sh /tmp/x.sh"))
fails(step("bash other.sh"), "`delta` says ·")
fails(step("./other.sh"), "`delta` says ·")

case("built-path-mention-is-not-execution")
passes(step(f"cp {BUILT}delta /tmp/delta"))
passes(step(f"ls -l {BUILT}delta"))
fails(step(f"./{BUILT}delta --serve"), "`delta` says ·")

case("variable-program-is-execution")
fails(sub("smoke.sh", f"{CR} -q --example beta\n",
          f"{CR} -q --example beta\nBIN=\"$ROOT/{BUILT}delta\"\n\"$BIN\" --serve\n"), "`delta` says ·")
fails(sub("smoke.sh", f"{CR} -q --example beta\n",
          f"{CR} -q --example beta\nRUN=\"{CR} --example delta --\"\n$RUN ask\n"), "`delta` says ·")
passes(sub("smoke.sh", f"{CR} -q --example beta\n",
           f"{CR} -q --example beta\nBIN=\"$ROOT/{BUILT}delta\"\nls \"$BIN\"\n"))

case("harness-name-with-a-row")
fails(both(sub("examples/README.md", "`harness_x` is", "`harness_x` and `delta` are"), step("bash other.sh")),
      "`delta` says ·")

case("makefile-suite-must-be-reached")
MAKEFILE = f"list-x:\n\t@echo '@@case-list@@ Makefile:suite-x::zeta'\n\nsuite-x:\n\t{CR} -q --bin \"$(B)\"\n"
LISTED = sub(".github/workflows/ci.yml", "          bash examples/suite/ci_smoke.sh --list\n",
             "          bash examples/suite/ci_smoke.sh --list\n          make -s list-x\n")
ROW = sub("examples/README.md", "| [`delta`]", "| [`zeta`](z.md) · [src](zeta.rs) | Adv | ✓ |\n| [`delta`]")
fails(both(add("Makefile", MAKEFILE), LISTED, ROW), "`zeta` is ✓ in the CI column")
passes(both(add("Makefile", MAKEFILE), LISTED, ROW, step("make suite-x")))

shutil.rmtree(FAKE)

ALPHA_NOT_EVERY = "`alpha` is ✓ in the CI column, but only a path-filtered workflow executes it"
ALPHA_STEP = f"      - run: scripts/example-case.sh {CR} --example alpha\n"

case("pr-excluded-job-is-not-every-change")
fails(sub(".github/workflows/ci.yml", "  t:\n    runs-on:", "  t:\n    if: github.event_name != 'pull_request'\n    runs-on:"),
      ALPHA_NOT_EVERY)

case("pr-excluded-step-is-not-every-change")
fails(sub(".github/workflows/ci.yml", ALPHA_STEP,
          f"      - if: ${{{{ github.event_name != 'pull_request' }}}}\n        run: scripts/example-case.sh {CR} --example alpha\n"),
      ALPHA_NOT_EVERY)

case("pr-types-filter-is-not-every-change")
fails(sub(".github/workflows/ci.yml", "on: [pull_request]", "on:\n  pull_request:\n    types: [labeled]"), ALPHA_NOT_EVERY)

case("pr-branches-ignore-main-is-not-every-change")
fails(sub(".github/workflows/ci.yml", "on: [pull_request]", "on:\n  pull_request:\n    branches-ignore: [main]"), ALPHA_NOT_EVERY)

# Text in a reached script that reads like an example run but never runs one must not pass a row: the fixture's
# `delta` row says ·, so each `passes` below fails against a check that reads it as a run, and each `fails` shows
# the same shape is still read where the shell (or Python) would run it.
def smoke(extra):
    return sub("smoke.sh", f"{CR} -q --example beta\n", f"{CR} -q --example beta\n{extra}")


case("single-quoted-substitution-is-not-a-run")
passes(smoke(f"echo '$({CR} --example delta)'\n"))
passes(smoke(f"echo 'a \"$({CR} --example delta)\" b'\n"))
fails(smoke(f"echo \"$({CR} --example delta)\"\n"), "`delta` says ·")
fails(smoke(f"X=$({CR} --example delta)\n"), "`delta` says ·")
fails(smoke(f"echo \"$(cd x && $({CR} --example delta))\"\n"), "`delta` says ·")

case("unused-array-is-not-a-run")
passes(smoke(f"CMD=({CR} --example delta)\n"))
passes(smoke(f"EMPTY=()\nEMPTY+=(x)\nCMD=({CR} --example delta); echo done\n"))
passes(smoke(f"CMD=(\n  {CR}\n  --example delta\n)\necho \"${{CMD[@]}}\"\n"))
fails(smoke(f"CMD=({CR} --example delta)\n\"${{CMD[@]}}\" --serve\n"), "`delta` says ·")
fails(smoke(f"local BIN=(./{BUILT}delta --serve)\n\"${{BIN[@]}}\"\n"), "`delta` says ·")

case("dead-branch-and-uncalled-function-are-not-runs")
passes(smoke(f"if false; then\n  {CR} --example delta\nfi\n"))
passes(smoke(f"while false; do {CR} --example delta; done\n"))
passes(smoke(f"if true; then echo; else\n  {CR} --example delta\nfi\n"))
passes(smoke(f"run_it() {{\n  {CR} --example delta\n}}\n"))
passes(smoke(f"function run_it {{ {CR} --example delta; }}\nuncalled() {{ run_it; }}\n"))
passes(smoke(f"if false; then\n  run_it() {{ {CR} --example delta; }}\nfi\nrun_it\n"))
fails(smoke(f"if false; then echo; else\n  {CR} --example delta\nfi\n"), "`delta` says ·")
fails(smoke(f"if [ -n \"$X\" ]; then\n  {CR} --example delta\nfi\n"), "`delta` says ·")
fails(smoke(f"for m in a b; do\n  {CR} --example delta\ndone\n"), "`delta` says ·")
fails(smoke(f"run_it() {{\n  {CR} --example delta\n}}\nrun_it\n"), "`delta` says ·")
fails(smoke(f"inner() {{ {CR} --example delta; }}\nouter() {{\n  inner\n}}\nif ! outer; then exit 1; fi\n"),
      "`delta` says ·")
fails(smoke(f"cleanup() {{ {CR} --example delta; }}\ntrap cleanup EXIT\n"), "`delta` says ·")
passes(smoke(f"run_it() {{ bash other.sh; }}\n"))
fails(smoke(f"run_it() {{ bash other.sh; }}\nrun_it\n"), "`delta` says ·")

case("python-list-outside-a-subprocess-call-is-not-a-run")
RUNNER = step("python3 runner.py")
passes(both(RUNNER, add("runner.py", f'print("usage:",\n      [{PYCR}, "--example", "delta"])\n')))
passes(both(RUNNER, add("runner.py", f'print(f"run it as "\n      f\'{PYCR}, "--example", "delta"\')\n')))
passes(both(RUNNER, add("runner.py", f'HELP = [{PYCR}, "--example", "delta"]\nprint(HELP)\n')))
fails(both(RUNNER, add("runner.py", f'import subprocess\nsubprocess.run(\n    [{PYCR},\n     "--example", "delta"],\n'
                                    f'    check=True)\n')), "`delta` says ·")
fails(both(RUNNER, add("runner.py", f'from subprocess import check_call as cc\nCMD = [{PYCR}, "--example", "delta"]\n'
                                    f'cc(CMD)\n')), "`delta` says ·")
fails(both(RUNNER, add("runner.py", f'import os\nos.execvp("{C}", [{PYCR}, "--example", "delta"])\n')),
      "`delta` says ·")
fails(both(RUNNER, add("runner.py", f'import asyncio\nasyncio.create_subprocess_exec({PYCR}, "--example", "delta")\n')),
      "`delta` says ·")

case("quote-spanning-lines-is-one-word")
passes(smoke(f"echo '\n$({CR} --example delta)\n'\n"))
passes(smoke(f"MSG=\"usage:\n{CR} --example delta\n\"\necho \"$MSG\"\n"))
passes(smoke(f"echo \"it's\n{CR} --example delta\n\"\n"))
fails(smoke(f"MSG=\"a\nb\"\n{CR} --example delta\n"), "`delta` says ·")
fails(smoke(f"# don't\n{CR} --example delta\n"), "`delta` says ·")

case("brace-as-argument-is-not-a-closer")
passes(smoke(f"run_it() {{\n  echo }}\n  {CR} --example delta\n}}\n"))
fails(smoke(f"run_it() {{\n  echo }}\n  {CR} --example delta\n}}\nrun_it\n"), "`delta` says ·")

case("subshell-bodied-function")
passes(smoke(f"run_it() (\n  {CR} --example delta\n)\n"))
fails(smoke(f"run_it() (\n  {CR} --example delta\n)\nrun_it\n"), "`delta` says ·")
fails(smoke(f"( cd x && {CR} --example delta )\n"), "`delta` says ·")

case("if-chain-after-a-true-branch-is-dead")
passes(smoke(f"if [ -n x ]; then :; elif true; then :; elif [ -z y ]; then :; else\n  {CR} --example delta\nfi\n"))
passes(smoke(f"if true; then :; elif [ -n x ]; then\n  {CR} --example delta\nfi\n"))
fails(smoke(f"if false; then :; elif [ -n x ]; then :; else\n  {CR} --example delta\nfi\n"), "`delta` says ·")
fails(smoke(f"if [ -n x ]; then :; elif true; then\n  {CR} --example delta\nfi\n"), "`delta` says ·")

case("plain-array-variable-runs-its-first-word")
passes(smoke(f"CMD=({CR} --example delta)\n$CMD\n"))
fails(smoke(f"CMD=({CR} --example delta)\n${{CMD[*]}}\n"), "`delta` says ·")
fails(smoke(f"BIN=(./{BUILT}delta --serve)\n$BIN\n"), "`delta` says ·")

case("short-circuit-and-exit-are-dead")
passes(smoke(f"false && {CR} --example delta\n"))
passes(smoke(f"true || {CR} --example delta | tee x\n"))
passes(smoke(f"false && echo && {CR} --example delta\n"))
passes(smoke(f"exit 0\n{CR} --example delta\n"))
passes(smoke(f"exit 0\nrun_it() {{ {CR} --example delta; }}\nrun_it\n"))
fails(smoke(f"false || {CR} --example delta\n"), "`delta` says ·")
fails(smoke(f"false && echo || {CR} --example delta\n"), "`delta` says ·")
fails(smoke(f"! false && {CR} --example delta\n"), "`delta` says ·")
fails(smoke(f"[ -n x ] || exit 1\n{CR} --example delta\n"), "`delta` says ·")
fails(smoke(f"if [ -n x ]; then exit 0; fi\n{CR} --example delta\n"), "`delta` says ·")
fails(smoke(f"run_it() {{ exit 0; }}\n{CR} --example delta\n"), "`delta` says ·")
fails(smoke(f"X=$(exit 3)\n{CR} --example delta\n"), "`delta` says ·")

case("unparseable-python-is-named")
fails(both(step("python3 runner.py"), add("runner.py", "def broken(:\n")), "runner.py", "cannot parse")

case("negated-and-compound-conditions")
passes(smoke(f"if ! true; then\n  {CR} --example delta\nfi\n"))
fails(smoke(f"if ! false; then\n  {CR} --example delta\nfi\n"), "`delta` says ·")
fails(smoke(f"if false || true; then\n  {CR} --example delta\nfi\n"), "`delta` says ·")
fails(smoke(f"while false || [ -n x ]; do {CR} --example delta; done\n"), "`delta` says ·")
fails(smoke(f"if true && false; then :; else\n  {CR} --example delta\nfi\n"), "`delta` says ·")

case("case-pattern-is-not-a-command")
passes(smoke(f"case \"$1\" in\n  other.sh) echo ;;\n  (x|y) echo ;;\nesac\n"))
fails(smoke(f"case \"$1\" in\n  a) echo ;;\n  b) bash other.sh ;;\nesac\n"), "`delta` says ·")
fails(smoke(f"case \"$1\" in\n  a) echo ;;\nesac\n{CR} --example delta\n"), "`delta` says ·")

print(f"test-check-example-matrix: {len(CASES)} cases passed")
