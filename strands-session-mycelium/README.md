# strands-session-mycelium

A [Strands Agents](https://strandsagents.com/) session store on the
[Mycelium](https://github.com/RichardTathata/mycelium) mesh. An agent's conversation and state, and
a swarm's or graph's shared state, live in the fleet's gossiped KV rather than in a file or a
bucket, so another node of the fleet can resume a session this one wrote.

```python
from strands import Agent
from strands_session_mycelium import MyceliumSessionManager

# was: session_manager=FileSessionManager(session_id="pickups-0917")
agent = Agent(
    agent_id="planner",
    session_manager=MyceliumSessionManager("pickups-0917", host="127.0.0.1", port=8101),
)
agent("Which bakeries have surplus bread tonight?")
```

A `Swarm` or `Graph` takes the same `session_manager=`; its state is one row under the session.

## Installation

```sh
pip install ./mycelium-py ./strands-session-mycelium     # from source, both: mycelium-py is not on PyPI
```

Requires Python ≥ 3.10 and a Mycelium node with an HTTP gateway (`http_port`). The package is
pinned to **`strands-agents` 1.61.1**, the release whose `SessionRepository` it implements;
`tests/test_interface.py` records that interface and fails when a release changes it, which is
when the pin moves.

## Reaching the gateway

`MyceliumSessionManager(session_id, *, agent=None, host="127.0.0.1", port=7946, token=None,
scheme="http", ca_file=None, prefix="strands", require_on_disk=False, on_receipt=None,
claims=True, writer=None)`.

Pass an SDK client you already hold as `agent=` (a `mycelium.MyceliumAgent`), or the gateway's
address. `token`, `scheme` and `ca_file` are the SDK's: the bearer defaults to
`MYCELIUM_GATEWAY_TOKEN`, `scheme="https"` reaches a gateway serving TLS with verification on, and
`ca_file` trusts a private fleet CA. Under scoped tokens the store needs `kv:read` and `kv:write`,
and for its claims (below) `consensus:read` and `consensus:write`.
`prefix` is the first segment of every key: an application namespace (the gateway's KV routes
refuse the namespaces the substrate owns).

## What a save's receipt names

Every write returns the SDK's `KvReceipt`; the manager keeps the latest on `last_receipt`, and
`on_receipt(key, receipt)` sees each one. A receipt names **local application on the gateway node**
and, as `local_durability`, that node's own disk state (`"on_disk"`, `"buffered"`,
`"not_configured"`, `"failed"`). It does **not** name replication: nothing here waits for a peer,
and a session saved through node A is readable through node B only once its rows have gossiped
there. This is the checkpointer's wording, and the substrate's
([contracts and receipts](../docs/design/contracts-receipts.md)). `require_on_disk=True` raises on
any write the gateway node did not report `"on_disk"`; a `"failed"` one is logged either way. The
write is applied all the same, and a message refused this way is held and re-sent before the next
one (below), so on a node without `[persistence]`, which reports `"not_configured"`,
`require_on_disk=True` fails every turn: set it only against nodes that persist.

## Incomplete is an error, never an empty session

Rows gossip independently, so a node can hold message 7 before message 6, or an agent's messages
before the agent. Each such read raises `IncompleteSession` (a Strands `SessionException`) with
`missing`, the keys it needs: retry once they arrive. Returning the rows that are there would be
worse, because Strands resumes by appending after the last message it was given and would overwrite
the ones it did not see. A row that cannot be decoded, a row that is JSON but not the record its key
names (a message whose `message_id` disagrees with its key included), and a gateway error (401, 403,
5xx) each raise `SessionException`. Only a record with no row at all reads as absent.

The tail is covered once the count arrives. Each agent has a count row written after each message,
so a node that holds the count but not the last message raises. Messages that arrive ahead of their
count are read, not dropped. So is the other direction: an agent row whose conversation manager has
moved past the messages this node holds (a sliding window's removed count beyond them) raises,
rather than reading as an empty conversation that Strands would restart at message 0. An agent row
with no count yet raises too; `create_agent` writes the count first, so that row is still arriving.

A removed count *equal* to the messages here is read as an empty tail, because it is legitimate: a
sliding window of 0 removes every message each turn. Strands then numbers the next message 0 again
(its `FileSessionManager` writes it over the first message); this store moves it to the next free
index instead, and Strands carries on from there.

A write that fails is held, not skipped. Strands moves to the next message index before the store
writes the current one, so a failed write (a 503, a dropped connection, a row past the size limit)
would leave a hole that no retry fills. Instead the store keeps the message and writes it, in order,
before any later one, raising until it can. The stored session stays the conversation up to the
first message it could not write. A message that can never be written (one past the size limit)
holds every later message back and says so in each error; the session as stored is still resumable
from before it.

## What this does not establish

- **Replication.** A receipt names the gateway node's own application and disk; nothing here waits for a peer, and
  a session saved through node A is readable through node B only once its rows have gossiped there (above).
- **Tracing.** Session rows carry no correlation id or parent hash; a run that spans the fleet is not joined to the
  session through this store ([the adapter contract](../docs/reference/adapter-contract.md)).
- **The caller's identity at the gateway** is the bearer's: it rides every request the store makes
  (`test_the_bearer_rides_every_request_the_store_makes`), and the gateway resolves and records its principal; the
  store signs nothing of its own.
- **Cancellation, grants, provider-side enforcement.** A session store makes no governed call; those belong to the
  door the agent's calls go through.
- **A claim is this node's view.** Claims stop a node writing over what it has not seen; they do not make two nodes
  agree on what a session is — the claims section below says exactly what they establish.

## Claims: nothing is written over what this node has not seen

What a node reads is its own view, and a view can be behind: none of a record may have arrived, or
an older version of it. No read can tell either from the real thing. So the store does not trust its
view for writes. Before each write it **claims** what it is about to write, once, fleet-wide: a
consensus write (`consistent_set`) that a quorum decides, so no two writers ever hold one claim
however far behind either node is.

- **A message index** is claimed before its message is written. A writer refused an index (its view
  is behind the conversation, or another writer is writing the same agent) raises `StaleWriter` and is
  fenced for that agent: it writes nothing more for it, its agent state included. Re-create the agent
  where the session has arrived to carry on.
- **An agent's record, and a swarm's or graph's state**, are versioned. Creating one claims version 0;
  each update claims the version after the one the writer's object was read at. A node that has not
  seen a record another writer created raises `IncompleteSession` instead of creating it afresh, and a
  writer resumed from an older version is refused (`StaleWriter`) at its first update instead of
  writing over the newer state.
- **A session** has a generation, the decided value of its own claim; every other claim is scoped by
  it. Two nodes creating one session agree on it. A deleted session's claim stays, so its id is
  refused by name rather than reused.

**A claim names its write.** Before claiming, the writer puts the row under `pending/{token}`, and
the claim's value is the writer's id and that token; the row follows the claim and the pending copy
is removed. A reader that finds a claimed index or version whose row has not arrived reads it from the
pending copy, and writes nothing: a write from its view would carry a newer clock than whatever the
claimant wrote since (a newer version, a redaction) and bury it. So a writer that claims and then dies before writing leaves no hole
and blocks no one: whoever resumes the agent next reads that message. A retry reuses its token and is
recognised as the same write; any other write to a claimed index or version is refused, whoever
makes it. One read-side window follows from writing nothing: a node that holds a message's pending copy
but neither its row nor the copy's removal serves the message as first written, without a redaction
made since, until the row arrives. `writer` (the host's name by default; no newline) says who claimed what.

**What claims cost.** One consensus round, and one more row, per message and per agent or swarm
update (tens of milliseconds on a local mesh), and **a quorum of the fleet for every write**: while a
majority is unreachable, writes raise and are held, as any failed write is. A node that is behind
still *reads* an older conversation without error; what it can no longer do is write over the newer
one. The fleet must vote: an embedder starts `start_consensus_listener` on its nodes; the `mycelium`
binary does in the release after 2.34.0. **Voters should persist** (`[persistence]`): an acceptor
without it forgets its promises when it restarts, and a quorum counting it can then decide a second
value for a claim (`src/agent/acceptor_guard.rs`; guide 04 requires it of an electorate member). A fleet that requires a declared
electorate needs an `exclusive_default` group, or every claim is refused, which the store reports as a
configuration error rather than something to retry.

`claims=False` writes by last-writer-wins with no consensus, as a single-node or availability-first
deployment may prefer. Then the limits claims close are back: resume an agent only where its rows
have arrived in full (its `…/count` row on the new node at the count the old one wrote), one writer per
agent id, and one orchestrator per swarm or graph. The window-0 renumbering does not depend on claims.

## Concurrent agents

- Two agents with different ids in one session write disjoint rows and claims. Neither loses the
  other's writes, on one node or on several (the live suite runs it across two nodes).
- Two writers of **one** agent, or of one swarm: the first to claim each index or version writes it;
  the other gets `StaleWriter`. Nothing is written over (the live suite runs it across two nodes at
  once).
- `delete_session` while another manager is still writing the session is not supported: the writer's
  later rows outlive the session record.

## Layout

One KV row per record, every segment percent-encoded so an id cannot forge a `/`:

```
strands/{session}/session                     the Session
strands/{session}/agent/{agent}               the SessionAgent (state, conversation-manager state)
strands/{session}/agent/{agent}/count         {"count": n}
strands/{session}/agent/{agent}/msg/{index}   one SessionMessage (index zero-padded)
strands/{session}/multi/{id}                  a Swarm's or Graph's serialized state
strands/{session}/claim/session               the session's claim: its generation
strands/{session}/claim/{generation}/…        a message index's or a record version's claim
strands/{session}/pending/{token}             a claimed write's row, until the row itself is written
```

A row is at most about **1.5 MiB**. The limit is the gateway's request body (axum's default 2 MiB),
which carries the row base64-encoded; the store computes the request's size and refuses a larger row
by name before sending it (`GATEWAY_BODY_LIMIT`, `request_bytes`; the live suite writes the largest
row it allows through a real gateway). That is below the substrate's KV write cap,
`MAX_KV_WRITE_BYTES` (10 MiB less 64 KiB). KV rows reach every node, so this store suits
conversations, not bulk payloads: a message carrying a large image or document is refused, and holds
later messages back, as above. Listing an agent's messages reads one row per message.

## Tests

```sh
pytest strands-session-mycelium/tests        # the store against an in-memory gateway; the live suite skips
MYCELIUM_STRANDS_PORTS=9341,9342,9343 pytest strands-session-mycelium/tests   # + a meshed fleet that votes
```

CI runs the directory in its Python job, then again against three meshed `mycelium` nodes. With `MYCELIUM_STRANDS_LIVE_REQUIRED` set, the live suite
fails rather than skips if the ports are missing.
