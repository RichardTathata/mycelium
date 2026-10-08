#!/usr/bin/env python3
"""Self-test for check-example-matrix.py and example-case.sh: each mutation of a small fixture repository must
fail the check (or the listing) naming what it broke; the unmutated fixture must pass. --list: its cases.

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
         "list-refuses-non-literal", "wrapper-marks-and-runs", "wrapper-refuses-no-name"]
if sys.argv[1:] == ["--list"]:
    for c in CASES:
        print(f"@@case-list@@ {SUITE}::{c}")
    sys.exit(0)

HERE = os.path.dirname(os.path.abspath(__file__))
CHECK = os.path.join(HERE, "check-example-matrix.py")
WRAPPER = os.path.join(HERE, "example-case.sh")
CR = "cargo" + " run"
BUILT = "target/" + "debug/examples/"

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
SMOKE = f"#!/usr/bin/env bash\n{CR} -q --example beta\n"
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

**Harness binaries — deliberately not rows above.** `harness_x` is a fixture a suite starts.
"""


def case(name):
    assert name in CASES, name
    print(f"@@case@@ {SUITE}::{name}", flush=True)


def fixture(edit=None):
    root = tempfile.mkdtemp(prefix="example-matrix-")
    files = {".github/workflows/ci.yml": WORKFLOW, "smoke.sh": SMOKE, "examples/suite/ci_smoke.sh": SUITE_SH,
             "examples/README.md": README}
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


def sub(rel, old, new):
    def edit(files):
        assert old in files[rel], f"fixture no longer matches: {rel}: {old!r}"
        files[rel] = files[rel].replace(old, new)
        return files
    return edit


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
p = subprocess.run(["bash", WRAPPER, "sh", "-c", "echo ran; exit 3", "--example", "seven"],
                   capture_output=True, text=True)
assert p.returncode == 3 and p.stdout == "@@case@@ examples::seven\nran\n", (p.returncode, p.stdout, p.stderr)
p = subprocess.run(["bash", WRAPPER, "true", "--example=eight", "--", "--example", "nine"],
                   capture_output=True, text=True)
assert p.returncode == 0 and p.stdout == "@@case@@ examples::eight\n", (p.returncode, p.stdout, p.stderr)

case("wrapper-refuses-no-name")
for argv in (["true"], ["true", "--example"], ["true", "--", "--example", "x"], ["true", "--example", "$bad"]):
    p = subprocess.run(["bash", WRAPPER, *argv], capture_output=True, text=True)
    assert p.returncode == 2 and p.stdout == "" and "no literal --example NAME" in p.stderr, (argv, p)

print(f"test-check-example-matrix: {len(CASES)} cases passed")
