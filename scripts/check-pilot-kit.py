#!/usr/bin/env python3
"""The pilot kit's gate (adopter-surface Y10), in `make check`.

Four things the kit promises that nothing else holds:
  1. the shared-responsibility matrix's doctor-line table covers the guarantee catalogue — every id once, no id the
     catalogue lacks, every row it names a row of the matrix (or `*no row*` with a reason);
  2. the pilot runbook names the kit's directory, and the proof page keeps the named-deployment line open while saying
     the kit is not the deployment;
  3. `report.py` over the checked-in fixture (`tests/fixtures/pilot-kit/`: real `doctor` and `test --faults` output)
     writes a section for every guarantee a fault class exercised, leaves no placeholder, and refuses input that is
     not those outputs;
  4. the kit's README has its `bash pilot` blocks — `--blocks` prints them, in order, as the one script CI runs.
"""
import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
KIT = ROOT / "docs/operations/pilot-kit"
MATRIX = ROOT / "docs/operations/shared-responsibility-matrix.md"
CATALOGUE = ROOT / "docs/reference/guarantee-catalogue.json"
RUNBOOK = ROOT / "docs/operations/customer-pilot.md"
PROVEN = ROOT / "docs/operations/what-is-proven.md"
FIXTURE = ROOT / "tests/fixtures/pilot-kit"
TABLE = "## From a `doctor` line to its row"


def pilot_blocks(md):
    out, cur = [], None
    for line in md.splitlines():
        if cur is not None:
            if line.strip().startswith("```"):
                out.append("\n".join(cur))
                cur = None
            else:
                cur.append(line)
        elif line.strip() == "```bash pilot":
            cur = []
    return out


def main():
    if sys.argv[1:] == ["--blocks"]:
        print("\n\n".join(pilot_blocks((KIT / "README.md").read_text())))
        return 0
    fails = []

    # 1. The matrix's table against the catalogue and the matrix's own rows.
    text = MATRIX.read_text()
    ids = [g["id"] for g in json.loads(CATALOGUE.read_text())["guarantees"]]
    rows, table, inside = set(), {}, False
    for line in text.splitlines():
        if line.startswith("## "):
            inside = line.strip() == TABLE
            continue
        if not line.startswith("| ") or line.startswith("|---"):
            continue
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if inside:
            if cells[0].startswith("`"):
                gid = cells[0].strip("`")
                if gid in table:
                    fails.append(f"matrix: `{gid}` has two lines in the doctor-line table")
                table[gid] = cells[1]
        else:
            rows.add(cells[0].strip("*").strip())
    if not table:
        fails.append(f"matrix: no section `{TABLE}`")
    for gid in ids:
        if gid not in table:
            fails.append(f"matrix: the catalogue's `{gid}` has no line in the doctor-line table")
    for gid, cell in table.items():
        if gid not in ids:
            fails.append(f"matrix: `{gid}` is not in the guarantee catalogue")
        if cell.startswith("*no row*"):
            if len(cell) < len("*no row* — ") + 10:
                fails.append(f"matrix: `{gid}` reads no row without saying why")
            continue
        for part in cell.split(" · "):
            name = re.sub(r"\s*\((CC\d|Confidentiality & Availability)\)$", "", part).strip()
            if name not in rows:
                fails.append(f"matrix: `{gid}` names the row `{name}`, which the matrix does not have")

    # 2. The runbook names the kit; the proof page keeps the deployment open and says the kit is not it.
    if "pilot-kit/" not in RUNBOOK.read_text():
        fails.append("customer-pilot.md does not name the kit's directory (pilot-kit/)")
    named = [l for l in PROVEN.read_text().splitlines() if "named" in l.lower() and "deployment" in l.lower() and l.startswith("|")]
    if not any("pilot kit" in l and "not the deployment" in l for l in named):
        fails.append("what-is-proven.md: no named-deployment row saying the pilot kit is not the deployment")

    # 3. The report from the fixture.
    doctors = sorted(str(p) for p in FIXTURE.glob("doctor-*.txt"))
    faults = FIXTURE / "faults.json"
    run = subprocess.run([sys.executable, str(KIT / "report.py"), "--doctor", *doctors, "--faults", str(faults),
                          "--date", "2026-10-30"], capture_output=True, text=True)
    if run.returncode != 0:
        fails.append(f"report.py over the fixture exited {run.returncode}: {run.stderr.strip()}")
    else:
        report = run.stdout
        exercised = {e["id"] for c in json.loads(faults.read_text())["classes"]
                     for e in c.get("exercised", []) if e.get("kind") == "guarantee"}
        for gid in sorted(exercised):
            if f"### `{gid}`\n" not in report:
                fails.append(f"report: no section for `{gid}`, which a fault class exercised")
        sections = re.findall(r"^### `([^`]+)`$", report, re.M)
        for gid in sections:
            if gid not in exercised:
                fails.append(f"report: a section for `{gid}`, which no fault class exercised")
        if "{{" in report:
            fails.append("report: a placeholder is left unfilled")
        for d in doctors:
            node = pathlib.Path(d).stem.removeprefix("doctor-")
            if f"| `{node}` |" not in report:
                fails.append(f"report: the fleet table has no row for `{node}`")
    bad = subprocess.run([sys.executable, str(KIT / "report.py"), "--doctor", str(faults), "--faults", str(faults)],
                         capture_output=True, text=True)
    if bad.returncode != 2:
        fails.append(f"report.py read a file that is not `doctor` output as one (exit {bad.returncode})")

    # 4. The README's blocks.
    if len(pilot_blocks((KIT / "README.md").read_text())) < 3:
        fails.append("pilot-kit/README.md: fewer than three `bash pilot` blocks")

    for f in fails:
        print(f"check-pilot-kit: {f}")
    print(f"check-pilot-kit: {len(fails)} failure(s); {len(table)} doctor lines, {len(ids)} guarantees")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
