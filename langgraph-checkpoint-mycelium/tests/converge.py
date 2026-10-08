"""Waiting for a second node to converge on a thread head — the poll a cross-node test needs (issue #561)."""
import time

from langgraph_checkpoint_mycelium import IncompleteCheckpoint


def converged_head(saver, config, expected_id, expected_writes, timeout=60.0, sleep=time.sleep, now=time.monotonic):
    """The head `saver`'s node reads for `config` once it is `expected_id` with at least `expected_writes`
    pending writes — bounded, no fixed sleeps."""
    deadline = now() + timeout
    while True:
        try:
            head = saver.get_tuple(config)
        except IncompleteCheckpoint as e:
            # The row gossips ahead of its blobs: `not_found` / `unavailable` here means not yet. A corrupt blob or a
            # refused read will not converge by waiting.
            if not e.retriable:
                raise
            head = None
        if head is not None and head.checkpoint["id"] == expected_id and len(head.pending_writes or []) >= expected_writes:
            return head
        assert now() < deadline, "node B never converged on the thread head"
        sleep(0.25)
