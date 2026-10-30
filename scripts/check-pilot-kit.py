#!/usr/bin/env python3
"""The pilot kit's gate (adopter-surface Y10), in `make check`.

Four things the kit promises that nothing else holds:
  1. the shared-responsibility matrix's doctor-line table covers the guarantee catalogue — every id once, no id the
     catalogue lacks, every row it names a row of the section it names (or `*no row*` with a reason) — and equals
     the table `doctor` prints from (`src/doctor.rs` MATRIX_ROWS), so a doctor line and the page cannot drift;
  2. the pilot runbook names the kit's directory, and the proof page keeps the named-deployment line open while saying
     the kit is not the deployment;
  3. `report.py` over the checked-in fixture (`tests/fixtures/pilot-kit/`: real `doctor` and `test --faults` output)
     writes a section for every guarantee a fault class exercised, leaves no placeholder, and refuses input that is
     not those outputs;
  4. the kit's README has its `bash pilot` blocks, each sourcing `env.sh` first — `--blocks-dir DIR` writes them
     there, one script each, which CI runs in order, each in a fresh shell.
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
DOCTOR = ROOT / "src/doctor.rs"
# The matrix's control sections, by the heading each begins with, and the suffix a row named from one carries.
SECTIONS = {"CC6": "CC6", "CC7": "CC7", "CC8": "CC8", "Confidentiality & Availability": "Confidentiality & Availability"}


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


def report(*args):
    return subprocess.run([sys.executable, str(KIT / "report.py"), *map(str, args), "--date", "2026-10-30"],
                          capture_output=True, text=True)


def report_cases(doctors, faults):
    """The report's edges: input it must refuse (exit 2, never a traceback), a node that enforces nothing, and a
    value that would break a Markdown table."""
    import tempfile
    fails = []
    tmp = pathlib.Path(tempfile.mkdtemp(prefix="check-pilot-kit-"))
    data = json.loads(faults.read_text())

    def write(name, body):
        p = tmp / name
        p.write_text(body if isinstance(body, str) else json.dumps(body))
        return p

    no_state = json.loads(json.dumps(data)); no_state["classes"][0].pop("state")
    no_ok = json.loads(json.dumps(data)); no_ok["classes"][0]["checks"][0].pop("ok")
    same_stem = tmp / "elsewhere"; same_stem.mkdir()
    (same_stem / "doctor-x.txt").write_text(pathlib.Path(doctors[0]).read_text())
    no_file = pathlib.Path(doctors[0]).read_text().replace(
        next(l for l in pathlib.Path(doctors[0]).read_text().splitlines() if l.startswith("configuration: ")),
        "configuration: defaults + environment (no file)")
    refused = {
        "a doctor file that is not there": ["--doctor", tmp / "absent.txt", "--faults", faults],
        "a doctor file that is not doctor output": ["--doctor", faults, "--faults", faults],
        "a faults file that is not there": ["--doctor", *doctors, "--faults", tmp / "absent.json"],
        "a faults file that is not an object": ["--doctor", *doctors, "--faults", write("list.json", [1, 2])],
        "a class without its state": ["--doctor", *doctors, "--faults", write("no-state.json", no_state)],
        "a check without ok": ["--doctor", *doctors, "--faults", write("no-ok.json", no_ok)],
        "one doctor file given twice": ["--doctor", doctors[0], doctors[0], "--faults", faults],
        "two configurations with the same stem": ["--doctor", doctors[0], same_stem / "doctor-x.txt", "--faults", faults],
        "doctor run without a configuration file": ["--doctor", write("no-file.txt", no_file), "--faults", faults],
    }
    for what, args in refused.items():
        r = report(*args)
        if r.returncode != 2 or "Traceback" in r.stderr:
            fails.append(f"report.py over {what}: exit {r.returncode}, not a refusal (2){' with a traceback' if 'Traceback' in r.stderr else ''}")

    # A node that enforces nothing (`enforced: none`, real `doctor` output): no guarantee named `none`, 0 enforced.
    bare = FIXTURE / "cases" / "doctor-bare.txt"
    r = report("--doctor", *doctors, bare, "--faults", faults)
    if r.returncode != 0:
        fails.append(f"report.py with a node that enforces nothing exited {r.returncode}: {r.stderr.strip()}")
    else:
        if "`none`" in r.stdout:
            fails.append("report: `enforced: none` read as a guarantee named `none`")
        if not re.search(r"^\| `bare` \|.*\| 0 of \d+ \|", r.stdout, re.M):
            fails.append("report: the node that enforces nothing is not counted 0 enforced")

    # A `|` in a doctor reason stays inside its cell.
    src = next(d for d in doctors if "kitchen-1" in d)
    piped = pathlib.Path(src).read_text().replace("a node cannot verify its own clock", "a node | cannot verify its own clock")
    pd = tmp / "piped"; pd.mkdir()
    # Given first, so its reason is the one the table quotes.
    r = report("--doctor", write("piped/doctor-kitchen-1.txt", piped), *[d for d in doctors if d != src], "--faults", faults)
    if r.returncode != 0 or "a node \\| cannot verify" not in r.stdout:
        fails.append("report: a `|` in a doctor reason is not escaped inside its table cell")
    return fails


def main():
    if len(sys.argv) == 3 and sys.argv[1] == "--blocks-dir":
        # Each block as its own script, so CI runs each in a fresh shell: a block that leans on a variable an
        # earlier block set fails there, as it would for a reader in a new terminal.
        out = pathlib.Path(sys.argv[2])
        out.mkdir(parents=True, exist_ok=True)
        for i, block in enumerate(pilot_blocks((KIT / "README.md").read_text()), 1):
            (out / f"block-{i}.sh").write_text(block + "\n")
            print(out / f"block-{i}.sh")
        return 0
    fails = []

    # 1. The matrix's table against the catalogue, the matrix's own rows, and `doctor`'s copy.
    text = MATRIX.read_text()
    ids = [g["id"] for g in json.loads(CATALOGUE.read_text())["guarantees"]]
    rows, table, section = {}, {}, None   # rows: (section suffix, row name)
    for line in text.splitlines():
        if line.startswith("## "):
            head = line[3:].strip()
            section = ("table" if head == TABLE[3:] else
                       next((k for k in SECTIONS if head.startswith(k)), None))
            continue
        if section is None or not line.startswith("| ") or line.startswith("|---"):
            continue
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if section == "table":
            if cells[0].startswith("`"):
                gid = cells[0].strip("`")
                if gid in table:
                    fails.append(f"matrix: `{gid}` has two lines in the doctor-line table")
                table[gid] = cells[1]
        elif cells[0] != "Control":   # a control table's header is not a row
            rows.setdefault(SECTIONS[section], set()).add(cells[0].strip("*").strip())
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
            m = re.match(r"^(.*) \((CC\d|Confidentiality & Availability)\)$", part.strip())
            if not m:
                fails.append(f"matrix: `{gid}` names `{part}` without its section, as `Row (CC6)`")
            elif m.group(1) not in rows.get(m.group(2), set()):
                fails.append(f"matrix: `{gid}` names the row `{m.group(1)}` in {m.group(2)}, which that section does not have")
    # `doctor` prints the same table (`src/doctor.rs` MATRIX_ROWS): equal to the page's, cell for cell, links and
    # emphasis stripped, so the row a doctor line names is the row an auditor reads here.
    plain = {g: re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", c).replace("*", "") for g, c in table.items()}
    code = dict(re.findall(r'^    \("([^"]+)", "([^"]+)"\),$', DOCTOR.read_text(), re.M))
    if not code:
        fails.append("src/doctor.rs: no MATRIX_ROWS entries found")
    for gid in sorted(set(plain) | set(code)):
        if plain.get(gid) != code.get(gid):
            fails.append(f"doctor and the matrix disagree on `{gid}`: doctor {code.get(gid)!r}, matrix {plain.get(gid)!r}")

    # 2. The runbook names the kit; the proof page keeps the deployment open and says the kit is not it.
    if "](pilot-kit/README.md)" not in RUNBOOK.read_text():
        fails.append("customer-pilot.md does not link the kit (`](pilot-kit/README.md)`)")
    named = [l for l in PROVEN.read_text().splitlines() if "named" in l.lower() and "deployment" in l.lower() and l.startswith("|")]
    if not any("pilot kit" in l and "not the deployment" in l for l in named):
        fails.append("what-is-proven.md: no named-deployment row saying the pilot kit is not the deployment")

    # 3. The report from the fixture.
    doctors = sorted(str(p) for p in FIXTURE.glob("doctor-*.txt"))
    if len(doctors) != 4:
        fails.append(f"tests/fixtures/pilot-kit: {len(doctors)} doctor outputs, not the scaffold's four")
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
        # The split: a section under the enforced heading only for a guarantee some class's fleet enforced.
        enforced_in_fleet = {e["id"] for c in json.loads(faults.read_text())["classes"]
                             for e in c.get("exercised", []) if e.get("kind") == "guarantee" and e["state"] == "enforced"}
        head, _, rest = report.partition("## Guarantees a class touched where its fleet did not enforce them")
        first = set(re.findall(r"^### `([^`]+)`$", head, re.M))
        second = set(re.findall(r"^### `([^`]+)`$", rest, re.M))
        if not rest or first != enforced_in_fleet or second != exercised - enforced_in_fleet:
            fails.append(f"report: the sections are not split by the state the class's fleet resolved them in "
                         f"(enforced {sorted(first)}, the rest {sorted(second)})")
        if "feeds nothing into this report" not in report:
            fails.append("report: it does not say the cross-edge call feeds nothing into it")
        for d in doctors:
            node = pathlib.Path(d).stem.removeprefix("doctor-")
            if f"| `{node}` |" not in report:
                fails.append(f"report: the fleet table has no row for `{node}`")
    fails += report_cases(doctors, faults)

    # 4. The README's blocks.
    blocks = pilot_blocks((KIT / "README.md").read_text())
    if len(blocks) < 3:
        fails.append("pilot-kit/README.md: fewer than three `bash pilot` blocks")
    for i, block in enumerate(blocks, 1):
        if not block.lstrip().startswith('. "$MYCELIUM_REPO/docs/operations/pilot-kit/env.sh"'):
            fails.append(f"pilot-kit/README.md: `bash pilot` block {i} does not source env.sh first")

    for f in fails:
        print(f"check-pilot-kit: {f}")
    print(f"check-pilot-kit: {len(fails)} failure(s); {len(table)} doctor lines, {len(ids)} guarantees")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
