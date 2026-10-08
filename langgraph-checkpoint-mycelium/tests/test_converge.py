"""The cross-node poll, node-free (issue #561). A row reaches a second node by gossip before its blobs are
fetchable there, so `get_tuple` raises `IncompleteCheckpoint` with a retriable reason (`not_found`) — that is
"not converged yet", not a failure. A non-retriable one (a corrupt blob, a refused read) still ends the wait."""
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
