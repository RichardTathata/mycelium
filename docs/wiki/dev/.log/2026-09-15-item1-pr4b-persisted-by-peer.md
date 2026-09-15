## [2026-09-15] ingest | item 1 PR 4b — the origin asks, because it can never be told

Up: [dev](../dev.md) · page [runtime-invariants](../architecture/runtime-invariants.md) §*A node can
never observe its own write's propagation* · record `docs/design/contracts-receipts.md` §1a, §2.1, §8 ·
plan §10.12.15 · code `src/agent/replica_sync.rs`, `kv_quorum_ext.rs`, `mycelium-core/src/persistence.rs`.

**The constraint PR 4a left.** No predicate over inbound updates can acknowledge a write, because the
evidence is not in that stream: an update's `sender` is its originating node across every hop, fan-out
excludes the origin, and anti-entropy re-attributes what it delivers to the receiver. A better
predicate was never the answer.

**So ask.** The origin sends each peer the operation's identity — stamp, content hash, key. The peer
checks its own store for that exact stamp and content, forces an `fdatasync`, and answers. The answer
*is* the evidence, sent by the party that has it, about the thing in question.

**Three properties, each the reason a more obvious design was not taken.**
*No wire change* — the exchange rides the existing RPC layer, so `WIRE_VERSION` is untouched and a peer
on an older build never answers, which reads as unknown rather than as a denial. *No retained operation
status* — the peer answers from live state, so there is no table to size, expire or explain when a query
arrives late; §9a reached the same conclusion for the prepared write, for the same reason. *No per-entry
durability tracking* — records append in order to one file, so a single `fdatasync` covers everything
already appended. That last one is the trick that makes the whole design cheap: the new
`WalHandle::sync` appends nothing and is handled in the writer's own loop, so it cannot race a
concurrent append, and the ordering is what makes "everything before this is durable" true.

**The verb had to move up a layer.** `set_with_min_acks` is an extension trait on `KvHandle`, which
holds only `CoreCtx` — and core knows nothing about RPC, deliberately. *Consistency as a service, not a
foundation* had put the verb one layer **below** the protocol it needed to keep its promise. The
replacement `GossipAgent::set_with_replica_sync` sits where both halves are reachable; the old verb is
`#[deprecated]`, still cannot succeed, and still compiles for existing callers. The gateway route needed
no move — it already held the upper context — so `POST /gateway/kv/quorum` works again, and both SDKs'
verbs with it.

**What an answer means, stated once so it is not re-derived.** `persisted_by` names peers whose store
held this exact stamp and content at the moment of answering **and** whose `fdatasync` returned `Ok` —
so they hold it across their own crash and restart. `missing` names everyone else and means **unknown**:
unreachable, mid-restart, on a build without the handler, or now holding a newer value. None of those
establishes absence, and `Failed` in particular denies nothing at all.

**The pin was replaced, not deleted.** PR 4a asserted the timeout *as the contract* so that closing the
gap would have to change the assertion in the open. `a_peer_holding_the_write_acknowledges_it` is that
change. The Phase B exit gate is separate and stricter:
`an_acknowledged_replica_still_holds_the_record_after_restart` stops the peer, restarts it from the same
directory with nothing re-gossiped to it, and asks again — a mechanism answering from memory passes the
first assertion and fails this one.

**One deadline is load-bearing and says so.** The restart gate allows 30 s for the origin's query, not
as slack but because a restarted peer keeps its `NodeId`, so the origin must work through reconnect
backoff on a dead writer entry before any RPC lands. At 10 s it failed while the peer's own answer was
already correct — which is why the test asserts that answer *directly* first, separating the gate's
claim from transport. The distinction is the one recorded this morning in
[testing](../testing/testing.md): a deadline below the callee's own budget makes the assertion
unobservable, and raising it is not the forbidden timeout-widening.
