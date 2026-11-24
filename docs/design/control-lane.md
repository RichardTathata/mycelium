# A control lane on each peer connection (B1): design note

**Status:** PR 2, the cancel-forward and its reply on the lane, built (2026-11-23; tests
`src/agent/call_cancel_lane_tests.rs`). PR 1, the mechanism, built (2026-11-22; #832, with its review's F1–F7 answered on 2026-11-23: no lane to
a removed member; the 300 s backoff only after a refused handshake; the real bound on a data reconnect stated, one
refused connect and one reconnect backoff; a reply rides only for a received control request, to its sender; the
yield tick a ticker, not a per-iteration sleep): the lane connection and its writer map, the M1 rules, the M3 sites,
M4's evidence append, `CONTROL_KINDS`, `control_lane`, the counters and D7 (`mycelium-core/src/control_lane.rs`; tests
`src/control_lane_tests.rs`). In PR 1 no frame kind used the lane; PR 2 moves `sys.call-cancel.v1` and its reply onto
it (§6). B1 was decided 2026-11-22. An independent review (2026-11-22) found the note *sound with changes*, and this
revision folds those changes in. **Line numbers** are from `origin/main` at `9644e9f8`.

**The spike (§0.4) passed its gate**, 2026-11-22, on #824 + #826 in a test-only build: with the lane, the mints test
kept bound 1 in **20 of 20** debug runs and **10 of 10** release runs; without it, 4 of 10 debug and 3 of 5 release. With
the provider's data-connection reader held for 5 s after the cancel was armed (T1r's shape, debug): without the lane 0
of 3; with it 4 of 5 in the first variant of the hold and 10 of 10 in the second. The spike's code is not merged: its
probes break the sim-seam gate. Its numbers are debug and release builds on one machine, not production figures.
**Evidence:**

- the #826 review (§1);
- the hop probe on #824 + #826 (`probe.patch`, five runs);
- the mint-flood probes (`probe-mints*.txt`, seven runs).

All of it was measured in **debug builds** (`cargo test --lib`), with gateway, provider and about 12,000 mint tasks in
one process. Facts are marked *measured*, *code* or *inferred*.

**The promise at stake.** I4 bound 1: the provider hears a gateway's cancel within *F* = 2 s. The gateway can make
that promise only if the cancel does not travel on a channel the gateway's own bulk traffic fills. Today it does. #826's
re-send turns a *lost* forward into a *late* one. Under about 12,000 concurrent mints only 4 of 10 runs kept bound 1,
with stops from 0.011 to 3.42 s.

---

## 0. Decided (the maintainer, 2026-11-22)

1. **B1 is to be built** (decided earlier the same day).
2. **D1: a separate control connection per peer**, option (c). Yes.
3. **D3/M2: the dual send is kept.** A late duplicate of a control kind is counted in **its own counter**, not in the
   caller-replay counter, so that counter stays meaningful as a replay and security signal (§3.3, T5b).
4. **A spike comes first**, done separately, before PR 1. It is a test-only second writer map on #824 + #826, about
   100 lines, sending the forward's frame on it. Its gate: the mints test is run 20 times in debug and 10 times in
   release, with probe points at the forward's `write_frame` and `read_frame` and the frame sizes recorded. PR 1 is
   written only if bound 1 holds in every run. The spike's numbers replace §2.3's inferred split.
5. **The gateway dropping its own gossip** (15,000–18,000 frames per flood run) gets **a plan row of its own**, outside
   B1 (Appendix A).

Kept from the first draft, as the review recommended: D2, D5, D7, D8.

---

## 1. Decisions (each with a recommendation)

| # | Decision | Recommendation / decided | Behaviour others see change |
|---|---|---|---|
| D1 | Mechanism: (a) a second channel in today's writer; (b) a reserved slice; (c) a separate control connection. | **(c), decided 2026-11-22.** (a) and (b) leave the frame in the same TCP byte stream and the same sequential receiving reader as the gossip. The code alone settles that (§2.2); the numbers only size it. | One more outbound TCP (mTLS) connection to a peer while control traffic flows; it is idle-evicted. It counts against the receiver's `max_connections` (1,024, one semaphore) and `max_connections_per_source`, and appears in conntrack and firewall counts. In the worst case, a mass revocation from many gateways, the lane doubles a provider's inbound connections for up to 30 s and can take a permit a new peer's data connection needed. That is bounded by idle eviction and goes in the upgrade note. |
| D2 | Which frames qualify. | **`sys.call-cancel.v1` and its `rpc.result` reply only** (kept). `CONTROL_KINDS` is a closed `const`; a PR that extends it names the kind and its rate bound. | None outside I4. |
| D3 | Lane only, or in addition. | **Dual send, decided 2026-11-22**, with late duplicates counted on their own (M2). | One duplicate frame per control frame. The receiver's node-wide seen-set drops the second copy before delivery and before relay (`connection.rs:948-950`). A twin arriving after the seen-set trimmed its nonce is counted in `control_late_duplicates` (§3.3). |
| D4 | To which peer a lane opens (M7, M1). | **To the frame's own target:** for a request, the provider the gateway dispatched the call to; for a reply, the request's sender. **Never while that peer's data writer exists but has no connected socket** (in backoff, or not yet connected), and **never within a long backoff after a refused lane connect**. | A provider outside the active forwarding set (a partial mesh) still gets a lane: reachability is the call's own. No lane opens where it could take the per-source slot a data reconnect needs. |
| D5 | Capacities and timeouts. | Lane depth **64 per peer**. Write-progress bound **10 s**. Idle eviction **30 s** (`writer_idle_timeout_secs`). Connect backoff `reconnect_backoff_secs`. After a refused connect, **300 s** (`CONTROL_LANE_REFUSED_BACKOFF_MS`). The lane **yields** within **1 s** when the peer's data writer loses its socket. All are constants in PR 1 (kept). | None by default. |
| D6 | An off switch. | `GossipConfig.control_lane: bool`, **default on**. `GossipConfig` is `#[non_exhaustive]` on main (CR12, `config.rs:1178`), so the field breaks no struct literal outside the crate. *(The review assumed CR12 had not landed; it has.)* | A configuration field (a table row in `configuration.md`, an environment variable, the feature page). **The upgrade note and the `validate` doc must say that `max_connections_per_source` has to allow 2 per node behind one source address, or the fleet sets `control_lane = false`.** The cap is on the receiver and the switch on the sender: a dependency across nodes, stated. |
| D7 | Relays. | **A relay never puts a frame it received on a lane** (kept). | None. Relays forward through the shard only (`connection.rs:1031-1057`). |
| D8 | Counters and gauges. | `SystemStats` and metrics: `control_lane_sent`, `control_lane_full`, `control_lane_connect_failed`, `control_lane_yielded`, `control_lane_dropped`, `control_late_duplicates`, `control_lane_connect_ms` (a histogram), and **a separate gauge `gossip_control_lanes_connected`**. `gossip_peers_connected` and `cached_connections` exclude lanes (M3). | `SystemStats` is not `#[non_exhaustive]` (`src/agent/mod.rs:566`), so an outside struct literal breaks, as at earlier releases. |

**Not decided here:** #826's F1 (re-sends drain the provider's confirm-wait permits) and F2 (no per-term dedup of
forwards). The lane makes F1 rarer, because the first attempt arrives early, but it does not fix it. F2's per-term
marker bounds how much demand reaches the lane, and should land with #826's fix round, before PR 2.

---

## 2. Today's path, exactly

### 2.1 Send side: `rpc_call_ctx` to the socket

1. **`call_cancel::forward`** (`src/agent/call_cancel.rs:284`; on #826, `attempt`) calls `rpc_call_ctx` →
   `rpc_call_framed` (`src/agent/rpc.rs:468`). That registers the waiter, prepends the nonce and calls `emit_signal`
   with `Individual(target)` (`rpc.rs:482`).
2. **`emit_signal`** (`mycelium-core/src/ops.rs:80`) delivers locally, encodes and calls `dispatch_gossip_try_send`
   (`ops.rs:110`). Its `true` means only that the frame entered a shard queue.
3. **Shard queue** (`framing.rs:264`).
   - **Signals are sharded by kind** (`framing.rs:252-258`): all cancel-forwards share one shard, and so do all
     `rpc.result` frames.
   - There are `gossip_shards` queues (default `available_parallelism`, `config.rs:2360`), each 1,024 deep
     (`gossip_channel_capacity`, `config.rs:2356`). The send is a non-blocking `try_send` through the channel seam
     (`outbound.rs:211`).
   - **Drop point 1:** the shard is full; `dropped_frames` rises (`framing.rs:279`) and `rpc_call_framed` ignores the
     `false`. *Measured:* no drop here.
4. **Shard worker** (`run_gossip_shard`, `src/agent/tasks.rs:221`). A `biased` select (`:293`). It never awaits a
   writer. *Measured:* it dequeues in the offer's millisecond. Before handing off:
   - a frame addressed to this node terminates (`:329`);
   - with no peers at all, the frame is dropped and counted (`:333`, **drop point 2**);
   - with no direct route (`targets ∪ pinned`, `:366`), it floods via relays (`:360-393`).
5. **Hand-off to the peer writer** (`send_to_peer!`, `tasks.rs:246-289`): `try_send_counted` on the peer's `WriterTx`
   (`outbound.rs:245`). Its depth is `writer_channel_depth` = max(4N, 1,024) (`config.rs:2552`).
   - **Drop point 3:** the channel is full (`:264`). A closed writer is respawned and retried once (`:277-285`).
   - *Measured:* this is where the 26 s run lost its forward (`probe-run3.txt`). The gateway had dropped 900–2,100 of
     its own frames by the time the forward was emitted, and 15,000–18,000 by the end. The provider dropped none.
6. **Other producers on the same writer channel:** the hello (`tasks.rs:628`), the TCP ping when SWIM is off (`:876`),
   `connect_peer`'s ping (`topology.rs:68`), `request_state` (`writer.rs:534`), the pong (`connection.rs:757`), and
   the StateResponse, sent from a spawned task that awaits room (`connection.rs:800-806, 890-898`). SWIM is UDP
   (`swim.rs`) and off this path. It cannot carry the frame anyway: `SWIM_MAX_DATAGRAM` = 512 B (`swim.rs:45`).
7. **The writer task** (`run_peer_writer`, `mycelium-core/src/writer.rs:101`). One task, one TCP connection.
   - **Receive:** a `biased` select over channel, shutdown, eviction and idle (`:137`).
   - **Backoff:** every frame dequeued inside a reconnect backoff is dropped and counted (`:155-163`, **drop point 4**).
   - **Connect:** lazy, TCP plus mTLS, bounded by `handshake_timeout_ms` (`:184-245`).
   - **Batch:** writes, drains the queue into a 16 KiB `BufWriter` and flushes once (`:276-295`). TLS encrypts per
     write, under the buffer.
   - **Progress bound:** `peer_write_stall_timeout_ms`, default `PEER_WRITE_STALL_DEFAULT_MS` = 1,140 s
     (`config.rs:1147`). That is the sender's stall ceiling, sized for a receiver applying a worst-case chunk under
     `flush`. It is not a measured reader stall.
   - **On failure:** the frames queued are dropped during backoff (`:316`, **drop point 5**).
   - **Oversize:** an oversize frame is dropped and the connection kept (`:255-268`; size gate `framing.rs:357`,
     `MAX_FRAME_BYTES` = 10 MiB).
8. **Delays on the send side.** The frame waits behind up to 1,024 queued frames. Then, in order, it waits behind every
   byte already in the `BufWriter`, both kernel buffers and the peer's 8 KiB `BufReader`. Priority inside the writer
   could shorten only the first of these.

### 2.2 Receive side: the connection reader (corrected)

`handle_connection` (`mycelium-core/src/connection.rs:380`) is **one sequential loop per accepted socket**. Each frame
is read, decoded and handled before the next byte is read.

- **Rate limiter** (`:538-570`): drops any frame over `max_inbound_frames_per_sec` or the M7 throttle, per connection
  (**drop point 6**). Off by default.
- **`Data` / `SignedData`:** `apply_inbound_then_append(...).await` (`:1132`, `:1329`). Under `flush` this awaits the
  WAL's per-record acknowledgement (`persistence.rs:231-238`). The WAL queue is shared by every connection, so the wait
  includes other peers' records. O11-2 removes it. Under `async` the append is a `try_send` (`persistence.rs:371-383`).
  - *Measured, debug:* the provider applied about 2,850 records/s, about 0.35 ms each.
- **`StateResponse`** (`:903-932`): every entry of a chunk of up to 10 MiB is applied inline. This is CPU-bound even
  without a WAL wait.
- **`StateRequest`** (`:764`): the O(divergence) scan runs inline. **The response send does not stall the reader:**
  it runs in a spawned task (`:800-806`, `:890-898`). *(The first draft said otherwise; corrected.)*
- **`Signal`** (`:938`):
  - removed-member drop and nonce dedup (`:944-950`);
  - the interceptor or `signal_handlers.deliver` (`:1001`): a `try_send` into the kind's own channel
    (`signal.rs:392-414`; full → `signal_handler_drops`, **drop point 7**);
  - then, for a kind with a local worker (`sys.call-cancel.v1` has one, `signal.rs:1191`), **quorum evidence**: a KV
    write and `wal.append(..).await` inline (`:1007-1021`). Under `flush` this waits FIFO behind the shared WAL
    channel (`persistence.rs:371-373`), behind every record any reader queued. It is not "one fsync";
  - then a forward into this node's shard, where the frame terminates (`:1031-1057`).
- **After the reader:** `call_cancel::serve` (`call_cancel.rs:674`) takes requests one at a time from its own channel.
  It does not queue behind gossip.

**Answer to the brief's question.** Yes: on today's single connection a control frame queues behind gossip **in the
reader**, which handles every frame ahead of it, waiting under `flush` per record on a WAL queue shared with third
parties. It does not queue behind gossip in the dispatch after the reader. The reply crosses the gateway's inbound
reader from the provider the same way. **Even after O11-2**, three waits remain in the reader:

- the inline apply of a StateResponse chunk;
- under `flush`, the shared WAL queue;
- the quorum-evidence append.

So (a) plus O11-2 plus bounded socket buffers could not support a *stated* bound, though it would often pass the debug
test. A green T1b is therefore not the proof; T1r is (§5).

### 2.3 Where the ~1 s goes (*inferred, from a debug build*)

The probe (debug) recorded hand-off to the provider's decode at 1,087, 831, 824 and 826 ms. The shard hop took about
0 ms. At the 0.35 ms per frame measured in the same debug process, the ≤ 1,024 frames in the writer channel account for
at most about 0.36 s.

The rest, 0.46–0.73 s, is *inferred* to sit in the batch, the kernel buffers and the reader. This is arithmetic on one
rate. It assumes the writer drains at the receiver's apply rate through TCP backpressure, and frame sizes were not
recorded. In a release build the residual is likely several times smaller. **These are not production figures.** The
structural argument does not depend on them. The spike (§0.4) measures the split.

---

## 3. Design

### 3.1 Options (the record of the choice)

| | (a) second channel in today's writer | (b) reserved slice | (c) separate connection (**decided**) |
|---|---|---|---|
| Drop when gossip fills the writer | removed | removed | removed |
| Wait behind ≤ 1,024 queued frames | removed | kept | removed |
| Wait behind socket buffers and the receiver's reader | kept | kept | removed |
| Receiver's rate-limit window | shared with the flood | shared | its own |
| New `select!` choice | yes | no | no |

Rejected:

- **(b)**, which prevents the drop and nothing else.
- **(d) bypassing the shard alone**, which saves about 0 ms.
- **(e) a small `SO_SNDBUF`**, which bounds the bytes ahead of the cancel but not how long the reader takes over them,
  and costs throughput.
- **UDP/SWIM**: 512 B datagrams, unauthenticated and lossy.
- **The gateway shaping its own burst.** That is the gateway's own promise and worth making (Appendix A), but it
  cannot bound the receiver's reader.

### 3.2 The shape

**Sender.**

- A second map, `control_writers: papaya::HashMap<NodeId, WriterEntry>`, beside `peer_writers`. Its writers are
  spawned by `get_or_spawn_writer` (`writer.rs:364`) with their own depth, timing and stream name
  (`writer/control/{peer}`).
- `get_or_spawn_writer` takes a **gauge selector**, so a lane updates `gossip_control_lanes_connected` and never
  `gossip_peers_connected`. Today `publish_connected_gauge` (`writer.rs:451, 497-501`) would count lanes as peers (M3).
- Each lane runs `run_peer_writer` with two additions:
  - a **yield check**: before each batch and on a 1 s tick, if the peer's data `WriterEntry` exists and its new
    `connected` flag is false, the lane closes and counts `control_lane_yielded`;
  - a **refused-connect backoff** of 300 s.
- `WriterEntry` gains `connected: Arc<AtomicBool>`. The writer sets it on a completed handshake and clears it on any
  drop of the socket.

**Emission.** `ops::emit_signal_control(ctx, kind, target, payload) -> ControlSend`:

1. It encodes **once**. Then it dispatches the shared copy exactly as `emit_signal` does.
2. If `kind ∈ CONTROL_KINDS`, `control_lane` is on, D4 allows a lane to `target`, and the frame is ≤ 64 KiB, it
   `try_send_counted`s the **same `Bytes`** to the lane. A fresh encode would draw a new nonce and defeat the dedup;
   that is T5's plant.
3. **It reuses `send_to_peer!`'s `Closed` → respawn-and-retry-once** (`tasks.rs:277-285`), so a lane copy enqueued as
   an idle lane shuts down is retried, not counted as sent and lost.

`rpc_call_framed` gains a lane argument. `rpc_respond_ctx` (`rpc.rs:414`) puts the reply on the lane when the
request's kind is in `CONTROL_KINDS`, toward the request's sender.

**Receiver.** The lane is one more inbound socket served by `handle_connection`. The review verified that nothing is
keyed one-connection-per-peer:

- the listener spawns per socket (`tasks.rs:136-157`);
- `handle_connection` keeps per-socket state only and cleans up nothing keyed by peer;
- `record_anchor` on the lane's handshake records the same key.

**One change is needed (M4).** For `CONTROL_KINDS`, the quorum-evidence WAL append (`connection.rs:1007-1021`) moves
off the reader into a spawned task. The apply stays inline first, so "apply, then hand to the WAL" holds. The spawned
appends are bounded by the evidence rate limit of one per (kind, sender) per second. `append_try` was considered and
not chosen: a dropped append would be counted in `dropped_appends`, which the O10 tests assert is zero. The inline
await for other kinds is left as is; that is a data-path question outside B1. **M4 covered request kinds only in PR 1** (`is_control_kind(&sig.kind)` in `connection.rs`): a lane frame of kind
`rpc.result` would still have appended inline, safe only because `rpc.result` has no local worker and so writes no
evidence (`quorum_evidence_payload`, `has_worker`). **PR 2 widens the condition** to `rpc.result` too, so the reply
never waits for the WAL in a reader even if something were to register a worker for it; for every other kind the
inline await is unchanged. **Old receivers keep the coupling:** a
new gateway with an old provider gets the first lane frame through, and a later lane frame can wait behind the
provider's WAL backlog under `flush`.

### 3.3 The questions, for the decided shape

**Which frames qualify (D2).**

| In the lane | Kept out | Why |
|---|---|---|
| `sys.call-cancel.v1` requests | Consensus | Its volume is set by the slot. |
| Their `rpc.result` replies | `sys.replica-sync.v1` | Its answer must stay behind its records on the data connection. |
| | `mcp.invoke`, `llm.invoke`, general RPC | Their volume is set by clients. |
| | Ping, hello, StateRequest, StateResponse | The hello must precede the StateRequest in FIFO order (`tasks.rs:605-628`); StateResponse is bulk. |
| | Cluster and Group signals | Flood frames. |

**Who may mark a frame.** Only `emit_signal_control` with a kind in `CONTROL_KINDS`; any other kind is refused and
counted. No remote input reaches the lane:

- relays use the shard (D7);
- gateway clients cannot emit `sys.` kinds or `rpc.result` (`refuse_internal_kind`, `src/agent/http/authz.rs:587`);
- a client's `rpc/respond` answers a client-served kind.

Local code is the node itself, and the depth of 64 bounds even that. A remote member flooding a connection it opened is
today's situation, and B1 does not change it.

**Capacity, and a full lane.** The depth is 64. A full lane never blocks and is never silent: `control_lane_full`
rises, a warning is rate-limited, and the shared copy has already been offered. Lane copies lost in backoff or a failed
write go to `control_lane_dropped`, not `dropped_frames`.

**Fairness.** Two tasks and two sockets, with no arbitration. Control volume is at most 64 small frames per peer.
Gossip's drop odds are unchanged.

**Ordering.**

- *Kept:* FIFO within each lane, and today's Individual-scope path in full. The lane is additive: forwarding stays
  unconditional, only admission is scoped, and it is not a carve-out.
- *Lost:* order between a control frame and frames sent earlier on the data connection. Two consequences:
  - The cancel-forward carries its signed checkpoint, the issuer's key is already at the provider, and the reply
    carries its own answer.
  - **M6: a cancel can now overtake its own call.** The call request is an ordinary Individual signal on the data
    connection. A cancel issued while that request is still queued behind gossip can reach the provider first. The code
    handles it: the provider holds the revocation, answers `NotRegistered`, and refuses the later call `revoked`
    (`call_cancel.rs:129-134`). The receipt is then `NotRegistered`, where it would have been `Stopped`. If the
    provider's revocation windows are at their cap (`NotHeld`), the late call is judged by the KV record instead. This
    goes on the feature page and in T9.

**Late duplicates (M2, decided 2026-11-22).**

- The shared copy is the slow one: 24–27 s late in the measured failures, and unbounded behind a stalled reader.
- The seen-set normally keeps a nonce for about 1,200 s, but **emergency-trims to 60 s** when over `max_seen_entries`
  (`tasks.rs:1126-1140`). Under a flood the set passes that bound within seconds.
- A twin arriving after the trim reaches `rpc_rx`. There the envelope's replay window drops it as `Replayed`, which
  today raises `caller_replay.count_replay()` (`gateway_caller.rs:1693-1697`).
- **Decided:** in `verify_received`, a `Replayed` request whose kind is in `CONTROL_KINDS` is counted in
  `control_late_duplicates` instead. `caller_replay` keeps counting every other kind.
- *The trade-off, stated:* a genuine replay of a cancel-forward is now counted there too. It is harmless: a repeat is
  idempotent and answered from what the provider holds, as the #826 review §3 verified.
- A twin older than 600 s is refused as stale and answered. The gateway finds no waiter for that answer and drops it.
- A late reply twin at the gateway finds its waiter already removed and reaches no caller.

**Wire.** v12 unchanged: no variant, no field and no version change. **Mixed fleets:**

- an old receiver serves the lane like any socket, but keeps M4's inline evidence wait;
- an old sender has no lane;
- a new gateway with an old provider gets a fast request and today's reply path.

**Lane down, data up:** the shared copy delivers. **Data down, lane up:** the lane delivers. The reply goes on the
provider's lane back if one opens; otherwise it goes by the shared path, which may flood. That still meets bound 1, but
the receipt is late.

**A flapping data writer** (PR 2, from #832's round-3 review). While the provider's data writer is reconnecting, no
lane opens to it and an open one yields, so a cancel in that window goes by the shared path alone: it is covered by
#826's re-sends (an unanswered forward is sent again until the provider answers or the window ends) and by the
revocation the KV carries, which reaches the provider once the data connection is back and stops the call there. **A
lane in the middle of its TCP connect or TLS handshake checks nothing until that step ends** (bounded by
`handshake_timeout_ms`): the yield check runs before each batch and on the tick, so a data writer that loses its socket
during the lane's handshake is seen only once the handshake completes or fails.

**A reply rides the lane only for a request that passed its receive boundary as its own sender** (PR 2, #832's
round-3 review): the provider's `serve` answers through `rpc_respond_control` only when the request's envelope verified
signed as its sender and its `via` is the frame's `sender`. A refusal, and an admitted request whose `via` is another
node, is answered by the shared path only; and the lane opens no M7 lane for a reply to a node this one does not know.
A forged `sender` therefore cannot make a provider dial an address of the forger's choosing.

**Per-source caps (M1).**

- `max_connections_per_source` refuses by accepting and closing the socket (`tasks.rs:143-146`).
- **The harmful case is a data writer that loses its socket while a lane is open:** an idle eviction, a write failure
  or a provider restart, whose reconnect is then refused. D4 and the yield check close it:
  - no lane opens while the data writer is unconnected;
  - an open lane closes within 1 s of the data writer losing its socket;
  - a refused lane connect backs off 300 s.
- Recovery bound for the data writer: 1 s + `reconnect_backoff_secs` × 1.5 + one handshake.
- **Plaintext:** without TLS, an accept-and-close refusal is not seen as a failed connect. The first write appears to
  succeed, and a later write fails. So `control_lane_connect_failed` can stay 0, and the loss shows in
  `control_lane_dropped`. T6 and T6b run under TLS, and the plaintext case is stated.

**The first cancel's handshake cost.** The lane connects lazily on its first frame: a TCP connect plus a full mTLS
handshake, bounded by `handshake_timeout_ms`. That is milliseconds on loopback, and 1–3 RTT plus certificate checks on
a WAN, more under the gateway's CPU flood. It is reported in `control_lane_connect_ms` and quoted in T1b. Warming the
lane when a cancellable call is dispatched is an option **not taken**, because it costs one connection per provider with
a call in flight.

---

## 4. Invariants touched

1. **One lock per function.** No new lock, so no lock-order row. The existing `reorder_buf` mutex (`connection.rs:972`)
   is now taken by two reader tasks for one peer, as it already is across peers.
2. **Papaya.** The claim-then-spawn code is reused, with `connected` as an atomic. A stale D4 read only adds or skips a
   lane copy.
3. **Every user of `peer_writers` that needs a control twin (M3, rule 1).** Found with
   `git grep -n "peer_writers\|evict_peer_writer(\|publish_connected_gauge" -- src mycelium-core/src`. PR 1 carries
   this as a table of each site and its test:

   | Site | What the lane needs |
   |---|---|
   | `tasks.rs:772, 792, 857, 939` (`evict_peer_writer`) | Evict the lane too. |
   | `swim.rs:278` (SWIM's death of a member) | Evict the lane. |
   | **`membership_ops.rs:56`, C5 member removal** (a blind `writers.pin().remove`) | Remove the lane and signal its shutdown, or an mTLS lane stays open to a removed member. |
   | `lifecycle.rs:1519-1524`, the shutdown sweep | Sweep lanes, and the outbound count waits for them. |
   | `tasks.rs:1145-1154`, `reap_finished_writers` | Reap lanes and publish the lane gauge. |
   | `topology.rs` `disconnect_peer`, `peer_drop_counts` | Close the lane; drops reported apart. |
   | `introspect.rs:189`, `cached_connections` | Lanes **excluded**; `control_lanes_open` beside it. |
   | `writer.rs:451, 497-501`, `publish_connected_gauge` | The gauge selector (§3.2). |
   | `connection.rs` (6 uses: the pong and StateResponse writers) | **None.** They stay data-only. |

4. **The sim seam.** The lane's send goes through `chan_try_send` on stream `writer/control/{peer}`, so it is
   recorded. No forbidden call is added: `run_peer_writer`'s sites are reused, and the baseline line
   `6 mycelium-core/src/writer.rs` is unchanged. The yield tick goes through `sim_seam::sleep_ms`, and M4's spawn adds
   no clock or RNG call. The writer's choice is not a recorded choice, because under (c) there is none. Two sockets are
   real network timing, outside the kernel by the inventory, so **replay needs nothing new**. The inventory gains the
   stream name. *Inferred, to check in PR 1:* that the order in which two reader tasks deliver into one handler channel
   is not asserted per stream by the recorder. No recording contains a cancel-forward.
5. **The `framing` size gate.** Unchanged on both connections, and the lane refuses copies over 64 KiB, counted.
6. **The write-stall detector.** `PEER_WRITE_STALL_DEFAULT_MS` stays on the data connection. The lane writes under
   10 s, and expiry is counted in `outbound_stalls`. *(The first draft claimed this 10 s bound covered the receiver's
   evidence wait. It does not: 64 small frames never fill a socket, so a sender-side bound never sees a receiver-side
   wait. M4's change covers it.)*
7. **`Unsent`.** Lane frames go through `WriterTx`, so a graceful shutdown waits for them.
8. **Individual-scope forwarding is unconditional.** Kept, because the lane is additive. The wiki's runtime-invariants
   page gains a line saying the lane is an extra copy and not a carve-out.
9. **Persistence.** Apply, then hand to the WAL: kept under M4. The append is moved, not reordered.

---

## 5. Tests that prove it

Each test is collected by discovery and seen failing first with the plant named.

| Test | Asserts | Fail-first plant |
|---|---|---|
| **T1** `a_cancel_forward_reaches_the_provider_while_the_gateways_data_writer_is_held` | The sender side. The gateway's **data** writer to the provider is held and a mint burst fills its channel. The provider stops the call while the writer is still held, receipt `Stopped`. **`TEST_WRITE_PAUSE_MS` changes:** keyed by peer *and lane*, and held until released rather than a finite sleep. | No lane copy: no stop while held. |
| **T1r** `a_cancel_reaches_the_provider_while_its_data_reader_is_held` (**the test of D1**) | Under `flush`, the provider's **data-connection reader** is held mid-record by a held WAL acknowledgement for a burst key (`pause_inbound_after_apply`, the pattern at `connection.rs` tests about :1380). The cancel still stops the call, receipt `Stopped`, while the reader is held. | Lane copies put into the data writer's channel (option (a)): no stop while the reader is held. |
| **T1b** the mints test, kept, plus a structural line | At the stop, the provider has applied fewer burst records than the gateway holds. The *F* assertion stays. It also reports `control_lane_connect_ms`. Run 20 times in debug and 10 in release on #824 + B1, both quoted. | As T1: about 4 in 10. |
| **T2** `a_control_frame_is_never_dropped_while_the_lane_has_room` (core) | Data channel full and held. 64 control frames → `sent = 64`, `full = 0`, all on the lane socket. The 65th → `full = 1`, with its shared copy offered. | Lane routed into the data `WriterTx`. |
| **T3** `gossip_progresses_while_the_control_lane_is_held` (core) | The lane is held with its channel full. KV converges, and the data writer's batches continue. | The lane's write made in the data writer's task. |
| **T4a** `a_relay_never_puts_a_received_frame_on_a_lane` | On a line A–R–P, R's `control_lane_sent` stays 0 and P receives by flood. | A relay promoting by kind. |
| **T4b** `no_gateway_client_reaches_the_lane` | `signal/emit` and `rpc/respond` leave the lane counters at 0. | Classifying by kind inside `emit_signal`. |
| **T4c** `a_storm_of_cancels_is_bounded_by_the_lane` | 4 × 64 concurrent cancels: in flight on the lane ≤ 64, `full` counted, all answered, gossip converges. **Run also under `flush` with the provider's WAL acknowledgement held.** | Depth unbounded. |
| **T5** `each_cancel_attempt_is_served_once_though_sent_twice` | `serve` sees each attempt once, and each reply is claimed once. | The lane copy re-encoded (a fresh nonce). |
| **T5b** `a_late_twin_of_a_cancel_is_counted_as_a_late_duplicate_not_a_replay` | The provider's seen-set is trimmed (a test hook on `emergency_trim`), then the shared twin is released. `control_late_duplicates` = 1, `caller_replay` unchanged, nothing re-delivered. A replayed non-control kind still raises `caller_replay`. | The classification removed: `caller_replay` rises. |
| **T6** `a_refused_lane_connect_falls_back_and_backs_off` (**under TLS**) | Receiver at `max_connections_per_source = 1` with the data connection up. The lane connect fails, counted; the receipt is `Stopped` by the shared copy; the data connection is undisturbed; no second lane attempt within the 300 s backoff (clock seam). The plaintext limitation is stated in the test's doc. | No refused backoff: repeated connects. |
| **T6b** `a_data_writer_reconnects_past_an_open_lane` (**under TLS**, M1) | Cap 1. The lane connects first, then the data writer's socket is dropped. Asserted: the lane yields (`control_lane_yielded` = 1), and the data writer reconnects within 1 s + 1.5 × `reconnect_backoff_secs` + a handshake, and gossip resumes. | Yield check removed: the data writer is refused until the lane idles out. |
| **T7** `every_writer_site_handles_the_lane` | Each row of §4.3: removal (C5 included), SWIM death, reap, disconnect, shutdown. Each closes the lane. `Unsent::underflows() == 0`. **`gossip_peers_connected` and `cached_connections` are unchanged by a lane.** | One site left out, or the gauge selector removed. |
| **T8** `a_provider_without_the_lane_still_answers` (mixed, emulated) | Provider at `control_lane = false`: request on the lane, reply shared, `Stopped`. A run against the v2.40.x binary is **not run** unless the PR says so. | — |
| **T9** `a_cancel_that_overtakes_its_call_refuses_the_call` (M6) | The call request is queued behind a held data writer, and the cancel goes on the lane. Receipt `NotRegistered`; on release, the call is refused `revoked` and never runs. | The provider's hold of an unregistered term removed: the call runs. |
| **T10** `a_lane_reader_does_not_wait_for_the_wal` (M4) | Under `flush`, the provider's WAL acknowledgement is held. Two cancels for two calls arrive in sequence on the lane, and **both** stop their calls while the acknowledgement is held. | The evidence append put back inline: the second waits. |

---

## 6. PR split, size and order

0. **The spike** (§0.4, being done separately): no PR, its results quoted in PR 1. It replaces the first draft's
   probe-only step.
1. **PR 1, core and mechanism, about 700–900 lines with tests:**
   - `control_writers`, `connected` on `WriterEntry`, the yield check and refused backoff, the gauge selector;
   - `emit_signal_control` with `CONTROL_KINDS` and the respawn-retry;
   - M4's evidence-append move in `connection.rs`;
   - the counters, gauge and histogram (`metrics.md`, `/stats`);
   - `control_lane` (configuration table, environment variable, `validate` doc text on per-source caps);
   - the §4.3 site table;
   - T2, T3, T6, T6b, T7, T10;
   - the inventory's stream name and the wiki's runtime-invariants line.

   No kind uses the lane yet.
2. **PR 2, I4's use, about 400–600 lines:**
   - the cancel-forward and its reply on the lane;
   - the M2 classification in `verify_received`;
   - T1, T1r, T1b, T4a–c, T5, T5b, T8, T9;
   - the feature page (`docs/features/mandates.md`'s cancel section: the lane, the overtaking cancel, the per-source
     rule) and its registry entry, `what-is-proven.md`, and the upgrade notes (second connection, `max_connections`,
     per-source caps, `SystemStats` fields);
   - O13's text: (B) built.

**Order:**

1. #826 merges first, with F1 and F2 fixed or answered.
2. The spike.
3. PR 1, then PR 2.
4. **#824 waits for B1** (decided), is rebased on it, and T1b is run there; that closes O13.
5. **O11-2 after #824.** B1 does not need O11-2, but O11-2 still matters for gossip throughput and for every other RPC.

---

## Appendix A. A proposed plan row: the gateway drops its own gossip

**Decided 2026-11-22: a separate row, outside B1.**

- **Observed** (*measured*, debug, the mints probes). Under a burst of about 12,000 concurrent mints the gateway drops
  **15,000–18,000 of its own gossip frames per run**. 900–2,100 are already gone when a cancel is emitted. All are
  dropped at its writer channel to the provider (`send_to_peer!` → `Full`, `tasks.rs:264`). The provider drops none.
  Anti-entropy repairs the records about 25–30 s later (its interval is 30 s).
- **Why it is a defect.** The gateway's own writes are delivered by the 30 s repair loop instead of the 0.x s push.
  Nothing tells the writer: `kv_set` and the mint's records return success, and only `dropped_frames` moves. A reader
  of the provider sees the gateway's records late by the anti-entropy interval exactly when the gateway is busiest.
- **The promise.** In Promise Theory terms this is the gateway's own behaviour, so it can promise about it: the records
  it writes reach a connected peer by push, or the gateway knows they did not.
- **Options for the row:**
  - (1) pace the producer: a mint path that awaits shard room (`dispatch_gossip_send`), bounding the burst at its
    source;
  - (2) backpressure at the shard → writer hand-off for KV records (await, not drop), with a bounded wait and a counted
    fallback;
  - (3) coalesce: the writer drops an older queued update for the same key when a newer one arrives;
  - (4) state the repair interval as the bound and count it per key family.
- **Interactions:**
  - (2) changes the shard worker's never-block property, which today keeps the shard queues from backing up into
    every emitter.
  - (1) changes the mint's latency under load.
  - None touches B1, because the lane is independent of the data path's drops.
- **Closing evidence:** the mints test reports gateway `dropped_frames` = 0 for KV records (or the bound stated),
  repeated as often as it is run.
- **Where it belongs:** after O11-2. The group-commit (#824) and O11-2 work change the burst's shape and its pace.
