#!/usr/bin/env python3
"""Self-test for scripts/ci-test-coverage.py: synthetic job logs in each format the parser reads, and each
way a test can be known but not executed. Run: python3 scripts/test-ci-test-coverage.py (--list: its cases)"""
import importlib.util
import os
import subprocess
import sys
import tempfile

SUITE = "scripts/test-ci-test-coverage.py"
CASES = ["log-formats", "script-cases", "scripts-only-mode", "named-blocks", "exceptions-scope",
         "run-group-echo", "list-script-cases"]
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


def run_cov(args: list[str], logs: list[str], exceptions: str = "") -> tuple[int, str]:
    """ci-test-coverage.py end to end over synthetic job logs, with its own exceptions file."""
    with tempfile.TemporaryDirectory() as d:
        os.mkdir(os.path.join(d, "logs"))
        for i, text in enumerate(logs):
            with open(os.path.join(d, "logs", f"{i}.log"), "w", encoding="utf-8") as f:
                f.write(text)
        exc = os.path.join(d, "exceptions.txt")
        with open(exc, "w", encoding="utf-8") as f:
            f.write(exceptions)
        r = subprocess.run([sys.executable, os.path.join(HERE, "ci-test-coverage.py"), *args, os.path.join(d, "logs")],
                           capture_output=True, text=True, env={**os.environ, "CI_TEST_COVERAGE_EXCEPTIONS": exc})
    return r.returncode, r.stdout


def expect(failures: list[str], name: str, got: tuple[int, str], want_rc: int, must_name: str = "") -> None:
    rc, out = got
    if rc != want_rc:
        failures.append(f"{name}: rc {rc}, want {want_rc}: {out.strip()[-400:]}")
    elif must_name and must_name not in out:
        failures.append(f"{name}: output does not name {must_name!r}: {out.strip()[-400:]}")


# The Docker workflow's jobs, each listing its own suite under a named block.
DOCKER_INTEGRATION = """@@test-universe@@ begin integration
@@case-list@@ tests/integration/run.sh::01_mesh_convergence
@@case-list@@ tests/integration/run.sh::02_mgmt_api
@@test-universe@@ end
mycelium-test-runner  | @@case@@ tests/integration/run.sh::01_mesh_convergence
"""
DOCKER_OVERLAY = """@@test-universe@@ begin overlay
@@case-list@@ tests/overlay/run.py::s11_task_auction
@@test-universe@@ end
runner-1  | @@case@@ tests/overlay/run.py::s11_task_auction
"""
REQUIRE_DOCKER = ["--scripts-only", "--require", "integration,overlay"]


def scripts_only_mode() -> list[str]:
    """The Docker workflow's mode, end to end: a listed case that never ran fails it, all run passes, and every
    required job's listing must be present and non-empty (M2: one listing per job, not one in total)."""
    failures = []
    ran_02 = "@@case@@ tests/integration/run.sh::02_mgmt_api\n"
    expect(failures, "--scripts-only, one listed case never ran",
           run_cov(REQUIRE_DOCKER, [DOCKER_INTEGRATION, DOCKER_OVERLAY]), 1, "tests/integration/run.sh::02_mgmt_api")
    expect(failures, "--scripts-only, every listed case ran",
           run_cov(REQUIRE_DOCKER, [DOCKER_INTEGRATION + ran_02, DOCKER_OVERLAY]), 0)
    expect(failures, "--scripts-only, one job's listing missing",
           run_cov(REQUIRE_DOCKER, [DOCKER_INTEGRATION + ran_02]), 1, "overlay")
    expect(failures, "--scripts-only, one job's listing empty",
           run_cov(REQUIRE_DOCKER, [DOCKER_INTEGRATION + ran_02,
                                    "@@test-universe@@ begin overlay\n@@test-universe@@ end\n"]), 1, "overlay")
    expect(failures, "--scripts-only without --require", run_cov(["--scripts-only"], [DOCKER_INTEGRATION + ran_02]), 1)
    return failures


# The CI workflow's three listings, named (M1): a guard that counted blocks passed with one of them missing.
FULL_RUN = """     Running unittests src/lib.rs (target/debug/deps/mycelium_core-470e5ebc6310f251)
test a::ran ... ok
PASS tests/agent.test.ts
    ✓ sets a key (3 ms)
@@case@@ scripts/test-x.sh::one
"""
FULL_RUST = """@@test-universe@@ begin rust-python
     Running unittests src/lib.rs (target/debug/deps/mycelium_core-ffffffffffffffff)
a::ran: test
@@test-universe@@ end
"""
FULL_TS = """@@test-universe@@ begin typescript
@@ts-test@@ tests/agent.test.ts::sets a key
@@test-universe@@ end
"""
FULL_SCRIPTS = """@@test-universe@@ begin scripts
@@case-list@@ scripts/test-x.sh::one
@@test-universe@@ end
"""


def named_blocks() -> list[str]:
    failures = []
    expect(failures, "every listing present", run_cov([], [FULL_RUN, FULL_RUST, FULL_TS, FULL_SCRIPTS]), 0)
    expect(failures, "the TypeScript listing missing", run_cov([], [FULL_RUN, FULL_RUST, FULL_SCRIPTS]), 1, "typescript")
    expect(failures, "the scripts listing missing", run_cov([], [FULL_RUN, FULL_RUST, FULL_TS]), 1, "scripts")
    expect(failures, "the TypeScript listing bare (old form), so not the named one",
           run_cov([], [FULL_RUN, FULL_RUST, FULL_TS.replace("begin typescript", "begin"), FULL_SCRIPTS]), 1, "typescript")
    return failures


def exceptions_scope() -> list[str]:
    """L1: one exceptions file serves both workflows. A `script` exception is judged only where its suite is
    listed; elsewhere it is neither used nor stale. And it is still stale where its suite is listed and ran."""
    failures = []
    docker_exc = "script tests/integration/run.sh::02_mgmt_api — not in this workflow\n"
    expect(failures, "a Docker suite's exception is out of scope in the full run",
           run_cov([], [FULL_RUN, FULL_RUST, FULL_TS, FULL_SCRIPTS], docker_exc), 0)
    expect(failures, "the same exception is used in the Docker run",
           run_cov(REQUIRE_DOCKER, [DOCKER_INTEGRATION, DOCKER_OVERLAY], docker_exc), 0)
    expect(failures, "a full-run script exception and a Rust one are out of scope in the Docker run",
           run_cov(REQUIRE_DOCKER, [DOCKER_INTEGRATION + "@@case@@ tests/integration/run.sh::02_mgmt_api\n",
                                    DOCKER_OVERLAY],
                   "script scripts/test-x.sh::one — elsewhere\nrust src/lib.rs::mycelium_core::a::gone — elsewhere\n"), 0)
    expect(failures, "an exception for a listed suite whose case ran is stale",
           run_cov([], [FULL_RUN, FULL_RUST, FULL_TS, FULL_SCRIPTS], "script scripts/test-x.sh::one — ran anyway\n"),
           1, "scripts/test-x.sh::one")
    return failures


def run_group_echo() -> list[str]:
    """L2: GitHub echoes a `run:` block's source between `##[group]Run …` and `##[endgroup]`. A marker written
    literally in that source is not a run of the case, and a universe mark there opens no block."""
    log = """2026-10-08T09:00Z ##[group]Run if false; then echo "@@case@@ examples/langgraph::06_deploy_reheal"; fi
2026-10-08T09:00Z echo "@@test-universe@@ begin scripts"
2026-10-08T09:00Z echo "@@case@@ examples/langgraph::05_traces"
2026-10-08T09:00Z shell: /usr/bin/bash -e {0}
2026-10-08T09:00Z ##[endgroup]
2026-10-08T09:00Z @@case@@ examples/langgraph::00_hello_skill
"""
    executed, universe = set(), set()
    before = len(cov.LISTED_BLOCKS)
    cov.scan(log, executed, universe)
    failures = []
    if executed != {"script examples/langgraph::00_hello_skill"}:
        failures.append(f"run-group echo: executed {sorted(executed)}")
    if len(cov.LISTED_BLOCKS) != before or universe != executed:
        failures.append(f"run-group echo: a universe mark in the step source opened a block ({cov.LISTED_BLOCKS[before:]}, "
                        f"{sorted(universe)})")
    return failures


def list_script_cases() -> list[str]:
    """L3: scripts/list-script-cases.py derives a suite's --list from its call sites. It reads the shell grammar
    (indentation, digits, a trailing `|| exit 1`, a continued line; not a heredoc or a comment), and a call site it
    cannot read — under `&&`/`if`, or with a variable for its case — fails the listing rather than dropping it."""
    good = """run_demo() { echo "$1"; }
run_demo "01 · a" alpha2 "x" || exit 1
  run_demo "02 · b" beta_3 "y"
run_demo "03 · c" \\
   gamma "z"
cat <<'EOT'
run_demo "nope" heredoc
EOT
# run_demo "comment" commented
echo run_demo not_a_call
"""
    failures = []

    def listing(text: str) -> tuple[int, str]:
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "suite.sh")
            with open(path, "w", encoding="utf-8") as f:
                f.write(text)
            r = subprocess.run([sys.executable, os.path.join(HERE, "list-script-cases.py"), path, "s.sh", "run_demo",
                                "{2}"], capture_output=True, text=True)
        return r.returncode, r.stdout

    rc, out = listing(good)
    want = "".join(f"@@case-list@@ s.sh::{c}\n" for c in ["alpha2", "beta_3", "gamma"])
    if rc != 0 or out != want:
        failures.append(f"list-script-cases: rc {rc}, listed {out!r}, want {want!r}")
    for name, bad in [("a call under &&", 'true && run_demo "x" delta\n'),
                      ("a call under if", 'if run_demo "x" delta; then :; fi\n'),
                      ("a variable case", 'run_demo "x" "$var"\n'),
                      ("no call at all", 'echo nothing\n')]:
        rc, out = listing(good + bad if name != "no call at all" else bad)
        if rc == 0:
            failures.append(f"list-script-cases accepted {name}: {out!r}")
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
    case("named-blocks")
    failures += named_blocks()
    case("exceptions-scope")
    failures += exceptions_scope()
    case("run-group-echo")
    failures += run_group_echo()
    case("list-script-cases")
    failures += list_script_cases()
    for f in failures:
        print("FAIL", f)
    if not failures:
        print(f"test-ci-test-coverage: ok ({len(expect_executed)} executed, {len(expect_not_executed)} known and not "
              "executed; script cases: 3 executed, 3 not)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
