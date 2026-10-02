#!/usr/bin/env python3
"""Check the tutorial consumer against a real export and four corrupted copies.

Run from the repository root after first_stem_fleet has exported a snapshot:
    python3 scripts/check-stem-observations.py <export-directory>
The original export is never modified. No third-party Python packages are needed.
"""
import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile


def compare(directory):
    return subprocess.run(
        ["cargo", "run", "--quiet", "--example", "compare_stem_observations", "--", str(directory)],
        capture_output=True, text=True, check=False,
    )


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: check-stem-observations.py <export-directory>")
    source = Path(sys.argv[1])
    good = compare(source)
    expected = "matched=2, missing=0, unexpected=0, excess=0"
    if good.returncode or expected not in good.stdout:
        raise SystemExit(f"baseline failed:\n{good.stdout}\n{good.stderr}")
    observed = json.loads((source / "observed.json").read_text())
    declared = json.loads((source / "declared.json").read_text())
    print("PASS: real snapshot matches the validated declaration", flush=True)
    for case, diagnostic in [
        ("missing", "missing=1"),
        ("unexpected", "UNEXPECTED"),
        ("duplicate", "duplicate node"),
        ("schema", "mycelium.design/declaration/1"),
    ]:
        rows, doc = copy.deepcopy(observed), copy.deepcopy(declared)
        if case == "missing":
            rows = rows[:1]
        elif case == "unexpected":
            rows[0]["unit"] = "unknown-host"
        elif case == "duplicate":
            rows.append(copy.deepcopy(rows[0]))
        else:
            doc["schema"] = "invalid"
        with tempfile.TemporaryDirectory(prefix="mycelium-observation-check-") as scratch:
            target = Path(scratch)
            (target / "observed.json").write_text(json.dumps(rows))
            (target / "declared.json").write_text(json.dumps(doc))
            result = compare(target)
            if result.returncode == 0 or diagnostic not in result.stdout + result.stderr:
                raise SystemExit(f"{case} did not produce its expected refusal:\n{result.stdout}\n{result.stderr}")
        print(f"PASS: {case} refused with its expected diagnostic", flush=True)


if __name__ == "__main__":
    main()
