"""The cross-node poll, node-free (issue #561). A second node can read a thread's row before it can fetch the row's
blobs — in #561's two CI failures because it did not yet resolve the first node as a blob provider (that node's
advertisement reached it only at the first 30 s refresh) — so `get_tuple` raises `IncompleteCheckpoint` with a retriable
reason (`not_found`): "not converged yet", not a failure. A non-retriable one (corrupt, unauthorized, unsupported, or a
mix with one) still ends the wait, and the deadline still fires."""
from types import SimpleNamespace

import pytest

from converge import converged_head
from langgraph_checkpoint_mycelium import IncompleteCheckpoint


class Stub:
    def __init__(self, answers):
        self.answers = list(answers)

    def get_tuple(self, _config):
        a = self.answers.pop(0)
        if isinstance(a, Exception):
            raise a
        return a


def head(cid, writes=0):
    return SimpleNamespace(checkpoint={"id": cid}, pending_writes=[None] * writes)


def missing(reason):
    return IncompleteCheckpoint("t", "", "c1", ["b" * 64], {"b" * 64: reason})


def test_a_row_ahead_of_its_blobs_is_not_converged_yet():
    stub = Stub([None, missing("not_found"), missing("unavailable"), head("c0"), head("c1", 1)])
    got = converged_head(stub, {}, "c1", 1, sleep=lambda _s: None)
    assert got.checkpoint["id"] == "c1" and not stub.answers


def test_a_corrupt_blob_ends_the_wait():
    stub = Stub([missing("corrupt"), head("c1")])
    with pytest.raises(IncompleteCheckpoint):
        converged_head(stub, {}, "c1", 0, sleep=lambda _s: None)


@pytest.mark.parametrize("reasons", [{"a": "unauthorized"}, {"a": "unsupported"}, {"a": "not_found", "b": "corrupt"}])
def test_any_non_transient_reason_ends_the_wait(reasons):
    stub = Stub([IncompleteCheckpoint("t", "", "c1", list(reasons), reasons), head("c1")])
    with pytest.raises(IncompleteCheckpoint):
        converged_head(stub, {}, "c1", 0, sleep=lambda _s: None)


def test_the_deadline_fires_while_misses_keep_coming():
    clock = iter(range(0, 1000, 10))
    stub = Stub([missing("not_found")] * 100)
    with pytest.raises(AssertionError, match="never converged"):
        converged_head(stub, {}, "c1", 0, timeout=30, sleep=lambda _s: None, now=lambda: next(clock))
