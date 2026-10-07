#!/usr/bin/env python3
"""Self-test for scripts/ci-test-coverage.py: synthetic job logs in each format the parser reads, and each
way a test can be known but not executed. Run: python3 scripts/test-ci-test-coverage.py"""
import importlib.util
import os
import sys

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
2026-10-06T10:21Z @@ts-test@@ tests/live/gateway.test.ts::reads a key from a real node
2026-10-06T10:21Z @@test-universe@@ end
"""


def main() -> int:
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
    if executed != expect_executed:
        failures.append(f"executed: extra {sorted(executed - expect_executed)}, missing {sorted(expect_executed - executed)}")
    if universe - executed != expect_not_executed:
        failures.append(f"not executed: got {sorted(universe - executed)}")
    for f in failures:
        print("FAIL", f)
    if not failures:
        print(f"test-ci-test-coverage: ok ({len(expect_executed)} executed, {len(expect_not_executed)} known and not executed)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
