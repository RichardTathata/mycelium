#!/usr/bin/env python3
"""Self-test for scripts/ci-test-coverage.py: synthetic job logs in each format the parser reads, and each
way a test can be known but not executed. Run: python3 scripts/test-ci-test-coverage.py (--list: its cases)"""
import importlib.util
import os
import subprocess
import sys
import tempfile

SUITE = "scripts/test-ci-test-coverage.py"
CASES = ["log-formats", "script-cases", "scripts-only-mode"]
if sys.argv[1:] == ["--list"]:
    for c in CASES:
        print(f"@@case-list@@ {SUITE}::{c}")
    sys.exit(0)

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("cov", os.path.join(HERE, "ci-test-coverage.py"))
cov = importlib.util.module_from_spec(spec)
sys.modules["cov"] = cov
spec.loader.exec_module(cov)

E = "\x1b"
LOG = f"""2026-10-06T10:11:02.6189856Z {E}[1m{E}[92m     Running{E}[0m unittests src/lib.rs (target/debug/deps/mycelium_core-470e5ebc6310f251)
2026-10-06T10:11:02.7Z test a::ran ... ok
2026-10-06T10:11:02.7Z test a::failed ... FAILED
2026-10-06T10:11:02.7Z test a::ignored ... ignored, slow
2026-10-06T10:11:03Z      Running tests/gateway.rs (target/debug/deps/gateway-0123456789abcdef)
2026-10-06T10:11:03Z test signed ... ok
2026-10-06T10:11:04Z    Doc-tests mycelium_core
2026-10-06T10:11:04Z test mycelium-core/src/hlc.rs - hlc::Hlc (line 12) ... ok
2026-10-06T10:11:05Z tests/test_x.py::test_live SKIPPED (no node)
2026-10-06T10:11:05Z tests/test_x.py::test_free[1] PASSED [ 50%]
2026-10-06T10:11:06Z PASS tests/agent.test.ts
2026-10-06T10:11:06Z   MyceliumAgent
2026-10-06T10:11:06Z     ✓ sets a key (3 ms)
2026-10-06T10:11:06Z     ○ skipped needs a node
"""
UNIVERSE = f"""2026-10-06T10:20Z @@test-universe@@ begin
2026-10-06T10:20Z      Running unittests src/lib.rs (target/debug/deps/mycelium_core-ffffffffffffffff)
2026-10-06T10:20Z a::ran: test
2026-10-06T10:20Z a::only_with_tls: test
2026-10-06T10:20Z tests/test_x.py::test_never_collected_elsewhere
2026-10-06T10:20Z @@test-universe@@ end
2026-10-06T10:21Z @@test-universe@@ begin
2026-10-06T10:21Z /home/runner/work/mycelium/mycelium/mycelium-ts/tests/agent.test.ts
2026-10-06T10:21Z /home/runner/work/mycelium/mycelium/mycelium-ts/tests/live/gateway.test.ts
2026-10-06T10:21Z @@ts-test@@ tests/agent.test.ts::sets a key
2026-10-06T10:21Z @@ts-test@@ tests/live/gateway.test.ts::reads a key from a real node
2026-10-06T10:21Z @@test-universe@@ end
"""

# A script-style suite (verification policy rule 3): `--list` prints `@@case-list@@ <suite>::<case>` inside a
# universe block; a run prints `@@case@@ <suite>::<case>` as each case starts — anywhere on the line, since a
# Docker runner's lines come prefixed with the container's name.
SCRIPT_LOG = """2026-10-08T09:00Z @@test-universe@@ begin
2026-10-08T09:00Z @@case-list@@ examples/coop/ci_smoke.sh::mailbox_llm
2026-10-08T09:00Z @@case-list@@ examples/coop/ci_smoke.sh::stigmergy
2026-10-08T09:00Z @@case@@ examples/coop/ci_smoke.sh::listed_only
2026-10-08T09:00Z @@test-universe@@ end
2026-10-08T09:01Z @@case@@ examples/coop/ci_smoke.sh::mailbox_llm
2026-10-08T09:02Z runner-1  | @@case@@ tests/integration/run.sh::01_mesh_convergence
2026-10-08T09:02Z + echo '@@case@@ examples/coop/ci_smoke.sh::traced'
"""
SCRIPT_UNIVERSE = """2026-10-08T09:03Z @@test-universe@@ begin
2026-10-08T09:03Z @@case-list@@ tests/integration/run.sh::01_mesh_convergence
2026-10-08T09:03Z @@case-list@@ tests/integration/run.sh::02_mgmt_api
2026-10-08T09:03Z @@test-universe@@ end
"""


def case(name: str) -> None:
    print(f"@@case@@ {SUITE}::{name}", flush=True)


def script_cases() -> list[str]:
    executed, universe = set(), set()
    cov.scan(SCRIPT_LOG, executed, universe)
    cov.scan(SCRIPT_UNIVERSE, executed, universe)
    s = "script examples/coop/ci_smoke.sh::"
    i = "script tests/integration/run.sh::"
    expect_executed = {s + "mailbox_llm", i + "01_mesh_convergence", s + "traced"}
    expect_not_executed = {
        s + "stigmergy",          # listed, never printed its marker
        s + "listed_only",        # a marker inside a universe block is a listing, not a run
        i + "02_mgmt_api",        # listed by a Docker suite's --list, its runner never reached it
    }
    failures = []
    if executed != expect_executed:
        failures.append(f"script executed: extra {sorted(executed - expect_executed)}, "
                        f"missing {sorted(expect_executed - executed)}")
    if universe - executed != expect_not_executed:
        failures.append(f"script not executed: got {sorted(universe - executed)}")
    return failures


def scripts_only_mode() -> list[str]:
    """The Docker workflow's mode, end to end: a listed case that never ran fails it; all run passes; and with
    no listing at all it fails rather than passing on nothing."""
    failures = []
    for name, logs, want in [
        ("one listed case never ran", [SCRIPT_LOG, SCRIPT_UNIVERSE], 1),
        ("every listed case ran", [SCRIPT_UNIVERSE,
                                   "@@case@@ tests/integration/run.sh::01_mesh_convergence\n"
                                   "@@case@@ tests/integration/run.sh::02_mgmt_api\n"], 0),
        ("nothing listed", ["@@case@@ tests/integration/run.sh::01_mesh_convergence\n"], 1),
    ]:
        with tempfile.TemporaryDirectory() as d:
            for i, text in enumerate(logs):
                with open(os.path.join(d, f"{i}.log"), "w", encoding="utf-8") as f:
                    f.write(text)
            r = subprocess.run([sys.executable, os.path.join(HERE, "ci-test-coverage.py"), "--scripts-only", d],
                               capture_output=True, text=True)
        if r.returncode != want:
            failures.append(f"--scripts-only, {name}: rc {r.returncode}, want {want}: {r.stdout.strip()[-300:]}")
        elif name == "one listed case never ran" and "examples/coop/ci_smoke.sh::stigmergy" not in r.stdout:
            failures.append(f"--scripts-only did not name the case that never ran: {r.stdout.strip()[-300:]}")
    return failures


def main() -> int:
    case("log-formats")
    executed, universe = set(), set()
    cov.scan(LOG, executed, universe)
    cov.scan(UNIVERSE, executed, universe)
    lib = "rust src/lib.rs::mycelium_core::"
    expect_executed = {
        lib + "a::ran", lib + "a::failed",
        "rust tests/gateway.rs::gateway::signed",
        "rust doc:mycelium_core::mycelium-core/src/hlc.rs - hlc::Hlc (line 12)",
        "python tests/test_x.py::test_free[1]",
        "typescript tests/agent.test.ts::sets a key",
        "typescript-file tests/agent.test.ts",
    }
    expect_not_executed = {
        lib + "a::ignored",                                   # ignored
        lib + "a::only_with_tls",                             # listed by the compiler, never run
        "python tests/test_x.py::test_live",                  # skipped everywhere
        "python tests/test_x.py::test_never_collected_elsewhere",
        "typescript tests/agent.test.ts::needs a node",       # jest skip
        "typescript-file tests/live/gateway.test.ts",         # a suite that printed nothing: listed, never ran
        "typescript tests/live/gateway.test.ts::reads a key from a real node",  # a named test, listed by --json
    }
    failures = []
    # The join that matters: a TypeScript test listed by name and run by `--verbose` counts as executed.
    if "typescript tests/agent.test.ts::sets a key" not in universe:
        failures.append("the listed TypeScript test was not read into the universe")
    if executed != expect_executed:
        failures.append(f"executed: extra {sorted(executed - expect_executed)}, missing {sorted(expect_executed - executed)}")
    if universe - executed != expect_not_executed:
        failures.append(f"not executed: got {sorted(universe - executed)}")
    case("script-cases")
    failures += script_cases()
    case("scripts-only-mode")
    failures += scripts_only_mode()
    for f in failures:
        print("FAIL", f)
    if not failures:
        print(f"test-ci-test-coverage: ok ({len(expect_executed)} executed, {len(expect_not_executed)} known and not "
              "executed; script cases: 3 executed, 3 not)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
