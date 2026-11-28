# Lock-free mutation rules (papaya) and the atomics policy

↑ [concurrency](concurrency.md) · sibling: [lock-order](lock-order.md)

## The papaya rules — the race family that keeps recurring

Four M2-audit findings (Runs 16–18) plus a sweep all reduced to one shape: **a lock-free
operation followed by an unserialised derived effect.** Two rules close the family; apply
them to every new papaya call site, and rule 3 is the library's own defect on 0.1.x:

1. **`compute` closures must be retry-safe.** papaya re-invokes the closure on concurrent
   change. Never `take()` a single-use value inside one (panics on retry); clone per
   invocation and reset captured outputs at the top of the closure (see
   `apply_and_notify`'s `old_live`, reset at the top of every CAS retry — `mycelium-core/src/store.rs`).
2. **Never act on a stale read.** Collect-then-`remove()`, check-then-act
   (`is_empty()` → spawn), unconditional remove keyed by something another caller may have
   replaced — all must re-validate inside a `compute` (conditional remove), behind an atomic
   `swap`, by `Arc::ptr_eq` identity, or under a stripe lock with a re-read.

3. **No `papaya::HashMap::retain` on papaya 0.1.x** (#799's review, H1, 2026-11-16). papaya 0.1.9's `retain`
   (`raw/mod.rs:1212`) reads each entry through `entry.raw`, the pointer **with its tag bits still set**; `iter`
   reads `entry.ptr`, masked (`raw/mod.rs:2722`). After an incremental resize (the default,
   `ResizeMode::Incremental(64)`) every copied entry carries the `BORROWED` tag (`0b100`), so `retain`'s closure is
   handed a key and value read 4 bytes off: in release a map of about 40 entries already decides on garbage, in
   debug a large one aborts (`misaligned pointer dereference`). Repro: a crate with `papaya = "=0.1.9"`, insert *n*
   `u64 → u64` pairs with key = value, `retain(|k, v| { assert_eq!(k, v); k % 2 == 0 })` — *n* = 40 fails the
   assertion in release, *n* = 100 000 aborts in debug. Fixed upstream in 0.2.x (the same repro passes on 0.2.1 and
   0.2.5). Until the workspace moves to 0.2, filter with `iter` and `remove` the collected keys one by one (the
   reference: `call_issuer::collect` and `call_cancel::register_mcp_call`). The one `retain` the workspace had,
   `call_issuer::collect`'s on `issued` (since #794), was replaced; gate:
   `collect_keeps_the_live_issued_calls_and_drops_the_dead_past_a_resize`. **Gated:** the workspace `clippy.toml`
   bans `papaya::{HashMap, HashMapRef, HashSet, HashSetRef}::retain` (`disallowed-methods`), so every clippy line of
   `make check` and CI refuses a new one — seen firing on a planted `retain` in `src/` (`HashMapRef`) and in
   `mycelium-core` (`HashSetRef`). Lift the ban with the move to papaya 0.2.x (`rust-ecosystem-watch.md` W8).

Reference implementations: `get_or_spawn_writer` (claim-by-sentinel, spawn outside the
closure), `ShardedSeen::evict_below` (conditional remove), `kv_quorum::{install_tracker,
remove_tracker}` (copy-on-write + identity-checked removal), `helpers::merge_peer_keys`
(union recomputed inside `compute` — the prior get-clone-modify-insert lost ~87% of retained
keys under concurrent rotation merges; gate:
`concurrent_merges_for_one_node_never_drop_a_key`).

The same shape reappears **outside papaya** wherever a check and its effect straddle an
await or a lock release — the `AgentStateMachine::transition` budget race (Run 28) was
check-then-act with the counters incremented after commit; the fix is `try_commit`
(validate-and-swap + reserve under the state lock, `src/agent/state_machine.rs`).

And it reappears on **`tokio::sync::watch` channels**: `borrow()` → clone/modify → `send()`
is the same unserialised read-modify-write — two writers each borrow the same snapshot and
the second `send()` silently drops the first's update. The event-driven peer-list publish in
`handle_connection` lost a concurrently-dialing peer exactly this way (unsendable until the
health monitor's first reconcile; found via the `mailbox_llm` flake, 2026-07-21, `1ffe9ea`).
Rule: **mutate watch state only inside `send_modify` / `send_if_modified`** — the closure
holds the channel's internal lock, making the RMW atomic (reference:
`mycelium-core/src/connection.rs`, the `peer_list_tx.send_if_modified` site). Model-checked:
`loom-spike/tests/bounded_append.rs` proves the lost-update schedule against the broken shape
and the single-hold shape's correctness; `/wiki-lint` §1 now runs a mechanical
`borrow()`-then-`send(` sweep so the next such site is a lint finding, not an incident. **Classify every sweep hit by shape:** a *receiver*-borrow relayed into a different sender
(`raw_rx.borrow()` → `tx.send`) is not an RMW; a *sender*-borrow compare-then-send
(`*tx.borrow() == next` → `tx.send(next)`) is not a lost update either when `next` is computed
independently of the borrowed value — but it is the flagged shape, so write it as
`send_if_modified` (the `watch_wiring`/`watch_demand` publishers, converted 2026-09-04) and keep
the sweep mechanically clean.

**A waiter on a single-flight job accepts the run another thread makes for it** (2026-11-21). The identity fold
(`revocation::refresh`) and the client-certificate fold (`client_cert::refresh`) are single-flight on a `running` /
`again` pair: a call while one runs sets `again`, and that one runs once more. A caller that must see a run that
started after its call used to retry until **it** ran one itself (`while !refresh_now(ctx) { yield }`). While another
thread's run was in progress each retry set `again`, so that run re-ran on every pass, never released `running`, and
the waiter never ran one: a livelock, broken only if the waiter's thread was descheduled for a whole run. It held
`an_often_rotated_peer_does_not_exhaust_the_kept_anchor_budget` and CI's Test job for six hours, twice, whenever the
identity watcher's fold started first. The rule: **a waiter takes a ticket before it asks and accepts any run that read
the ticket at its start** (`revocation::fold_ticket` / `refresh_answers`; the run records the newest ticket it read when
it ends whole). Gate: `a_waiter_is_answered_by_a_fold_another_thread_is_running` (a fold slowed by a plant, the waiter on
another thread; seen failing first with the old waiter), with `a_fold_cut_short_by_a_change_of_roots_answers_no_ticket`
and `tickets_are_answered_under_a_busy_fold_and_changing_roots` (#822's review).

**Ask, then try; release, then look** (the same review). The request flag must be set **before** the caller tries to
claim the run, and the holder must read it **after** it releases the run. Otherwise the caller sees the run held, the
holder releases it, finds no request and returns, and only then does the caller's request land, so nobody runs it.
Each side stores one variable and then reads the other, which is the two-variable handshake of the ordering policy
below: `SeqCst` on all four operations. A guard releases the run on every exit, a panic included. Both folds share
the one implementation, `src/agent/single_flight.rs` (`SingleFlight::{run, ticket, answered}`); gate
`a_request_made_while_a_run_ends_is_run`.

**A waiter that holds the run leaves once it is answered: it hands the run on** (2026-11-28, the mirror of #822's
livelock). After #822 a waiter that found no run in progress became the holder, and a holder runs while `again` is set.
With another thread asking every few µs and a pass taking milliseconds, `again` was always set when a pass ended, so the
waiter folded for everyone else and never returned to its own answered ticket. It hung
`tickets_are_answered_under_a_busy_fold_and_changing_roots` about one run in eleven on CI, and in the product it held
`revoke_key` (`refresh_through`) and `change_roots` past their own fold on a node with frequent identity writes.
Releasing the run once answered and re-taking it while `again` is set does not cure it: the release-to-retake window is
nanoseconds wide, so the waiter wins it almost every time. A hand-off to the identity watcher was rejected too: an
unstarted or shut-down agent has no watcher, and the tests fold on such contexts (`a_fold_cut_short_…`,
`a_waiter_is_answered_…`). The rule, in Promise Theory terms: **the folder promises to fold; a waiter promises only to ask
and wait for its own answer.** A plain caller (`SingleFlight::run`: the watcher, the maintenance tick, startup) that
holds the run keeps going while asked. A ticketed caller (`SingleFlight::run_for`: `refresh_answers` in both folds) offers
the run once a pass has answered its ticket. The next caller that finds it held accepts, and is handed it **without
`running` ever being released**. The holder checks for an acceptance *before* it takes the request bit, so it never
consumes a request it will not run. It withdraws an unaccepted offer before it releases, so the release-then-look
handshake above is unchanged. The guarantees: a ticketed holder returns after the pass it is in once someone has
accepted, and so within two passes of the one that answered it under a steady asker. It runs a further pass only when a
request lands in the nanoseconds between its withdrawal and its release and nobody else takes the run. No request is
left unrun, because the taker runs from the request bit as it was left. A plain caller is unchanged except that, when it
finds a ticketed holder offering, it waits at most for that holder's current pass and then holds the run itself. A pass
must not call its own fold's `run_for` on the holding thread (it would accept its own offer); none does. Gates:
`a_ticketed_holder_hands_the_run_to_an_asker_once_answered` (`single_flight`) and
`a_waiter_holding_the_fold_leaves_when_answered_while_another_thread_asks` (`revocation`), each seen failing first on
`01bb75b2`, and the pin `an_asker_that_accepts_as_the_holder_ends_is_handed_the_run`.

## Memory-ordering policy for atomics

- **`SeqCst` — only for a two-sided handshake on two variables** (store-then-load on each side, where one side must
  see the other): `SoftStateGate` (`mycelium-core/src/kv_persist.rs`) — a writer increments `in_flight` then reads
  `closed`; `close` sets `closed` then reads `in_flight`. Acquire/Release does not order a store before a later load
  of a *different* variable, so with anything weaker both sides can miss each other. Say why in a comment at the site.
  - **Across papaya maps, with `fence(SeqCst)`:** the same handshake when the two "variables" are concurrent maps —
    the member-removal admission (#801). `membership_ops::apply` stores the removed set, then clears `peers`; every
    inserter into `peers` (the TCP Ping arm's sender and piggybacked peers, SWIM's `BecameAlive`) stores `peers`, then
    re-checks the set. Clearing a key not yet present writes nothing, so the map's own atomics order nothing; a
    `std::sync::atomic::fence(SeqCst)` between store and load on **both** sides closes the litmus. No test can make
    the fence fail deterministically; the hook tests cover the interleavings, the fences the memory model.
- **`Relaxed` — purely diagnostic counters** (`dropped_frames`, `hash_acc`,
  `listener_count`, `AliveGuard` flags): read by stats/logging only; no control flow depends
  on precise visibility.
- **`Release`+`Acquire` — generation counters and readiness gates:**
  `KvState::grp_generation` (bump on `grp/` write; cache reader sees all prior writes),
  `CoreCtx::soft_state_advertised` (persist-loop store; `/ready` load).
- **`AcqRel`+`Acquire` — lifecycle state:** `GossipAgent::state` CAS transitions
  (`src/agent/lifecycle.rs`).
- **Cancelled flags:** `RegEntry::cancelled` — `Release` on handle drop, `Acquire` in the
  opacity watcher, so pre-drop work is visible before the watcher stops.
