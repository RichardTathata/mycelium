# ingest — mycelium-py: persistent pooled HTTP client (2026-08-18)

Upstream finding relayed from a downstream test session: the Python bridge opened a fresh
`httpx.Client` per call (connect→request→close), leaving a TIME_WAIT socket each time — at
Group-scale write rates (~16k rapid KV calls) this exhausts macOS ephemeral ports. Their probe
proved the same gateway endpoints healthy through a pooled keep-alive client.

Fix: `mycelium/_pool.py` — a `ClientPool` holding one persistent sync client + one async client
**per running event loop** (an `AsyncClient` is loop-bound; bridge users routinely run several
`asyncio.run()` calls against one handle, so the pool is loop-aware and abandons clients whose
loop died). All ~45 request/response call sites in `agent.py`/`wiki.py`/`tuple.py`/
`blackboard.py` now borrow from the pool via context managers that preserve each site's original
shape and per-call timeout overrides (`_Bound` injects `timeout=`). SSE stream sites stay on
dedicated clients on purpose (a stream occupies its connection for life; pooling them would
starve request traffic). New public surface: `MyceliumAgent.close()/aclose()` + sync/async
context-manager support; `aclose()` on Wiki/TupleSpace/Blackboard. `reason`/`prompt_skill`/`a2a`
already held persistent clients.

Gate: `tests/test_connection_reuse.py` — a counting keep-alive stub server (no node needed):
400 sync KV calls must ride ≤5 TCP connections; 100 async reads across two `asyncio.run()`
loops ≤6. Verified both tests FAIL on the pre-fix code (stash swap) and PASS on the fix.
mycelium-py 0.1.0 → 0.2.0 (behavior fix + new lifecycle surface; consumers pin the checkout
SHA, the version signals the change).
