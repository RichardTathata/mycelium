#!/usr/bin/env python3
"""The causal door registry's gate (adopter-surface X4; scripts/causal-doors.txt).

Finds every door in the code — a call of the preflight (`ae_preflight`, `ae_preflight_routed`, `ae_decide`) outside
its definition and outside test modules, and each raw door the SCAN list names by symbol — and fails when one has no
row in the registry, when a row names a door the code no longer has, or when a row's test is not a `fn` anywhere
under src/ or tests/. `--self-test` plants an unregistered preflight call in a copy of the tree and expects the gate
to fail on it.
"""
import os, re, shutil, subprocess, sys, tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REGISTRY = os.path.join(ROOT, "scripts", "causal-doors.txt")
PREFLIGHT = re.compile(r"\b(ae_preflight_routed|ae_preflight|ae_decide)\s*\(")
# Raw doors: file → symbols the registry must list when the file defines them.
SCAN = {
    # The seam's own file: its wrappers are not scanned as call sites; its two public doors are listed by symbol.
    "src/agent/http/authz.rs": ["gateway_preflight", "gateway_preflight_routed"],
    "src/agent/http/mesh.rs": ["gw_rpc_call", "gw_scatter", "gw_mailbox_deliver", "gw_shard_emit"],
    "src/agent/http/signals.rs": ["gw_signal_emit"],
    "src/agent/http/overlay.rs": ["gw_overlay_emit_reliable"],
    "src/agent/service_handle.rs": ["rpc_call", "bulk_call", "scatter_gather"],
}
TEST_MODULE = re.compile(r"#\[cfg\((all\()?test\b")


def enclosing_fn(lines, i):
    """The nearest `fn` above line i, as `Type::fn` when it sits inside an `impl` block that is still open there."""
    fn, ty = None, None
    for j in range(i, -1, -1):
        m = re.match(r"\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)", lines[j])
        if m and fn is None:
            fn = m.group(1)
            continue
        if fn is None:
            continue
        # A `}` at column 0 above the fn closes whatever block came before it: the fn is free.
        if lines[j].startswith("}"):
            break
        m = re.match(r"^impl(?:<[^>]*>)?\s+(?:[A-Za-z_][A-Za-z0-9_:<>]*\s+for\s+)?([A-Za-z_][A-Za-z0-9_:]*)", lines[j])
        if m:
            ty = m.group(1).split("::")[-1]
            break
        if re.match(r"^(pub\s+)?(mod|struct|enum)\b", lines[j]):
            break
    if fn is None:
        return None
    return f"{ty}::{fn}" if ty else fn


def doors_in_tree(root):
    found = set()
    src = os.path.join(root, "src")
    for dirpath, _, files in os.walk(src):
        for f in files:
            if not f.endswith(".rs") or f.endswith("_tests.rs") or f == "lib_tests.rs":
                continue
            path = os.path.join(dirpath, f)
            rel = os.path.relpath(path, root)
            lines = open(path, encoding="utf-8").read().split("\n")
            in_tests = rel == "src/agent/http/authz.rs"  # the seam's own wrappers are not doors
            for i, line in enumerate(lines):
                if TEST_MODULE.search(line):
                    in_tests = True  # everything after the test module's gate is test code (modules end the file)
                if in_tests:
                    break
                if PREFLIGHT.search(line) and not re.match(r"\s*(pub(\([^)]*\))?\s+)?(async\s+)?fn\s+(ae_preflight|ae_decide)", line) and not line.strip().startswith("//"):
                    fn = enclosing_fn(lines, i)
                    if fn and not fn.startswith(("ae_preflight", "ae_decide")):
                        found.add(f"{rel}::{fn}")
            for sym in SCAN.get(rel, []):
                if re.search(r"\bfn\s+" + re.escape(sym) + r"\b", "\n".join(lines)):
                    found.add(f"{rel}::{sym}")
    return found


def registry_rows(path):
    rows = {}
    for n, raw in enumerate(open(path, encoding="utf-8"), 1):
        line = raw.split("#", 1)[0].strip() if not raw.lstrip().startswith("#") else ""
        if not line:
            continue
        parts = [p.strip() for p in raw.split("#", 1)[0].split("|")]
        if len(parts) != 3:
            sys.exit(f"check-causal-doors: {path}:{n}: a row has {len(parts)} columns, not 3")
        door, carries, test = parts
        if carries not in ("record", "record+fwd", "bytes", "none", "refused"):
            sys.exit(f"check-causal-doors: {path}:{n}: unknown carries '{carries}'")
        rows[door] = (carries, test, n)
    return rows


def fns_in_tree(root):
    names = set()
    for top in ("src", "tests"):
        for dirpath, _, files in os.walk(os.path.join(root, top)):
            for f in files:
                if f.endswith(".rs"):
                    for m in re.finditer(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)", open(os.path.join(dirpath, f), encoding="utf-8").read()):
                        names.add(m.group(1))
    return names


def check(root, registry):
    found = doors_in_tree(root)
    rows = registry_rows(registry)
    fns = fns_in_tree(root)
    failures = []
    for door in sorted(found - set(rows)):
        failures.append(f"door without a row: {door} — add it to scripts/causal-doors.txt with how the fields travel and the test")
    for door in sorted(set(rows) - found):
        failures.append(f"row without a door: {door} (registry line {rows[door][2]}) — the code no longer has it")
    for door, (carries, test, n) in sorted(rows.items()):
        if test != "-" and test not in fns:
            failures.append(f"row {door} (line {n}) names test `{test}`, which is no `fn` under src/ or tests/")
    return found, rows, failures


def main():
    if "--self-test" in sys.argv:
        tmp = tempfile.mkdtemp(prefix="causal-doors-")
        try:
            shutil.copytree(os.path.join(ROOT, "src"), os.path.join(tmp, "src"))
            os.makedirs(os.path.join(tmp, "tests"))
            with open(os.path.join(tmp, "src", "agent", "http", "planted_door.rs"), "w") as f:
                f.write("pub(super) async fn gw_planted() {\n    let p = ae_preflight(&ctx, None, \"x\", \"y\", &a, &b, \"planted\").await;\n}\n")
            _, _, failures = check(tmp, REGISTRY)
            planted = [x for x in failures if "planted_door.rs::gw_planted" in x]
            if not planted:
                sys.exit("check-causal-doors --self-test: a planted unregistered door was NOT caught")
            print("check-causal-doors --self-test: a planted unregistered door is caught by name")
            return 0
        finally:
            shutil.rmtree(tmp, ignore_errors=True)
    found, rows, failures = check(ROOT, REGISTRY)
    for f in failures:
        print(f"check-causal-doors: {f}")
    tested = sum(1 for c, t, _ in rows.values() if t != "-")
    print(f"check-causal-doors: {len(found)} door(s) in the code, {len(rows)} row(s), {tested} with a test, {len(rows) - tested} stated untested; {len(failures)} failure(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
