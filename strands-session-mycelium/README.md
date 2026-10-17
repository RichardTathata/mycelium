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
pip install ./strands-session-mycelium     # from source; it brings mycelium-py and strands-agents
```

Requires Python ≥ 3.10 and a Mycelium node with an HTTP gateway (`http_port`). The package is
pinned to **`strands-agents` 1.61.1**, the release whose `SessionRepository` it implements;
`tests/test_interface.py` records that interface and fails when a release changes it, which is
when the pin moves.

## Reaching the gateway

`MyceliumSessionManager(session_id, *, agent=None, host="127.0.0.1", port=7946, token=None,
scheme="http", ca_file=None, prefix="strands", require_on_disk=False, on_receipt=None)`.

Pass an SDK client you already hold as `agent=` (a `mycelium.MyceliumAgent`), or the gateway's
address. `token`, `scheme` and `ca_file` are the SDK's: the bearer defaults to
`MYCELIUM_GATEWAY_TOKEN`, `scheme="https"` reaches a gateway serving TLS with verification on, and
`ca_file` trusts a private fleet CA. Under scoped tokens the store needs `kv:read` and `kv:write`.
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
any write the gateway node did not report `"on_disk"`; a `"failed"` one is logged either way.

## Incomplete is an error, never an empty session

Rows gossip independently, so a node can hold message 7 before message 6, or an agent's messages
before the agent. Each such read raises `IncompleteSession` (a Strands `SessionException`) with
`missing`, the keys it needs: retry once they arrive. It never returns a shorter conversation,
because Strands resumes by appending after the last message it was given, and would overwrite the
messages it did not see. A row that cannot be decoded, or a gateway error (401, 403, 5xx), raises
`SessionException` too. Only a session or agent with no row at all reads as absent.

The tail is covered as well. Each agent has a count row written after each message, so a missing
last message is detected once the count has arrived. Messages that arrive ahead of their count are
read, not dropped.

## Resuming on another node

Resume where the session's rows have arrived. "Absent" and "not here yet" look the same to a node
when **nothing** of a record has reached it. A session or agent id read there before any of its
rows arrive is new as far as Strands is concerned. Strands then creates it, and the two copies
compete, last writer wins. A deployment that moves a session between nodes waits for it (for
example, for the agent's `…/count` row to reach the expected count, as `tests/test_live.py` does), or
keeps a session on one node at a time.

## Concurrent agents

- Two agents with different ids in one session write disjoint rows. Neither loses the other's
  writes, on one node or on several, and the live suite runs exactly that across two nodes.
- **One agent id is one writer.** Two processes running the same agent id in one session overwrite
  each other's rows (last writer wins), as Strands' own managers would.
- A swarm's or graph's state is its orchestrator's alone, by the same rule.

## Layout

One KV row per record, every segment percent-encoded so an id cannot forge a `/`:

```
strands/{session}/session                     the Session
strands/{session}/agent/{agent}               the SessionAgent (state, conversation-manager state)
strands/{session}/agent/{agent}/count         {"count": n}
strands/{session}/agent/{agent}/msg/{index}   one SessionMessage (index zero-padded)
strands/{session}/multi/{id}                  a Swarm's or Graph's serialized state
```

A row is at most the KV write cap, `MAX_KV_WRITE_BYTES` (10 MiB less 64 KiB). A larger message is
refused by name before it is sent. KV rows reach every node, so this store suits conversations, not
bulk payloads. Listing an agent's messages reads one row per message.

## Tests

```sh
pytest strands-session-mycelium/tests        # the store against an in-memory gateway; the live suite skips
MYCELIUM_TEST_PORT=8101 MYCELIUM_TEST_PORT_B=8102 pytest strands-session-mycelium/tests   # + two meshed nodes
```

CI runs both against two meshed nodes. With `MYCELIUM_STRANDS_LIVE_REQUIRED` set, the live suite
fails rather than skips if the ports are missing.
