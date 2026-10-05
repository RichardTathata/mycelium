## [2026-10-05] ingest | langgraph-checkpoint-mycelium 0.2.0 — an incomplete checkpoint raises

**What:** `langgraph-checkpoint-mycelium/src/.../saver.py` (`IncompleteCheckpoint`, raised by both
loaders), `__init__.py`, `tests/test_incomplete.py` (new, node-free), the README and guide 15, the
`python-sdk` CI job (a `mycelium` node and `mycelium-py`'s live gateway suite), the changelog, the
realignment plan's S5 row, and this log.

**Durable knowledge:**

- **"Present but not yet fetchable" is a third state, and both answers it used to get were wrong.**
  `None` means *no checkpoint* to LangGraph (it starts the thread over); a shorter tuple means *these
  tasks did not complete* (it re-runs them). Gossip makes the third state normal: the index row
  outruns its blobs. The only safe answer is an error the caller retries, naming what is missing.
- **The stricter path was its own hazard.** The review saw the dropped pending write; verification
  found that the "strict" handling of a missing channel blob — returning `None` — restarts a thread
  when the latest checkpoint is incomplete. Matching the dropped-write path to the `None` path would
  have spread the second bug, which is why the fix is a raise on both.
- **The flagship proved the state is real, not theoretical.** The first CI run of the change failed
  `examples/langgraph/06_deploy_reheal.py` at its convergence poll: node B saw the checkpoint row with
  three blobs not yet fetchable. Rung 03 had already worked around the old shorter tuple by waiting
  for `pending_writes` to reach a count. Both polls now catch `IncompleteCheckpoint` and retry; any
  cross-node reader must.
- **A node-free witness for a gateway client:** `httpx.MockTransport` in front of the saver's two
  clients, serving the five routes it uses with the gateway's own shapes. It runs in the existing
  pytest step with no node, and reaches the same code the live suite does.
