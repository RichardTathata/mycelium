#!/usr/bin/env python3
"""Print every jest test as `@@ts-test@@ <file>::<title>` from a `jest --json` report, skipped tests included.

The test-coverage job joins these to the `✓`/`○` lines `jest --verbose` prints, which carry only the leaf
title — so two tests in one file with the same leaf title (in different `describe`s, or a `describe.each`)
would collapse into one key, and one could be skipped forever behind the other. That is refused here.
"""
import collections
import json
import os
import sys

report = json.load(open(sys.argv[1]))
keys = [os.path.relpath(t["name"], os.getcwd()) + "::" + a["title"]
        for t in report["testResults"] for a in t["assertionResults"]]
dupes = sorted(k for k, n in collections.Counter(keys).items() if n > 1)
if dupes:
    sys.exit("jest tests sharing a file and leaf title (the coverage join cannot tell them apart; rename one):\n  "
             + "\n  ".join(dupes))
bad = [k for k in keys if "\n" in k]
if bad:
    sys.exit(f"jest test titles containing a newline: {bad}")
for k in keys:
    print("@@ts-test@@ " + k)
