"""The live suites: collected by any directory run, run only against a node.

A test here runs when MYCELIUM_TEST_HOST names a node and skips otherwise. A live CI step also sets
MYCELIUM_LIVE_REQUIRED, which turns "no node configured" into a failure instead of a green run that
skipped everything (verification policy rule 3, docs/wiki/dev/testing/verification-policy.md).
"""

import os

import pytest


def pytest_configure(config: pytest.Config) -> None:
    if os.getenv("MYCELIUM_LIVE_REQUIRED") and not os.getenv("MYCELIUM_TEST_HOST"):
        raise pytest.UsageError(
            "MYCELIUM_LIVE_REQUIRED is set but MYCELIUM_TEST_HOST is not: the live suite would skip"
        )
