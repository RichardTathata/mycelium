## [2026-10-17] ingest | A Strands session store on the fleet (post-360 row O)

`strands-session-mycelium` 0.1.0, a Python companion beside the LangGraph checkpointer: `MyceliumSessionManager`
implements Strands' `SessionRepository` over `mycelium-py`'s KV verbs. Each write hands back the SDK's `KvReceipt`
(rung 1 and the gateway node's disk, not replication). A partial session raises `IncompleteSession` rather than reading
short — a per-agent count row covers the tail, and messages ahead of a lagging count are read, not overwritten. Found
while writing the live test: a node with none of an agent's rows cannot tell *not arrived* from *new*, and Strands then
creates the agent there; stated as the limit (resume where the rows have arrived), and the live test waits for them.

Pages: `dev/companions/companions.md`. Repo: `strands-session-mycelium/README.md`, `operations/companions.md`, the
plan's row O block (§3.5) and §3.6, CI's `python-sdk` job and test universe.
