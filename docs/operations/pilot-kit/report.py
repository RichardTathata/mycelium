#!/usr/bin/env python3
"""The pilot report, from `mycelium doctor` and `mycelium test --faults` output alone.

    python3 report.py --doctor doctor/*.txt --faults faults.json > pilot-report.md

`--doctor` takes the text `doctor` prints, one file per node (`bash fleet.sh doctor <node> > doctor/<node>.txt`);
`--faults` takes `mycelium test --faults … --format json`. The template (`report-template.md`), the guarantee
catalogue (`docs/reference/guarantee-catalogue.json`) and the shared-responsibility matrix's doctor-line table are read
from beside this file in the repository: they are the vocabulary, not the run. Exit 2 on input it cannot read; a
section is written for every guarantee id a fault class exercised, and nothing is invented for one no output names.
"""
import argparse
import datetime
import json
import pathlib
import re
import sys

HERE = pathlib.Path(__file__).resolve().parent
CATALOGUE = HERE.parent.parent / "reference" / "guarantee-catalogue.json"
MATRIX = HERE.parent / "shared-responsibility-matrix.md"
REPO_URL = "https://github.com/RichardTathata/mycelium/blob/main"
MATRIX_URL = f"{REPO_URL}/docs/operations/shared-responsibility-matrix.md"


def die(msg):
    print(f"report.py: {msg}", file=sys.stderr)
    sys.exit(2)


def parse_doctor(path):
    """One node's doctor text: its configuration, profile, start() verdict and every guarantee's state."""
    text = pathlib.Path(path).read_text()
    node = {"file": str(path), "states": {}, "remedy": {}}
    for line in text.splitlines():
        if line.startswith("configuration: "):
            node["node"] = pathlib.Path(line.split(": ", 1)[1].strip()).stem
        elif line.startswith("profile: "):
            node["profile"] = line.split(": ", 1)[1].strip()
        elif line.startswith("start(): "):
            node["start"] = line.split(": ", 1)[1].strip()
        elif line.startswith("build features: "):
            node["features"] = line.split(": ", 1)[1].strip()
        elif line.startswith("  cluster_name = "):
            node["domain"] = line.split("=", 1)[1].split("#")[0].strip().strip('"')
        elif line.startswith("enforced: "):
            for gid in line.split(": ", 1)[1].split(","):
                node["states"][gid.strip()] = "enforced"
        else:
            m = re.match(r"^  ([a-z0-9_]+\.[a-z0-9_.]+): ([a-z_]+)(?: — (.*))?$", line)
            if m:
                node["states"][m.group(1)] = m.group(2)
                node["remedy"][m.group(1)] = (m.group(3) or "").strip()
    for key in ("node", "profile", "start"):
        if key not in node:
            die(f"{path}: not `doctor` output (no `{key}` line)")
    if not node["states"]:
        die(f"{path}: `doctor` output names no guarantee")
    return node


def matrix_rows():
    """The matrix's doctor-line table: guarantee id → the controls (rows) that cover it."""
    rows, inside = {}, False
    for line in MATRIX.read_text().splitlines():
        if line.startswith("## "):
            inside = line.startswith("## From a `doctor` line to its row")
            continue
        if inside and line.startswith("| `"):
            cells = [c.strip() for c in line.strip("|").split("|")]
            gid = cells[0].strip("`")
            # The matrix's own relative links, made absolute: the report is read outside the repository.
            rows[gid] = re.sub(r"\]\(\.\./", f"]({REPO_URL}/docs/", cells[1])
    return rows


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--doctor", nargs="+", required=True)
    ap.add_argument("--faults", required=True)
    ap.add_argument("--date", default=datetime.date.today().isoformat())
    a = ap.parse_args()

    nodes = [parse_doctor(p) for p in a.doctor]
    try:
        faults = json.loads(pathlib.Path(a.faults).read_text())
        classes = faults["classes"]
    except (OSError, ValueError, KeyError) as e:
        die(f"{a.faults}: not `test --faults --format json` output ({e})")
    catalogue = {g["id"]: g for g in json.loads(CATALOGUE.read_text())["guarantees"]}
    matrix = matrix_rows()
    template = (HERE / "report-template.md").read_text()
    head, rest = template.split("<!-- section -->\n", 1)
    section, tail = rest.split("<!-- /section -->\n", 1)

    # What each class exercised, guarantee ids only (rule ids are the rule catalogue's, not this report's).
    exercised = {}
    for c in classes:
        for e in c.get("exercised", []):
            if e.get("kind") == "guarantee":
                exercised.setdefault(e["id"], []).append((c, e))

    def state(c):
        st = c["state"]
        why = next((st[k] for k in ("feature", "missing", "because", "reason") if st.get(k)), "")
        return f"{st['state']} — {why}" if why else st["state"]

    fleet = ["| Node | Domain | Profile | `start()` | Enforced | Build features |", "|---|---|---|---|---|---|"]
    for n in nodes:
        enforced = sum(1 for s in n["states"].values() if s == "enforced")
        fleet.append(f"| `{n['node']}` | {n.get('domain', '')} | {n['profile']} | {n['start']} | {enforced} of {len(n['states'])} | {n.get('features', '')} |")
    runs = ["| Class | Outcome | Checks held |", "|---|---|---|"]
    for c in classes:
        checks = c.get("checks", [])
        runs.append(f"| `{c['class']}` | {state(c)} | {sum(1 for k in checks if k['ok'] is True)} of {len(checks)} |")

    if not exercised:
        die(f"{a.faults}: no class exercised any guarantee — nothing to report")
    domains = sorted({n.get("domain", n["node"]) for n in nodes})
    out = (head.replace("{{domains}}", " · ".join(domains)).replace("{{date}}", a.date)
           .replace("{{fleet}}", "\n".join(fleet)).replace("{{classes}}", "\n".join(runs)))

    for gid in sorted(exercised):
        g = catalogue.get(gid)
        per_node = "; ".join(f"`{n['node']}` {n['states'].get(gid, 'not reported')}" for n in nodes)
        under = []
        for c, e in exercised[gid]:
            held = c.get("checks", [])
            ok = sum(1 for k in held if k["ok"] is True)
            note = f" ({e['detail']})" if e.get("detail") else ""
            under.append(f"`{c['class']}` {state(c)}, {ok} of {len(held)} checks held; in its fleet {e['state']}{note}")
        out += (section.replace("{{id}}", gid)
                .replace("{{promise}}", g["promise"] if g else "*not in this repository's guarantee catalogue*")
                .replace("{{nodes}}", per_node)
                .replace("{{faults}}", "; ".join(under))
                .replace("{{matrix}}", f"[{matrix[gid]}]({MATRIX_URL}#from-a-doctor-line-to-its-row)" if gid in matrix
                         else "*no row of the shared-responsibility matrix names it*"))

    reported = sorted({gid for n in nodes for gid in n["states"]})
    unex = ["| Guarantee | On each node | Who holds it |", "|---|---|---|"]
    ext = ["| Guarantee | Nodes that report it | What would show it (the doctor's words) |", "|---|---|---|"]
    for gid in reported:
        states = {n["node"]: n["states"].get(gid, "not reported") for n in nodes}
        if any(s == "not_verifiable_here" for s in states.values()):
            remedy = next((n["remedy"].get(gid) for n in nodes
                           if n["states"].get(gid) == "not_verifiable_here" and n["remedy"].get(gid)), "")
            where = ", ".join(f"`{k}`" for k, v in states.items() if v == "not_verifiable_here")
            ext.append(f"| `{gid}` | {where} | {remedy} |")
        elif gid not in exercised:
            unex.append(f"| `{gid}` | {'; '.join(f'`{k}` {v}' for k, v in states.items())} | {matrix.get(gid, '—')} |")
    out += (tail.replace("{{unexercised}}", "\n".join(unex)).replace("{{external}}", "\n".join(ext)))
    sys.stdout.write(out)


if __name__ == "__main__":
    main()
