#!/usr/bin/env python3
"""Overlay integration test runner — exercises all 3 overlay scenarios."""

from __future__ import annotations

import importlib
import sys
import time
import traceback

GREEN  = "\033[0;32m"
RED    = "\033[0;31m"
BOLD   = "\033[1;34m"
RESET  = "\033[0m"

SCENARIOS = [
    ("S11 task auction (exact-once delivery)",             "s11_task_auction"),
    ("S12 leader election + consensus-durable config",      "s12_leader_election"),
    ("S13 shared reasoning log (multi-writer + ordering)", "s13_shared_reasoning_log"),
]
# Verification policy rule 3: each scenario prints `@@case@@ <suite>::<case>` as it starts (the case is its
# module's name), and --list names them on the host, without Docker or the scenarios' dependencies.
SUITE = "tests/overlay/run.py"


def banner(msg: str) -> None:
    print(f"\n{BOLD}══ {msg} ══{RESET}", flush=True)


def ok(label: str) -> None:
    print(f"  {GREEN}PASS{RESET}  {label}", flush=True)


def fail(label: str) -> None:
    print(f"  {RED}FAIL{RESET}  {label}", flush=True)


def main() -> int:
    from scenarios.helpers import ALL_HOSTS, wait_for_health, wait_for_cluster_ready

    passed = 0
    failed = 0

    # ── Phase 0: wait for all nodes to be healthy and gossip-connected ────────
    banner("Waiting for overlay cluster to be ready")
    for host in ALL_HOSTS:
        print(f"  Waiting for {host} health…", flush=True)
        wait_for_health(host, timeout=60)
    print("  All nodes healthy — waiting for gossip convergence…", flush=True)
    wait_for_cluster_ready(ALL_HOSTS, timeout=60)
    print("  Cluster converged — starting scenarios", flush=True)

    # ── Scenarios ─────────────────────────────────────────────────────────────
    banner("Running overlay scenarios")
    for label, name in SCENARIOS:
        print(f"@@case@@ {SUITE}::{name}", flush=True)
        print(f"  {label:<55}", end="", flush=True)
        try:
            importlib.import_module(f"scenarios.{name}").run()
            passed += 1
            ok(label)
        except Exception as exc:
            failed += 1
            fail(label)
            for line in traceback.format_exc().splitlines():
                print(f"    {line}", file=sys.stderr, flush=True)

    # ── Summary ───────────────────────────────────────────────────────────────
    banner("Results")
    print(f"  Passed: {passed}   Failed: {failed}\n", flush=True)
    return 0 if failed == 0 else 1


if __name__ == "__main__":
    if sys.argv[1:] == ["--list"]:
        for _, name in SCENARIOS:
            print(f"@@case-list@@ {SUITE}::{name}")
        sys.exit(0)
    sys.exit(main())
