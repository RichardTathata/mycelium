#!/usr/bin/env python3
"""The pilot report, from `mycelium doctor` and `mycelium test --faults` output alone.

    python3 report.py --doctor doctor/*.txt --faults faults.json > pilot-report.md

`--doctor` takes the text `doctor` prints, one file per node (`bash fleet.sh doctor <node> > doctor/<node>.txt`);
`--faults` takes `mycelium test --faults … --format json`. Each guarantee's matrix row is the one its `doctor` line
names. The template (`report-template.md`) and the guarantee catalogue (`docs/reference/guarantee-catalogue.json`,
for each promise's words) are read from beside this file in the repository: they are the vocabulary, not the run.

Exit 2 on input it cannot read: a file that is not there, one that is not `doctor` output, two outputs for one node,
or a faults file that is not `test --faults --format json`. A section is written for every guarantee a fault class
exercised, split by the state its own fleet resolved it in, and nothing is invented for one no output names.
"""
import argparse
import datetime
import json
import pathlib
import re
import sys

HERE = pathlib.Path(__file__).resolve().parent
CATALOGUE = HERE.parent.parent / "reference" / "guarantee-catalogue.json"
STATES = ("enforced", "not_configured", "not_in_build", "not_applicable", "not_verifiable_here")
# `  <id>: <state> — <detail> → <docs> → matrix: <row>` (a guarantee not enforced), and
# `  enforced → matrix: <row>: <id>, <id>` (the enforced ones, grouped by row).
LINE = re.compile(r"^  ([a-z0-9_]+\.[a-z0-9_.]+): ([a-z_]+) — (.*?)(?: → matrix: (.*))?$")
ENFORCED_ROW = re.compile(r"^  enforced → matrix: (.*): ([a-z0-9_., ]+)$")


def die(msg):
    print(f"report.py: {msg}", file=sys.stderr)
    sys.exit(2)


def fill(text, values):
    """The template's `{{name}}`s filled in one pass, so a `{{…}}` inside a value (a reason, a detail) stays as it is."""
    return re.sub(r"\{\{([a-z_]+)\}\}", lambda m: values[m.group(1)] if m.group(1) in values else m.group(0), text)


def cell(text):
    """A value safe inside a Markdown table cell."""
    return str(text).replace("|", "\\|").replace("\n", " ")


def parse_doctor(path):
    """One node's doctor text: its configuration, profile, start() verdict, every guarantee's state and row."""
    try:
        text = pathlib.Path(path).read_text(encoding="utf-8")
    except OSError as e:
        die(f"{path}: {e.strerror or e}")
    except UnicodeDecodeError as e:
        die(f"{path}: not text `doctor` printed ({e.reason} at byte {e.start})")
    node = {"file": str(path), "states": {}, "detail": {}, "row": {}}
    for line in text.splitlines():
        if line.startswith("configuration: "):
            conf = line.split(": ", 1)[1].strip()
            if conf.startswith("defaults + environment"):
                die(f"{path}: `doctor` ran without a configuration file, so it names no node")
            node["node"] = pathlib.Path(conf).stem
        elif line.startswith("profile: "):
            node["profile"] = line.split(": ", 1)[1].strip()
        elif line.startswith("start(): "):
            node["start"] = line.split(": ", 1)[1].strip()
        elif line.startswith("build features: "):
            node["features"] = line.split(": ", 1)[1].strip()
        elif line.startswith("  cluster_name = "):
            node["domain"] = line.split("=", 1)[1].split("#")[0].strip().strip('"')
        elif line.startswith("enforced: "):
            listed = line.split(": ", 1)[1].strip()
            if listed != "none":   # `doctor` prints `none` when it enforces nothing (`src/doctor.rs`)
                for gid in listed.split(","):
                    node["states"][gid.strip()] = "enforced"
        elif m := ENFORCED_ROW.match(line):
            for gid in m.group(2).split(","):
                node["row"][gid.strip()] = m.group(1)
        elif m := LINE.match(line):
            if m.group(2) not in STATES:
                die(f"{path}: `{m.group(1)}` reads `{m.group(2)}`, not a state the guarantee report uses")
            node["states"][m.group(1)] = m.group(2)
            node["detail"][m.group(1)] = m.group(3).strip()
            if m.group(4):
                node["row"][m.group(1)] = m.group(4).strip()
    for key in ("node", "profile", "start"):
        if key not in node:
            die(f"{path}: not `doctor` output (no `{key}` line)")
    if not any(line.startswith("guarantees: ") for line in text.splitlines()):
        die(f"{path}: `doctor` output names no guarantee")
    return node


def load_faults(path):
    try:
        faults = json.loads(pathlib.Path(path).read_text(encoding="utf-8"))
    except OSError as e:
        die(f"{path}: {e.strerror or e}")
    except ValueError as e:   # UnicodeDecodeError is one
        die(f"{path}: not JSON ({e})")
    if not isinstance(faults, dict) or not isinstance(faults.get("classes"), list):
        die(f"{path}: not `test --faults --format json` output (no `classes` list)")
    for c in faults["classes"]:
        if not isinstance(c, dict) or not isinstance(c.get("class"), str) or not isinstance(c.get("state"), dict) \
                or not isinstance(c["state"].get("state"), str):
            die(f"{path}: a class without its `class` and `state`")
        for key in ("checks", "exercised"):
            if not isinstance(c.get(key, []), list):
                die(f"{path}: class `{c['class']}` has a `{key}` that is not a list")
        for k in c.get("checks", []):
            if not isinstance(k, dict) or "ok" not in k or k["ok"] not in (True, False, None):
                die(f"{path}: class `{c['class']}` has a check without `ok`")
        for e in c.get("exercised", []):
            if not isinstance(e, dict) or not all(isinstance(e.get(k), str) for k in ("id", "kind", "state")):
                die(f"{path}: class `{c['class']}` lists an exercised entry without a string `id`, `kind` and `state`")
    return faults["classes"]


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--doctor", nargs="+", required=True)
    ap.add_argument("--faults", required=True)
    ap.add_argument("--date", default=datetime.date.today().isoformat())
    a = ap.parse_args()

    nodes, seen = [], {}
    for p in a.doctor:
        n = parse_doctor(p)
        if n["node"] in seen:
            die(f"{p} and {seen[n['node']]} are both `doctor` output for `{n['node']}`: one output per node")
        seen[n["node"]] = p
        nodes.append(n)
    classes = load_faults(a.faults)
    catalogue = {g["id"]: g for g in json.loads(CATALOGUE.read_text())["guarantees"]}
    template = (HERE / "report-template.md").read_text()
    head, rest = template.split("<!-- section -->\n", 1)
    section, tail = rest.split("<!-- /section -->\n", 1)

    # What each class exercised, guarantee ids only (rule ids are the rule catalogue's, not this report's).
    exercised = {}
    for c in classes:
        for e in c.get("exercised", []):
            if e["kind"] == "guarantee":
                exercised.setdefault(e["id"], []).append((c, e))
    if not exercised:
        die(f"{a.faults}: no class exercised any guarantee — nothing to report")

    def state(c):
        st = c["state"]
        why = next((st[k] for k in ("feature", "missing", "because", "reason") if st.get(k)), "")
        return f"{st['state']} — {why}" if why else st["state"]

    def row(gid):
        named = {n["row"][gid] for n in nodes if gid in n["row"]}
        return " · ".join(sorted(named)) if named else "*no `doctor` output names its row*"

    fleet = ["| Node | Domain | Profile | `start()` | Enforced | Build features |", "|---|---|---|---|---|---|"]
    for n in nodes:
        enforced = sum(1 for s in n["states"].values() if s == "enforced")
        fleet.append(f"| `{cell(n['node'])}` | {cell(n.get('domain', ''))} | {cell(n['profile'])} | {cell(n['start'])} "
                     f"| {enforced} of {len(n['states'])} | {cell(n.get('features', ''))} |")
    runs = ["| Class | Outcome | Checks held |", "|---|---|---|"]
    for c in classes:
        checks = c.get("checks", [])
        runs.append(f"| `{cell(c['class'])}` | {cell(state(c))} | {sum(1 for k in checks if k['ok'] is True)} of {len(checks)} |")

    def render(gid):
        g = catalogue.get(gid)
        per_node = []
        for n in nodes:
            s = n["states"].get(gid, "not reported")
            why = n["detail"].get(gid)
            per_node.append(f"`{n['node']}` {s}" + (f" ({why})" if why else ""))
        under = []
        for c, e in exercised[gid]:
            checks = c.get("checks", [])
            ok = sum(1 for k in checks if k["ok"] is True)
            note = f" ({e['detail']})" if e.get("detail") else ""
            under.append(f"`{c['class']}` {state(c)}, {ok} of {len(checks)} checks held; in its fleet {e['state']}{note}")
        return fill(section, {"id": gid,
                              "promise": g["promise"] if g else "*not in this repository's guarantee catalogue*",
                              "nodes": "; ".join(per_node), "faults": "; ".join(under), "matrix": row(gid)})

    enforced_ids = sorted(g for g, xs in exercised.items() if any(e["state"] == "enforced" for _, e in xs))
    other_ids = sorted(g for g in exercised if g not in enforced_ids)
    domains = sorted({n.get("domain", n["node"]) for n in nodes})
    out = fill(head, {"domains": " · ".join(domains), "date": a.date, "fleet": "\n".join(fleet),
                      "node_count": str(len(nodes)), "classes": "\n".join(runs),
                      "enforced_sections": "".join(render(g) for g in enforced_ids) or "*none*\n",
                      "other_sections": "".join(render(g) for g in other_ids) or "*none*\n"})

    reported = sorted({gid for n in nodes for gid in n["states"]})
    unex = ["| Guarantee | On each node | Who holds it |", "|---|---|---|"]
    ext = ["| Guarantee | Nodes that report it | What would show it (the doctor's words) |", "|---|---|---|"]
    for gid in reported:
        states = {n["node"]: n["states"].get(gid, "not reported") for n in nodes}
        if any(s == "not_verifiable_here" for s in states.values()):
            remedy = next((n["detail"].get(gid) for n in nodes
                           if n["states"].get(gid) == "not_verifiable_here" and n["detail"].get(gid)), "")
            where = ", ".join(f"`{k}`" for k, v in states.items() if v == "not_verifiable_here")
            ext.append(f"| `{gid}` | {where} | {cell(remedy)} |")
        elif gid not in exercised:
            unex.append(f"| `{gid}` | {cell('; '.join(f'{k} {v}' for k, v in states.items()))} | {cell(row(gid))} |")
    out += fill(tail, {"unexercised": "\n".join(unex), "external": "\n".join(ext)})
    sys.stdout.write(out)


if __name__ == "__main__":
    main()
