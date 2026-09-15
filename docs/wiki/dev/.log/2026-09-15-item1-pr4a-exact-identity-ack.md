## [2026-09-15] ingest | item 1 PR 4a — the exact-identity ack, and the verb that never could succeed

Up: [dev](../dev.md) · page [runtime-invariants](../architecture/runtime-invariants.md) §*A node can never
observe its own write's propagation* · record `docs/design/contracts-receipts.md` §1a, §8 · plan §3, §8,
§10.12.14 · code `src/agent/kv_quorum.rs`, `kv_quorum_ext.rs`, `mycelium-core/src/store.rs`.

**What PR 4a was scoped to do.** Narrow `set_with_min_acks`' ack predicate from `timestamp >= write_ts`
to exact identity. The plan called it a few lines fixing a live overclaim on a shipped API, and PR 1 had
already planted the pin to flip (`floor_observe_counts_any_update_at_or_after_write_ts`).

**What measuring it first found.** The verb **cannot succeed on this substrate and never could**. Two
connected nodes, the peer demonstrably holding the value:
`set_with_min_acks(1) -> Err(Timeout { acks_received: 0 }); B holds the value: true`.

Three mechanisms compose into it, each correct on its own, each with a good reason, none visible from the
call site: an update's `sender` is its **originating** node and survives every forwarding hop, so a peer
relaying our write is attributed to *us* and the tracker's loopback filter discards it; fan-out **excludes
the origin** rather than the previous hop, so the relayed copy never returns to us anyway; and
anti-entropy **re-attributes** delivered entries to the receiving node. No inbound frame says *a peer
holds your write*. The counter could only ever be moved by a different node independently writing the same
key.

**The corroboration that was there the whole time.** The verb had exactly two tests — a zero-ack case and
a no-peers timeout. The success path had never been exercised in the tracker's entire life: the mechanism
dates from `c6cc3ce` (2026-05-25), eleven days after the repo's first commit, and has shipped in every
release the project has tagged. A verb whose happy path has no test is a verb nobody has watched succeed.

**What shipped.** The ack now requires the inbound update's `content_hash` to equal the write's, so a
newer overwrite counts for nothing — the D9 overclaim is gone, and with it the one way a stray concurrent
writer could produce a false `Ok`. `QuorumObserver` gains `observe_update` (origin, stamp, nonce, content
hash) with a default that delegates to `observe`, so implementations written before this keep compiling
and behaving identically; the identity-less `observe` now establishes nothing and never counts. The floor
pin was replaced rather than deleted, in the open, which is what the floor is for. The structural gap is
pinned by `a_peer_holding_the_write_still_produces_no_acknowledgement`, asserted as a **failure** because
the rustdoc now claims it — a documented impossibility has to be executable, or it rots.

**What it changed about the plan.** PR 4b is reclassified from an enhancement to the only work that can
make a shipped verb return what its name says. No legacy flag was added for the old `>=` behaviour: the
success path it would have preserved was unreachable, so there was nothing to preserve.

**The lesson, and it is the axis' own.** The ADR's §1 inventory was built by reading each call site, and
every row was right about what its code did. This row was wrong about what the *composition* did, because
the three facts live in three files and none of them is wrong. **An inventory of claims owes at least one
measurement per row, not only a reading.** That is the §8 floor discipline — pin the semantics
executably — extended from *what a thing means* to *whether it can happen at all*. Same defect class as
everything else this axis closes: a record claiming more than the underlying mechanism establishes, only
this time the record was ours and the overclaim was reachability.
