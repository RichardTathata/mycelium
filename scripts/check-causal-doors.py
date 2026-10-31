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
PREFLIGHT = re.compile(r"\b(ae_preflight_routed|ae_preflight_door|ae_preflight|ae_decide)\s*\(")
# Raw doors: file → symbols the registry must list when the file defines them.
SCAN = {
    # The seam's own file: its wrappers are not scanned as call sites; its two public doors are listed by symbol.
    "src/agent/http/authz.rs": ["gateway_preflight", "gateway_preflight_routed"],
    "src/agent/http/mesh.rs": ["gw_rpc_call", "gw_scatter", "gw_mailbox_deliver", "gw_shard_emit"],
    "src/agent/http/signals.rs": ["gw_signal_emit"],
    "src/agent/http/overlay.rs": ["gw_overlay_emit_reliable"],
    "src/agent/service_handle.rs": ["rpc_call", "bulk_call", "scatter_gather"],
    # Adopter-surface Y11: the take, claim and emit doors' Rust verbs, and the companions' doors (outside src/).
    "src/agent/door.rs": ["emit_in", "deliver_event_in", "emit_reliable_in", "emit_sharded_in"],
    "mycelium-tuple-space/src/lib.rs": ["take_in", "take_by_key_in"],
    "mycelium-tuple-space/src/http.rs": ["gw_take", "gw_take_by_key"],
    "mycelium-blackboard/src/lib.rs": ["claim_in"],
    "mycelium-blackboard/src/http.rs": ["gw_claim"],
}
# Adopter-surface Y11's doors (#708's review, finding 4): a call that decides a take, claim or emit door. `door::decide`
# and `door::decide_raw` anywhere; the bare names inside `src/agent/door.rs`, where they are defined; the two public
# entries a companion calls.
DOOR_CALL = re.compile(r"(\bdoor::(decide|decide_raw)|\bexposure_door(_as_node)?)\s*\(")
DOOR_CALL_HOME = re.compile(r"(?<![:\w])(decide|decide_raw)\s*\(")
DOOR_HOME = "src/agent/door.rs"
TEST_MODULE = re.compile(r"#\[cfg\((all\()?test\b")
TEST_ATTR = re.compile(r"#\[(tokio::)?test[\]\(]")


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


def is_door_call(rel, line):
    """A Y11 door call on this line — not a definition, not a comment."""
    if line.strip().startswith("//") or re.match(r"\s*(pub(\([^)]*\))?\s+)?(async\s+)?fn\s", line):
        return False
    return bool(DOOR_CALL.search(line) or (rel == DOOR_HOME and DOOR_CALL_HOME.search(line)))


def production_lines(path):
    """The file's lines up to its test module (a `#[cfg(test)]` that gates a `mod`)."""
    lines = open(path, encoding="utf-8").read().split("\n")
    for i, line in enumerate(lines):
        if TEST_MODULE.search(line):
            nxt = next((l for l in lines[i + 1:] if l.strip()), "")
            if re.match(r"\s*(pub(\([^)]*\))?\s+)?mod\s+", nxt):
                return lines[:i]
    return lines


def door_scan_files(root):
    """Every Rust file a Y11 door call can sit in: src/ and each companion crate's src/."""
    tops = [os.path.join(root, "src")] + sorted(
        os.path.join(root, d, "src") for d in os.listdir(root) if d.startswith("mycelium-") and os.path.isdir(os.path.join(root, d, "src"))
    )
    for top in tops:
        for dirpath, _, files in os.walk(top):
            for f in files:
                if f.endswith(".rs") and not f.endswith("_tests.rs") and f != "lib_tests.rs":
                    path = os.path.join(dirpath, f)
                    yield os.path.relpath(path, root), path


def fn_body(lines, sym):
    """The text of `fn sym`'s body: from its line to the closing brace at its own indentation."""
    for i, line in enumerate(lines):
        m = re.match(r"(\s*)(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+" + re.escape(sym) + r"\b", line)
        if m:
            indent = m.group(1)
            for j in range(i + 1, len(lines)):
                if lines[j] == indent + "}":
                    return "\n".join(lines[i:j + 1])
            return "\n".join(lines[i:])
    return None


def ruled_without_a_call(root, rows):
    """Each `ruled` row whose symbol neither calls a door's decision nor a same-file fn that does (one level: a
    route's helper) — the regression where a door's decision is removed and the registry still lists it."""
    failures = []
    for door, (carries, _, n) in sorted(rows.items()):
        if carries != "ruled":
            continue
        rel, _, sym = door.rpartition("::")
        path = os.path.join(root, rel)
        if not os.path.exists(path):
            continue
        lines = production_lines(path)
        body = fn_body(lines, sym)
        if body is None:
            continue
        deciders = {m.group(1) for k, l in enumerate(lines) if is_door_call(rel, l)
                    for m in [re.match(r"(.*)", enclosing_fn(lines, k) or "")] if m.group(1)}
        helpers = {d.split("::")[-1] for d in deciders}
        body_lines = body.split("\n")[1:]
        calls = any(is_door_call(rel, l) or PREFLIGHT.search(l) for l in body_lines) or any(
            re.search(r"\b" + re.escape(h) + r"\s*\(", l) for h in helpers if h != sym for l in body_lines)
        if not calls:
            failures.append(f"row {door} (line {n}) is `ruled` but its fn calls no door decision (`door::decide`, `exposure_door`, …)")
    return failures


def doors_in_tree(root):
    found = set()
    # Y11: every call that decides a take, claim or emit door is a door, in src/ and in the companions.
    for rel, path in door_scan_files(root):
        lines = production_lines(path)
        for i, line in enumerate(lines):
            if is_door_call(rel, line):
                fn = enclosing_fn(lines, i)
                if fn:
                    found.add(f"{rel}::{fn.split('::')[-1]}")
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
                # A `#[cfg(test)]` gate ends the scan only when it gates a `mod` (a test module ends a file); an inner
                # cfg(test) statement or static inside production code does not (#672's review, finding 1).
                if TEST_MODULE.search(line):
                    nxt = next((l for l in lines[i + 1:] if l.strip()), "")
                    if re.match(r"\s*(pub(\([^)]*\))?\s+)?mod\s+", nxt):
                        in_tests = True
                if in_tests:
                    break
                if PREFLIGHT.search(line) and not re.match(r"\s*(pub(\([^)]*\))?\s+)?(async\s+)?fn\s+(ae_preflight|ae_decide)", line) and not line.strip().startswith("//"):
                    fn = enclosing_fn(lines, i)
                    if fn and not fn.startswith(("ae_preflight", "ae_decide")):
                        found.add(f"{rel}::{fn}")
            for sym in SCAN.get(rel, []):
                if re.search(r"\bfn\s+" + re.escape(sym) + r"\b", "\n".join(lines)):
                    found.add(f"{rel}::{sym}")
    # The companions' doors, which sit outside src/ and are listed by symbol only.
    for rel, syms in SCAN.items():
        path = os.path.join(root, rel)
        if rel.startswith("src/") or not os.path.exists(path):
            continue
        text = open(path, encoding="utf-8").read()
        for sym in syms:
            if re.search(r"\bfn\s+" + re.escape(sym) + r"\b", text):
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
        if carries not in ("record", "record+fwd", "ruled", "bytes", "none", "refused"):
            sys.exit(f"check-causal-doors: {path}:{n}: unknown carries '{carries}'")
        rows[door] = (carries, test, n)
    return rows


def fns_in_tree(root):
    """Test functions only: a `fn` with `#[test]` or `#[tokio::test]` in the three lines above it (so a row cannot
    name a helper and pass)."""
    names = set()
    for top in ("src", "tests"):
        for dirpath, _, files in os.walk(os.path.join(root, top)):
            for f in files:
                if not f.endswith(".rs"):
                    continue
                lines = open(os.path.join(dirpath, f), encoding="utf-8").read().split("\n")
                for i, line in enumerate(lines):
                    m = re.search(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)", line)
                    if m and any(TEST_ATTR.search(l) for l in lines[max(0, i - 3):i]):
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
    failures.extend(ruled_without_a_call(root, rows))
    for door, (carries, test, n) in sorted(rows.items()):
        if test != "-" and test not in fns:
            failures.append(f"row {door} (line {n}) names test `{test}`, which is no `#[test]`/`#[tokio::test]` fn under src/ or tests/")
        if test == "-" and "#" not in raw_line(registry, n):
            failures.append(f"row {door} (line {n}) is untested without a stated reason (`-  # why`)")
    return found, rows, failures


def raw_line(path, n):
    return open(path, encoding="utf-8").read().split("\n")[n - 1]


def main():
    if "--self-test" in sys.argv:
        tmp = tempfile.mkdtemp(prefix="causal-doors-")
        try:
            shutil.copytree(os.path.join(ROOT, "src"), os.path.join(tmp, "src"))
            os.makedirs(os.path.join(tmp, "tests"))
            # Plant 1: a new file with an unregistered preflight call.
            with open(os.path.join(tmp, "src", "agent", "http", "planted_door.rs"), "w") as f:
                f.write("pub(super) async fn gw_planted() {\n    let p = ae_preflight(&ctx, None, \"x\", \"y\", &a, &b, \"planted\").await;\n}\n")
            # Plant 2: a second /mcp door appended to the existing door file, *after* its inner `#[cfg(test)]` item —
            # the place a new handler naturally goes, and the place the first scanner never looked (#672's review).
            with open(os.path.join(tmp, "src", "agent", "http", "mcp_bridge.rs"), "a") as f:
                f.write("\npub(super) async fn gw_planted_after() {\n    let p = ae_preflight(&ctx, None, \"x\", \"y\", &a, &b, \"planted\").await;\n}\n")
            # Plant 3 (Y11, #708's review, finding 4): the emit door's decision removed from its route.
            sig = os.path.join(tmp, "src", "agent", "http", "signals.rs")
            text = open(sig, encoding="utf-8").read()
            cut = text.replace("crate::agent::door::decide(", "crate::agent::door::undecided(", 1)
            assert cut != text, "the self-test's emit-door plant found nothing to remove"
            open(sig, "w", encoding="utf-8").write(cut)
            # Plant 4: an ungated route calling the companions' public door entry, in a new file.
            with open(os.path.join(tmp, "src", "agent", "http", "planted_exposure_route.rs"), "w") as f:
                f.write("pub(super) async fn gw_planted_exposure() {\n    let p = agent.exposure_door(None, \"x\", \"y\", &b, \"planted\").await;\n}\n")
            # Plant 5: a new caller of `door::decide` in src/.
            with open(os.path.join(tmp, "src", "agent", "planted_decider.rs"), "w") as f:
                f.write("pub(crate) async fn planted_decider() {\n    let p = crate::agent::door::decide(&ctx, a, \"x\", \"y\", &b, \"planted\").await;\n}\n")
            _, _, failures = check(tmp, REGISTRY)
            for want in ("planted_door.rs::gw_planted", "mcp_bridge.rs::gw_planted_after", "signals.rs::gw_signal_emit (line",
                         "planted_exposure_route.rs::gw_planted_exposure", "planted_decider.rs::planted_decider"):
                if not any(want in x for x in failures):
                    sys.exit(f"check-causal-doors --self-test: the planted door {want} was NOT caught")
            print("check-causal-doors --self-test: a planted new-file door, a door appended after an inner cfg(test), an emit "
                  "route's decision removed, an ungated `exposure_door` route and a new `door::decide` caller are all caught by name")
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
