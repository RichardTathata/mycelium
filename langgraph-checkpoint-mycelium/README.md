# langgraph-checkpoint-mycelium

A [LangGraph](https://langchain-ai.github.io/langgraph/) checkpointer backed by the
[Mycelium](https://github.com/RichardTathata/mycelium) mesh. LangGraph runs **on**
Mycelium: graph state becomes coordinator-free, gossip-replicated, and resumable
across nodes — kill the node a thread was running on and any other node in the mesh
can pick it up.

## The one-line swap

```python
from langgraph_checkpoint_mycelium import MyceliumCheckpointSaver

# was: checkpointer = InMemorySaver()  /  PostgresSaver(...)
checkpointer = MyceliumCheckpointSaver("127.0.0.1", 8101)

graph = builder.compile(checkpointer=checkpointer)
graph.invoke(inputs, {"configurable": {"thread_id": "my-thread"}})
```

Both the sync and async LangGraph paths are supported (`get_tuple`/`list`/`put`/
`put_writes`/`delete_thread` and their `a*` variants, over `httpx.Client` /
`httpx.AsyncClient`). The `a*` variants never touch the sync client — including
`alist`'s key enumeration and per-row reads (fixed in 0.1.1; before that, `alist`
selected rows synchronously and blocked the event loop on large histories or a
slow gateway).

## Installation

```sh
pip install langgraph-checkpoint-mycelium     # PyPI (when published)

# Or from source:
pip install ./langgraph-checkpoint-mycelium
```

Requires Python ≥ 3.10 and a running Mycelium node whose gateway mounts the
`mycelium-reason` routes (see the `reason_node` example in that crate).

## Reaching the gateway

`MyceliumCheckpointSaver(host, port, *, timeout=30.0, serde=None, token=None, scheme="http", ca_file=None)`.

A token-protected gateway (`gateway_auth_token`, scoped tokens or OIDC — every `/gateway/*` route
answers 401 without a bearer) is reached with `token=`, sent as `Authorization: Bearer` on both the
sync and async clients; when omitted, `MYCELIUM_GATEWAY_TOKEN` is used, exactly as the Python SDK
resolves it (the argument wins; an empty string means none). The token travels in the header only —
never in a URL, the saver's `repr` or an error message. Under scoped tokens it needs `kv:read`,
`kv:write` and `llm:read`. 0.3.2; before it the saver could not present a bearer at all, while this
README described `"unauthorized"` as a token problem.

A gateway serving HTTPS — the node's `gateway_tls` or a TLS-terminating proxy — is reached with
`scheme="https"` (0.3.2; before it `http://` was hard-coded). Certificate verification stays on; a
private fleet CA is trusted with `ca_file=` (the node-cert mode of `gateway_tls` serves the cluster
CA's `ca-cert.pem`), read at construction. Both options follow the Python SDK's.

```python
checkpointer = MyceliumCheckpointSaver("10.0.0.5", 8101, token="…")                      # or MYCELIUM_GATEWAY_TOKEN
checkpointer = MyceliumCheckpointSaver("10.0.0.5", 9443, scheme="https", ca_file="mycelium-tls/ca-cert.pem")
```

## The storage split

Naïve checkpoint-blobs-in-KV would flood every node with every agent's channel
state; Mycelium's KV is also size-gated. So the saver splits storage the way the
substrate wants (`docs/plans/mycelium-reason.md`):

- **Metadata / index → gossiped KV** (small rows only): one row per checkpoint at
  `ckpt/{thread_id}/{checkpoint_ns}/{checkpoint_id}`, one per pending write at
  `ckptw/{thread_id}/{checkpoint_ns}/{checkpoint_id}/{task_id}/{idx}`. Checkpoint
  metadata (source / step / parents) stays inline in the row, so `list()`
  filtering never fetches a payload. The empty namespace (LangGraph's default
  `checkpoint_ns=""`) is encoded as the sentinel segment `__root__`.
- **Payloads → the content-addressed blob tier** (`PUT/GET /gateway/reason/blob`):
  the checkpoint skeleton is one blob and **each channel value is its own blob**.
  A blob's id is its SHA-256, so an unchanged channel value across super-steps
  dedups to a single stored blob — chatty graphs don't pay for their transcripts
  twice.

## Cross-node resume (the point)

A saver pointed at node **B**'s gateway resumes a thread checkpointed via node
**A**: the index rows arrive by gossip, and payload blobs are fetched through the
gateway's local-then-mesh path (`reason/blob-cache` providers, hash-verified).
No coordinator, no shared database — the mesh *is* the checkpoint store.

## Honest limits (v1)

- **≤ 8 MiB per blob** — the single-frame mesh-fetch ceiling; chunked transfer is
  the named follow-up in `mycelium-reason`.
- **Gossip-eventual metadata** — read-your-writes holds only against the *same*
  node's gateway. A cross-node reader polls until the thread head has gossiped in
  (the test suite shows the structural convergence loop).
- **An incomplete checkpoint raises `IncompleteCheckpoint`** (0.2.0). The index row can
  gossip in before every blob it references is fetchable. Before 0.2.0 the reader
  answered that two wrong ways: a missing pending-write blob was **dropped** (the tuple
  came back with fewer `pending_writes`, so LangGraph re-ran a task that had already
  completed), and a missing skeleton or channel blob returned **`None`**, which LangGraph
  reads as *no checkpoint* and starts the thread over. Now `get_tuple`, `aget_tuple`,
  `list` and `alist` raise `IncompleteCheckpoint`, naming the missing blob ids and why
  each is missing — and `None` means only that no checkpoint row exists. It is **retriable
  only when every reason is transient**: `not_found` (the blob route's 404) or `unavailable`
  (503, or any other 5xx, 408 or 429 — a proxy's 502 page included). `corrupt` (a 502 whose
  body is the route's own `corrupt`),
  `unauthorized` (401/403), `refused` (the route's 403) and `unsupported` (a bare 404 — no reason companion) are not:
  `e.retriable` is false and waiting will not fix them. Catch it and retry a retriable one
  after a short wait, with a bound; do not treat it as "start fresh". An application meets it from
  `graph.invoke(…)` / `graph.get_state(…)` on a node that has not converged yet:

  ```python
  import time
  from langgraph_checkpoint_mycelium import IncompleteCheckpoint

  for attempt in range(240):                     # ~60 s — CI has seen a provider become visible only at its
                                                 # first 30 s capability refresh (#563, root cause open);
                                                 # the examples and this package's tests also wait 60 s
      try:
          result = graph.invoke(None, config)    # or graph.get_state(config)
          break
      except IncompleteCheckpoint as e:          # e.missing: the blob ids; e.reasons: why each
          if not e.retriable:                    # refused or corrupt: waiting will not fix it
              raise
          time.sleep(0.25)
  else:
      raise RuntimeError("checkpoint still incomplete — see below")
  ```

  **Why each blob is missing** is `e.reasons[blob_id]` (0.3.0, with `mycelium-reason` 0.7.0):
  `"not_found"` — no reachable holder has it yet; `"unavailable"` — a holder could not be reached or refused for now (`mycelium-reason` 0.7.1+), the
  node or a proxy in front of it failed, the read was throttled, or the node itself could not be reached
  (an `httpx.TransportError`, such as a refused connection); `"unauthorized"` — the gateway refused
  the token (a 401/403 — no `llm:read`, or no bearer at all: pass `token=` or set
  `MYCELIUM_GATEWAY_TOKEN`, 0.3.2); `"refused"` (0.3.1) — the blob route's own 403: every holder refused for
  good (a removed member, a denied action; `mycelium-reason` 0.7.1) — fix the holder's membership or authority,
  not the token; `"corrupt"` — every copy currently on offer fails the content
  address (one bad provider beside an honest one that lacks it is `not_found`); `"unsupported"` — the node
  does not serve the blob route (no reason companion there). `e.retriable`
  is true only when every reason is `not_found` or `unavailable`, so the loop above should re-raise
  on `not e.retriable` rather than wait. A checkpoint that stays incomplete past your bound with
  only transient reasons means no reachable `reason`/`blob-cache` provider holds it — escalate; do
  not start the thread fresh (operations: `docs/operations/companions.md` § mycelium-reason).

  **Upgrade order:** upgrade this package to **0.3.0 before** upgrading the reason nodes to
  `mycelium-reason` **0.7.0**. A 0.2.x checkpointer reads only a 404 as a missing blob, so the
  503 (`unavailable`) or 502 (`corrupt`) a 0.7.0 node answers escapes its retry loop as an
  `httpx.HTTPStatusError`.
- **`put()` returns a rung-1 receipt** — the index row was *applied* to the store of
  the node you are talking to (`_kv_set` → `POST /gateway/kv`). It does **not** say
  the row crossed that node's persistence barrier, and it says nothing about any
  other node. A checkpoint `put()` acknowledged is therefore not yet one that
  survives losing that node. Where that matters — before an irreversible external
  effect, or before handing a thread to another node — ask for more: `set_with_min_acks`
  on the head key, or read the head back from the node that will resume it, which is
  what `examples/langgraph/06_deploy_reheal.py` does before it kills node A. The
  vocabulary is [guide 18](../docs/guide/18-contracts-and-receipts.md): *a receipt
  names its rung and nothing above it.*
- **`delete_thread` tombstones index rows only** — content-addressed blobs may be
  shared across threads; unreferenced blobs are a GC concern, not a correctness
  one.
- The package claims the **`ckpt/*` and `ckptw/*` KV prefixes** — treat them as
  reserved next to the substrate's own (`docs/guide/building-on-mycelium.md`).

## Where this sits — the Mycelium × LangChain/LangGraph integration map

One coherent story, four touchpoints (anti-scatter — these are different layers,
not competing examples):

| Touchpoint | Direction | Use when |
|---|---|---|
| `examples/a2a_langchain/` (A2A interop) | LangChain → Mycelium | a LangChain/AutoGen agent should *call Mycelium skills* as tools |
| **this package** (state backend) | LangGraph **on** Mycelium | a LangGraph graph should *survive node loss and hand off across the fleet* |
| `mycelium-reason` (Tier-3 wedges) | substrate-native | you want capability-routed inference, fleet-reasoning traces, artifact-aware resume |
| `mycelium.call_typed` (typed output) | through-the-mesh calls | you want schema-validated output from a *mesh-routed* prompt skill (use Instructor / Pydantic AI when talking to a provider directly) |
