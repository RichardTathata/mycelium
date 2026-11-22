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
never in a URL, the saver's `repr` or an error message. Under scoped tokens it needs `kv:read` and
`kv:write` (the index rows) and `llm:read` and `llm:write` (the blob tier: every `put()` writes
blobs through `PUT /gateway/reason/blob`, which is `llm:write`). 0.3.2; before it the saver could not present a bearer at all, while this
README described `"unauthorized"` as a token problem.

A gateway serving HTTPS — the node's `gateway_tls` or a TLS-terminating proxy — is reached with
`scheme="https"` (0.3.2; before it `http://` was hard-coded). Certificate verification stays on; a
private fleet CA is trusted with `ca_file=` (the node-cert mode of `gateway_tls` serves the cluster
CA's `ca-cert.pem`), read at construction; an empty `ca_file` means none, like `None`. An IPv6
host is bracketed for you (`"::1"` → `http://[::1]:8101`). Both options follow the Python SDK's.

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

## What this does not establish

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
  address (one bad provider beside an honest one that lacks it is `not_found`) — or the row names an id no content
  address can be (`.`, `..`, empty, a lone surrogate, or not a string at all), which is never fetched (0.3.3); `"unsupported"` — the node
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

What the adapter contract adds to the list above ([the table](../docs/reference/adapter-contract.md)): checkpoint
rows carry **no correlation id or parent hash**, so a LangGraph run is not joined to the fleet's causal records
through this saver; a **receipt names the gateway node's rung and nothing above it** (below) — never replication;
and the saver makes no governed call, so grants and provider-side enforcement belong to the doors the graph's tools
go through.

## What the serializer trusts (0.3.5)

**The saver deserializes checkpoint rows that any admitted member can write.** A checkpoint row
(`ckpt/…`, `ckptw/…`) is an unsigned record in the gossip KV, and it names both the blob ids and the
type tag each payload is decoded with; `_assemble` (`saver.py`) hands that tag and the blob's bytes
to `serde.loads_typed`. Any admitted member can write such a row — and so can any HTTP client
holding `kv:write`, since the gateway's KV routes do not refuse these prefixes — and a blob it chose
goes in through the blob tier (`llm:write`). The precondition LangGraph's advisories treat as
post-exploitation, write access to the checkpoint store, is on a fleet simply membership.

0.3.5 raises the floor to **`langgraph-checkpoint>=4.1.1`**. Two advisories reach this path:
GHSA-wwqv-p2pp-99h5 / CVE-2025-64439 (HIGH: the `"json"` mode's `lc: 2` constructor envelope ran
arbitrary code; fixed in 3.0.0) and GHSA-fjqc-hq36-qh5p / CVE-2026-48775 (method dispatch in the JSON
reviver; fixed in 4.1.1). The same floor also clears GHSA-mhr3-j7m5-c7c9 / CVE-2026-27794 (pickle in
LangGraph's `BaseCache`, fixed in 4.0.0), which the saver does not use.

**The floor alone did not make a member-written row safe to load, so 0.3.5 changes the default.**
Two paths stayed open under LangGraph's default `JsonPlusSerializer()`:

- the `"msgpack"` mode imports and calls any type a payload names, logging `Deserializing
  unregistered type …` and going on, unless `LANGGRAPH_STRICT_MSGPACK=true` is set in the loading
  process: a payload naming `os.system` ran on 4.1.1 and 4.3.2;
- the `"json"` mode refuses an `lc: 2` constructor outside its allow-list, but an `lc: 1` constructor
  goes to langchain-core's reviver with `allowed_objects="core"`, about 146 classes: a forged row
  loaded as a real `PromptTemplate` or `RunnableSequence` (no code ran at load, but the object is
  the forger's).

When you pass no `serde`, the saver now loads with **`default_serde()`**: LangGraph's
`JsonPlusSerializer` with `allowed_msgpack_modules=None`, its strict mode whatever the variable says,
and with the `"json"` tag refused. In msgpack only LangGraph's safe set is constructed
(`SAFE_MSGPACK_TYPES` in `langgraph.checkpoint.serde._msgpack`: datetimes, UUIDs, decimals, sets,
paths, the langchain-core messages and `Document`, `Send`, `Command`, `Interrupt` and LangGraph's
other types); any other type is never called and loads as its raw arguments. A `"json"` payload
raises **`RefusedCheckpointEncoding`**, naming the remedy. Nothing the saver writes is refused:
langgraph-checkpoint 3.0 and later write only `msgpack`, `null`, `bytes` and `bytearray`. Only 2.x's
fallback after a `UnicodeEncodeError` ever wrote `"json"`, and such an old row now fails to load by
name rather than silently; pass `serde=JsonPlusSerializer(allowed_msgpack_modules=None)` to read it as
LangGraph does. Pickle stays off. `tests/test_untrusted_rows.py` writes the forged rows — a channel
value, a skeleton and a pending write naming `os.system` or `os.mkdir`, and `lc: 1` `PromptTemplate` and
`RunnableSequence` rows, through `get_tuple`, `list`, `aget_tuple` and `alist` — and asserts that
nothing runs and nothing forged is revived.

**Upgrading from 0.3.4:** a graph that checkpoints its own classes (a Pydantic or dataclass state, a
custom message type) now gets them back **as plain dicts or lists, silently**: nothing errors at load,
and the failure shows later, in a node, as an `AttributeError` or a validation error. Name them:

```python
from langgraph_checkpoint_mycelium import MyceliumCheckpointSaver, default_serde
checkpointer = MyceliumCheckpointSaver("127.0.0.1", 8101,
    serde=default_serde().with_msgpack_allowlist([MyState, ("my_app.models", "Order")]))
```

(`with_msgpack_allowlist` keeps both refusals), or set `LANGGRAPH_STRICT_MSGPACK=true`, under which
LangGraph adds a compiled graph's state-schema types to the saver's serializer itself
(`BaseCheckpointSaver.with_allowlist`, which copies `default_serde()`'s rules too).

**A `serde` you pass is used as given** — including a permissive one; the saver does not narrow it.
**Encryption does not authenticate rows.** LangGraph's `EncryptedSerializer` hands a payload whose
type tag names no cipher (`"msgpack"`, not `"msgpack+aes"`) to its inner serializer *without
decrypting*, and its default inner serializer is the permissive `JsonPlusSerializer()`: a forged
plaintext row ran code through it. Build it as `EncryptedSerializer(cipher, default_serde())`. It has
no setting that requires a cipher tag (checked in 4.1.1 and 4.3.2), so a forged plaintext row is still
decoded, by `default_serde()`'s rules.

**What remains.** The rows are still unsigned. A member can still replace a checkpoint's *content*
with any values of the allowed types: its state, which parent it names, and its pending writes —
including a forged `Send` or `Command`, which steers the graph to its own nodes with the forger's
arguments when the thread resumes. A member can also point a thread at another checkpoint, and the
saver loads that as the thread's state. The follow-up — owner-signed checkpoint rows verified at read,
as the project's [owner-signed rule](../docs/wiki/dev/owner-signed-records.md) asks of a record an
agent resumes from — is not built
([what is proven](../docs/operations/what-is-proven.md#checkpoint-rows-deserialized-unsigned)).

## What a write's receipt names (0.3.4)

Every row and blob-index write returns the route's receipt, kept as `saver.last_receipt` (a `WriteReceipt`: the
`key`, the `operation_id`, `local_durability` — `"on_disk"` · `"buffered"` · `"not_configured"` · `"failed"` with
`local_durability_error`) and handed to `on_receipt(receipt)` as each lands. `require_on_disk=True` raises
`WriteNotOnDisk` for a write the gateway node did not report `"on_disk"`; the row is applied there all the same, so
on a node without `[persistence]`, which reports `"not_configured"`, it fails every write — set it only against
nodes that persist. A gateway older than v2.16.0 answers a bare `{"ok": true}` and every field reads `None`. The
receipt is the Strands store's wording and the substrate's ([contracts and receipts](../docs/design/contracts-receipts.md)).
