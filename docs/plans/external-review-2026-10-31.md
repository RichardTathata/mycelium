# The external code security review of 2026-10-31 — verification and repairs

**Status:** agreed with the maintainer on 2026-10-31, rev 0.8 (rev 0.2 takes the reviewer's two qualifications of the verification, 2026-10-31: F4 is P1 regardless of token lifetime, and F3's window is unmeasured; rev 0.3 takes the independent review of this plan's PR, #711: the journal consumers re-enumerated, R6's exit restated, R2 led by the poisoned writer, the raw routes enumerated; rev 0.4 records R4 delivered, #712, with the all-unparsable decision; rev 0.5 records R2 and R3 delivered, #713 — the restart that keeps only the WAL, and the grant records a step must not proceed without on disk before that step or the step refused by name, the use released on a durable refusal, 2026-10-31, merged 2026-11-06; rev 0.6 records R1 delivered, #714, 2026-11-06 — the raw routes' body doors, the three path doors, and from its second review round the replica-sync reads `snapshot` and `wal_replay` as reads and the mailbox stream; rev 0.7 records R1–R4 released in v2.39.0, 2026-11-06; rev 0.8 records R5–R9 delivered — #719, #718, #722, #720, #721 — and corrects R9's exit, whose dev-only pair left the tree rather than being ignored, 2026-11-07). Nine rows; **R1, R2, R3 and R4 released in v2.39.0** (#714, #713, #712); **R5–R9 delivered on `main`, unreleased** (#719, #718, #722, #720, #721), each closed on evidence in §5. The review is vendored at
[`external/2026-10-31-code-security-review.md`](external/2026-10-31-code-security-review.md); it examined commit
`d656f50c` (main after Y11, #708). Every finding was verified independently before this plan was written (§2), under
the [verification policy](../wiki/dev/testing/verification-policy.md): three read-only agents re-ran or re-derived
each finding against the reviewed commit, with the repository's own stated limits beside them.

## 1 · What the review found, and what the verification found

The review reports seven findings, five P1 and two P2, with reproductions for six. **All seven are true.** None is
false, and none is a priority inflation worth disputing. Its method is the one this repository asks of itself: a
precondition, an observed run, an acceptance criterion, and a plain statement of what was not reproduced.

Its one weakness is that it did not read what the repository already says it does and does not claim. Three findings
*contradict* a claim the repository makes — the strongest form of finding, and the review reports them as if they
were gaps. One mechanism the review names is a stated limit (`docs/threat-model.md:457`), with the scope consequence
unstated. Two architectural claims are refuted by the docs: the provider-side preflight exists
(`with_provider_enforcement`, gated by the C7 bypass matrix) and is honestly bounded by the threat model to
`mcp`/`skill`/listed kinds; and the guarantee catalogue names every guarantee's *Needs* and *Enforcement points*.
The dependency section is correct in every fact.

The verification also found one thing the review did not: the test that shows a grant **held across a restart** (X3,
`grant_tests.rs:537`) and the `restart` class of `test --faults` both shut the node down gracefully, which snapshots
from the store — so the WAL path is never exercised. With the WAL append removed from `write_owned_kv` entirely, all
eleven grant tests pass. That gate proves the exit-snapshot path, not the WAL, which is why F2 could exist unseen.
It is row R2, and it comes before F2's fix.

## 2 · Finding by finding

| | Review | Verified as | Documented already? | Priority here |
|---|---|---|---|---|
| F1 | raw `/gateway/rpc/call` under `mesh:write` reaches `blackboard.*.post` around `board:write` | **true, wider**: every tuple-space kind too; `/gateway/scatter` carries the same kinds under `mesh:write` (`/gateway/overlay/emit_reliable` carries them too but already requires `consensus:write`, `authz.rs:323`; `mailbox/deliver` and `shard/emit` take a kind from the request but do not reach a companion's `rpc_rx`, to be confirmed by R1's enumeration); wiki `ingest`/`access` gate on `req.sender()`, the node id, which through the raw route is the gateway itself | the mechanism (`threat-model.md:457`, closed only for protected kinds); not the scope consequence — `rbac.md:78` (line 79 on `main`, after Y12's `schedule:run` row) lists `rpc call` under `mesh:write` unqualified | **P1** |
| F2 | the `used/provider` marker goes through `append_try`; work proceeds after a dropped append | **true, narrower**: a graceful restart snapshots from the store and a poisoned writer takes a repairing snapshot, so reuse needs a dropped append *and* a hard crash before any snapshot *and* the envelope re-presented in its window | `docs/features/grants.md:128` says "buffered"; `what-is-proven.md:64` ("replayed from the WAL across a restart") and the comment at `state_machine.rs:881` say the opposite — a contradiction | **P1** as a contradiction; the window is small |
| F3 | `ReplicaSync` answers `Persisted` for a record applied but not yet enqueued | **true**; `replica_sync.rs:26-38` claims the dropped-append counter closes this window, and it cannot (the counter is bumped after the apply) — a contradiction | no | P2 on impact under the contract, not on the window's width: there is no await point inside it, and its duration is unmeasured (pre-emption can extend it); the origin polls at exactly that moment |
| F4 | an empty successful JWKS is read as a failed fetch and the old keys kept; no stale ceiling | **true, narrower**: a replace-rotation is applied; only withdraw-all is ignored. Token `exp` does *not* bound it: the holder of a compromised withdrawn key mints fresh short-lived tokens for as long as the verifier trusts the key (the reviewer's qualification, accepted) | `sso.md:61` ("a failed fetch keeps the previous keys"); not the empty-set reading or the missing ceiling | **P1** |
| F5 | the native HTTPS accept loop has no handshake timeout or admission bound | **true, wider**: `axum::serve` builds hyper without a timer, which disables hyper's 30 s header-read default on the plaintext listener too (hyper logs one warn line and returns no timeout) | the 2.32 bounds are the gossip listener's; nothing says whether they cover the gateway; the docs say to proxy, and offer `gateway_tls` as "no proxy required" | P1 for an exposed gateway |
| F6 | bulk fetches `http://` unconditionally; fails under `gateway_tls`; no fetch size cap | **true**; the reply is capped by the 10 MiB frame limit, the fetch is not; the fetch target is the sender's advertised IP and a sender-chosen port | no | P2 |
| F7 | a new journal acknowledges `OnDisk` before its directory entry is synced | **true, wider**: six `Journal::open` consumers over seven journals (`EvidenceJournal`, the rights ledger, `DurableEpochs`, the external-effects ledger, the revocation stream `src/agent/revocation.rs:316`, and the knowledge store's heads and records `src/knowledge/durable.rs:158,274`) and both companion WALs; the KV WAL is covered by the startup snapshot's directory sync | contradicts `journal.rs:59-60` ("readable after a restart"); v2.4.4's class | P2 |

Dependencies: `cargo audit` gives exactly the review's four warnings and no vulnerabilities. CI's audit job does not
`--deny warnings` and there is no `audit.toml`, so they pass by design and are recorded nowhere as accepted. Tokio's
`AsyncFd` advisory is unreachable (no use in the workspace or in any of the 533 locked crates); `backoff` and
`instant` are dev-only via `async-openai`; `rustls-pemfile` is the one in a shipped build (`tls`).

What the review did not look at, from the repository's own open list (`what-is-proven.md` § Not yet shown,
`post-360-hardening.md` §3.6): recovery from a compromised fleet CA key; revocation events and role claims signed
over untagged bytes; connection slots held by a peer that keeps talking; DNS rebinding outside the egress claim;
unsigned tally entries; unsigned release artefacts; no timeouts on the MCP-bridge and LLM-backend clients. Several
outweigh F3–F7. They stay where they are recorded; this plan is the review's seven plus the two the verification
added.

## 3 · Decisions

- **D1 (F1):** map each companion kind to its family scope on the raw routes — `blackboard.*` → `board:write`,
  `tuple.*` → `tuple:write`, `wiki.*` → `wiki:write`, read kinds to the `:read` scope — and refuse 403 naming the
  scope, rather than only documenting `mesh:write` as broad authority. *Maintainer, 2026-10-31.*
- **D2 (F2):** on a failed durable append of a grant-use marker, deny by name ("grant use not recorded durably");
  the in-memory claim stays spent. Never proceed. *Maintainer, 2026-10-31.*
- **D3 (F3):** an in-flight counter on the two inbound sites, not a change to invariant 1 (*apply, then persist*).
  The invariants page forbids enqueue-before-apply; changing that is a design decision this plan does not take.
- **D4 (order):** R1, R2+R3 and R4 before the next MINOR; the rest after it. The reviewer's reply (rev 0.2) agrees the order of work in the review was over its sample; this one is over the review plus the repository's open list, which stays where it is recorded. Docs that a finding contradicts are
  corrected in the same PR as the code, or restated as limits where the row decides not to build.

## 4 · Rows

Each row: one PR in its own worktree, every fix written fail-first against a planted fault, an independent
adversarial review before merge, the plan's status line naming the PR. The verifiers' probes are the fail-first
tests where one exists.

### R1 · Companion kinds carry their scope on the raw routes *(F1)*

**What:** every gateway door that carries an RPC kind, **in the body or the path** — `/gateway/rpc/call` and
`/gateway/scatter` under `mesh:write`; `/gateway/overlay/emit_reliable`, already under `consensus:write`;
`mailbox/deliver` and `shard/emit`, enumerated and stated; and, from #714's review, the two signal streams
(`/gateway/signal/sse/{kind}`, `/signals/{kind}`) under `mesh:read`, the mailbox stream (`/gateway/mailbox/{kind}`,
round 2) under `mesh:read` and the serve stream (`/gateway/rpc/serve/{kind}`) under `mesh:serve` — of a
companion-namespaced kind requires that companion's family scope (D1): in a body, the kind's own verb's (the
replica-sync reads `snapshot` and `wal_replay` being reads, round 2); on a signal or mailbox stream, the family's
`:read` (a tap receives every frame; a mailbox hands on a `:write`-gated delivery); on the serve stream, the family's
`:write` (serving is answering). A caller without it is refused 403 naming the scope. The wiki handlers that gate on `req.sender()` get the same treatment at the route. `rbac.md` says
which scope each companion kind needs on the raw routes, and `confined-fleet.md` names the companion kinds where it
tells operators to list protected work.

**Exit:** the verifier's probe as a test — a `mesh:write`-only token: 403 on `bb/post`, 403 on `rpc/call` of
`blackboard.*.post`, 403 on `tuple.*.put`, 403 on `scatter` of either; a `board:write` token succeeds on
`bb/post` and on `rpc/call` of the board kind — pinned in `src/agent/http/http_tests.rs` (every door, every family,
both verbs) and the companions' `tests/raw_route_scope.rs` (end to end), not in `tests/exposure_doors.rs`, because
CI runs `tests/` under `tls,a2a` and never `compliance`, so a scope test there would never execute; the wiki
ingest kind refused under `mesh:write` (the gate decides before dispatch, so a group-member gateway is not a
separate case); and, from the review: `mesh:read` alone refused the signal streams for a board kind naming
`board:read`, `mesh:read` + `board:read` receiving the frame; `mesh:serve` alone refused the serve stream for a
board kind naming `board:write`, with `board:write` a stream; and from round 2: `board:write` refused
`blackboard.board.snapshot` on `rpc/call` naming `board:read`, `tuple:write` refused `tuple.work.wal_replay` naming
`tuple:read`, and `mesh:read` alone refused the mailbox stream for a board kind naming `board:read`.

**Not built, stated:** a provider-side scope check inside the companions (the route is the door; a member's direct
`rpc_call` is not a client); and `cap:write`'s half of the serve finding — a `cap:write` client can advertise any
capability, a companion's `{ns}.primary` role included, which is that scope's meaning; what it is then sent is
gated at the serve door, so a client without the family's `:write` cannot be sent a companion's requests.

**As delivered** *(rev 0.6; the path doors from #714's review, the mailbox stream and the replica-sync reads from
its round 2)*. **Round 2:** `snapshot` (every fact on the board) and `wal_replay` (the space's items) are reads in
`companion_kind_scope` and both companions' scans — a `board:write` token with no `board:read` had read the whole
board through `rpc/call`, which the reviewer ran; the scans skip doc-comment lines; `/gateway/mailbox/{kind}` is in
`PATH_KIND_ROUTES` under the family's `:read`; `mycelium-py` 0.2.14's `on_signal` raises a refused stream as the HTTP error
(`test_on_signal_raises_a_refused_stream_as_an_http_error_not_an_sse_error`). Each seen failing first: the mapping
test and the raw-route test at `snapshot`/`wal_replay`, the streams test at the mailbox door, the pytest at the
`SSEError`. **The path doors:** `PATH_KIND_ROUTES` in
`authz.rs` — the two signal streams and the mailbox stream (`:read`) and the serve stream (`:write`), the kind read
percent-decoded as the handler's `Path` reads it; `companion_kind_family` and `companion_family_scope` in `src/agent/mod.rs`; tested by
`the_signal_streams_and_the_serve_stream_need_a_companion_kinds_family_scope` (both streams × four kinds, the
write scope not the read scope, a percent-encoded kind, an ordinary kind, the frame received under `board:read`;
the serve stream × three kinds, `board:read` not serving, `mesh:serve` still required), seen failing first with
`PATH_KIND_ROUTES` empty. The gate decides before the handler's extractor
(`the_raw_route_gate_decides_before_the_handlers_extractor`: a `text/plain` body naming a board kind is 403 where
the handler would have said 415). The gate is one function in the auth layer, `companion_kind_gate`
(`src/agent/http/authz.rs`), run after the route's own scope check with the credential's whole scope set in hand —
a handler sees only the scopes the route granted, so it could not have decided this. It reads the kind from the
body field the handler reads (`method` on `rpc/call` and `scatter`, `kind` on `signal/emit`, `mailbox/deliver`,
`shard/emit` and `overlay/emit_reliable`; `RAW_KIND_ROUTES`), maps it by `mycelium::companion_kind_scope`
(`src/agent/mod.rs`: `blackboard.*` → `board:*`, `tuple.*` → `tuple:*`, `wiki.*` → `wiki:*`; the read verbs
`read`/`depth`/`snapshot`, `depth`/`wal_replay`, `access` to `:read`, every other verb under the prefix — an unknown
one included — to `:write`), and refuses `403 {"error": "insufficient scope", "required_scope": …, "kind": …}`. The six routes:
`rpc/call`, `scatter`, `signal/emit` and `shard/emit` emit a frame of the kind that reaches the signal-handler
table an `rpc_rx` registers on, so a companion's handler could serve it; `overlay/emit_reliable` the same under
`consensus:write`; `mailbox/deliver` writes a `mailbox/` key and reaches no handler, gated alike so the list is C1's.
Each companion's tests diff the operations it serves, read from its own source, against the table
(`every_served_operation_maps_to_the_boards_own_scope`, `…_the_spaces_own_scope`, `…_the_wikis_own_scope`); the
wiki's `access`/`ingest` handlers say why they keep judging the node (`mycelium-wiki/src/agent.rs`).
**The exit, as met:** `mesh:write` alone — 403 on `bb/post`, 403 naming `board:write` on `rpc/call`, `scatter`
and the four emit routes of `blackboard.board.post`, 403 naming `tuple:write` on `tuple.work.put`; `board:write`
alone succeeds on `bb/post` and is refused the raw route naming `mesh:write`; both scopes post through `rpc/call`
and the fact lands; `tuple:write` cannot carry the board kind; the read verb under the write scope is refused
naming `board:read` — `a_board_write_through_a_raw_route_needs_board_write_as_the_boards_own_route_does`
(`mycelium-blackboard/tests/raw_route_scope.rs`),
`a_tuple_put_through_a_raw_route_needs_tuple_write_as_the_spaces_own_route_does`
(`mycelium-tuple-space/tests/raw_route_scope.rs`, both companions' test builds now carrying `compliance`),
`raw_routes_require_a_companion_kinds_family_scope` (`src/agent/http/http_tests.rs`: every route, every family,
both verbs, a body naming both fields, the `*` token, an ordinary kind) and the mapping's own test in
`src/lib_tests.rs`. The exit named `tests/exposure_doors.rs`; the pin lives beside the companions and in the
library instead, because CI runs `tests/` under `tls,a2a` and a scope test there would never execute. Each was
seen failing with `companion_kind_scope` returning `None` for every kind: the raw-only board post answered 504
in the library (nothing serves it — the gate is before dispatch) and 200 at both companions, the fact on the board
and the tuple taken. **The wiki ingest kind** is refused under `mesh:write` naming `wiki:write`
(`wiki.council.ingest` in the library test); the exit's "on a gateway that is a group member" is not a separate
test — the gate decides before any dispatch, so membership cannot change the answer, and `mycelium-wiki`'s test
build carries no scope model.

### R2 · The restart gate exercises the WAL *(the verification's finding)*

**What:** a grant test that restarts without the exit snapshot — poison the writer with a failing snapshot directory
(dropping the agent without `shutdown()` leaves the WAL's ownership lock with the writer task, so a restart in the
same process can be refused as a second owner) — and asserts the request, the grant and the `used/*` markers survive from
the WAL alone. The same shape for `an_acknowledged_replica_still_holds_the_record_after_restart`. `faults.rs`'s
`restart` class states which path it proves.

**Exit:** the new test fails with the WAL append removed from `write_owned_kv` (the plant the verification ran; the
eleventh grant test is under `a2a`, so the run needs that feature), and passes after R3.

### R3 · A grant's use is durable before the work runs *(F2, D2)*

**What:** `ae_decide` appends the `used/{door}` marker with `append_sync` and awaits it; an `Err` is a denial by
name. The same for `consume_grant` and the issuing write. `what-is-proven.md:64` and the comment at
`state_machine.rs:881` say what rung the records reach; `grants.md:128` is corrected to match.

**Exit:** with the WAL writer stopped, a protected call is refused, not executed (the review's acceptance criterion);
R2's test passes.

### R4 · An empty JWKS withdraws the keys; a stale ceiling *(F4)*

**What:** `fetch_keys` returns a typed result; a successful empty set replaces the cache with nothing; a failure
keeps the old keys for at most `JWKS_MAX_STALE` (default 24 h), after which `plan()` refuses rather than
`UseAndRefresh`. `sso.md` states both.

**Exit:** two tests — a valid empty JWKS refuses the old `kid` on the next verify; keys older than the ceiling are
refused — each seen failing first.

**As delivered** *(rev 0.4, #712)*. `fetch_keys` (`src/agent/oidc.rs`) returns `Result<Vec<_>, FetchFailed>`: `Ok` is
what the issuer published, every usable key and possibly none; `Err` is a fetch that did not reach a JWKS —
unreachable, timed out, egress-denied, **non-2xx** (`error_for_status`, new) or not a JWKS. `Ok` replaces the cache,
an empty set included; `Err` keeps the keys and marks the attempt failed. `JWKS_MAX_STALE` (24 h) is the ceiling:
`CachedKeys::past_ceiling` makes `plan()` know no `kid` and `usable()` hand out no keys once the last success is
older than it, so a verify is a fetch or a refusal, never `UseAndRefresh`; the fetch task warns each time it fails
past the ceiling. The short retry back-off now applies only while no fetch has **ever** succeeded — a successful
empty set is a withdrawal to honour, retried on the cooldown, not a blip. **Decided:** the JWKS is read key by key, and a
key whose **material** the verifier cannot read (a post-quantum key, an ES512/P-521 key, a `crv` outside
`jsonwebtoken`'s enums — one such key used to fail the whole set as a `JwkSet`) is dropped exactly like one it
cannot **build** or that lacks a string `kid`. "Cannot read" means the material, never metadata the verifier
ignores: `alg` (the algorithm is pinned from the token header), `use`, `key_ops`, `x5c`, `x5u`, `x5t` and
`x5t#S256` are stripped from each member before it is parsed, so an IdP stamping an `alg` the enum lacks or an
`x5c` in an unexpected shape does not have a usable key withdrawn by the reader (#712's review, round 2); a well-formed 2xx JWKS with no usable key is `Ok(empty)`, not `Err` — the published set is the
authority on which keys sign, so a key the issuer no longer publishes must not verify whatever else it publishes;
the warn log names how many it published and how many were usable (#712's review, finding 1). A success is a
**2xx** (`status().is_success()`, not `error_for_status`, which passes a 3xx), for the JWKS and the discovery
document alike (finding 2). The ceiling counts from the last successful fetch whether or not anything was asked in
between; a quiet node's first token after a day is refused if that one fetch fails (finding 4, fail-closed). `JWKS_MAX_STALE` stays a constant: it becomes an
`[oidc]` field when a deployment asks for a different ceiling, not before. **Not built:** nothing of the row.

### R5 · The gateway's handshake and header-read bounds *(F5)*

**What:** `serve_https` wraps `acceptor.accept` in a timeout (reusing `handshake_timeout_ms`), acquires a semaphore
sized from a new `gateway_max_connections` before spawning, and releases on shutdown; both hyper builders get a
timer so `header_read_timeout` is live on the plaintext listener too. `docs/reference/configuration.md` says which bounds are the
gossip listener's and which the gateway's.

**Exit:** a socket that connects and sends nothing is closed within the timeout on both listeners; N+1 silent
sockets do not block a real request; each seen failing first.

### R6 · Journals sync their directory entry on create *(F7)*

**What:** `Journal::open_inner` syncs the parent after creating the file (and after `create_dir_all` created a
parent); the tuple-space store and the blackboard WAL do the same. `journal.rs:59`'s guarantee holds as written.

**Exit:** an ordering witness — the journal's syncs go through `std::fs` directly, not the sim seam, so the witness is
a test-only hook that records each `sync_data`/`fsync_parent` call in order, asserting the parent sync precedes the
first append on a fresh journal — for the journal and both companion WALs; and, as `snapshot_install_syncs_the_directory`
does, a real-directory check that the sync is wired. The power-loss property itself is not observable without a
filesystem fault adapter; stated.

### R7 · Bulk under `gateway_tls`, with a fetch cap *(F6)*

**What:** the bulk fetch scheme follows `gateway_tls` and the client trusts the fleet CA (or the operator's); a node
with `gateway_tls` set and no usable bulk trust refuses `start()` by name, as R8 of the realignment plan does for
`http_port` without `gateway`; a `max_bulk_payload_bytes` cap on the fetch. `gateway-tls.md` says bulk is covered.

**Exit:** a `gateway_tls` node completes a `bulk_call`; a fetch over the cap is refused; each seen failing first.

**Not built, stated:** payload integrity beyond TLS (no per-payload signature); the fetch target remains the sender's
advertised address.

### R8 · A replica-sync answer covers the apply-to-append window *(F3, D3)*

**What:** an `inbound_in_flight` counter bumped before `apply_inbound` and dropped after the append call returns on
both inbound sites; `answer()` returns `Failed` while it is non-zero. `replica_sync.rs`'s module doc says what the
counter does and does not cover.

**Exit:** the verifier's probe inverted — a record applied but not appended answers `Failed`, and `Persisted` once
appended.

### R9 · The audit warnings are accepted in the open *(dependencies)*

**What:** `audit.toml` with a dated ignore and reason for each warning CI passes; `rustls-pemfile` replaced by
`rustls-pki-types`' PEM parsing under `tls`; Tokio bumped to 1.53.3 or later (1.52.x has no patched release); the stale `bincode` comment in `ci.yml`
removed. `what-is-proven.md` gets one line naming the accepted warnings.

**Exit:** `cargo audit` reports no warning that `audit.toml` does not name with a reason; `cargo audit` with the
ignores removed shows only the dev-only pair. *Corrected at rev 0.8:* the dev-only pair (`backoff`, `instant`) left the
tree with `async-openai`, so nothing ignores it; the one warning left is `examples/conway-gpu`'s `paste`
RUSTSEC-2024-0436, ignored on that lockfile's line of the `audit` job rather than in an `audit.toml` (§5).

## 5 · Evidence

Filled as rows close: each row's promises quoted beside the code and test that deliver them, or *not built*.

| Row | Delivered | PR |
|---|---|---|
| R2 | *"a grant test that restarts without the exit snapshot … and asserts the request, the grant and the `used/*` markers survive from the WAL alone"*: `a_restart_that_keeps_only_the_wal_replays_the_request_the_grant_and_the_door_marker` (`src/agent/http/grant_tests.rs`) — the exit snapshot's install target is made a directory, so its rename refuses and the WAL is not truncated (`test_util::sabotage_exit_snapshot`; the writer's ownership lock is released as usual, no drop without `shutdown()`); the start snapshot is put back and the restart replays start snapshot + WAL tail; it asserts the request, `issued`, `consumed` and `used/provider` are all back. *"the same shape for `an_acknowledged_replica_still_holds_the_record_after_restart`"*: `an_acknowledged_replica_still_holds_the_record_after_a_restart_that_keeps_only_the_wal` under `Async` (the gate's own mode, a try-send append) and `…_under_flush` (`src/agent/kv_handle_tests.rs`) — the origin is stopped before the restart and the WAL's bytes are checked for the key, so nothing but the WAL can supply it. *"`faults.rs`'s `restart` class states which path it proves"*: the module doc of `faults::restart` and the report lines of `restart` and `grant` — the exit-snapshot path; a hard-crash restart class is *not built*. **Exit met:** with the WAL append removed from `write_owned_kv`, under `tls,compliance,a2a,test-util`, the twelve older grant tests pass (the graceful restart among them) and the new one fails on the request; with every inbound append in `connection.rs` removed (three sites — a plant on the `Data` arm alone did not fail it, since a KV frame arrives through the `SignedData` arm), the older replica gate passes and the WAL-only one fails. Plants restored by copy and `touch`. | #713 |
| R3 | *"`ae_decide` appends the `used/{door}` marker with `append_sync` and awaits it; an `Err` is a denial by name"*: `state_machine::write_owned_kv_durable` (the receipt path's `kv_set_requiring_sync` — on the WAL and fsynced before it is applied or gossiped; `Ok` in memory only on a node with no `[persistence]`, stated on the feature page) called from `ae_decide`'s grant branch (`src/agent/http/authz.rs`); `Err` → `Decision::deny("grant use not recorded durably at {door}: {reason}")`, the history permit retracted, and the claim **released** (`ExecutionAuthority::release_grant_use`): nothing ran, so the use is not spent, and a retry after repair runs once — a restart answers the same (#713's review, finding 3). *"The same for `consume_grant` and the issuing write"*: `consume_grant` is durable **before** the commit and refuses `GrantNotPresented` naming it, the grant left un-consumed (a lost `consumed` re-honoured the grant — `await_grant` re-reads the un-consumed request's `issued`; #713's review, finding 2); the issue-time checkpoint and a revocation's checkpoint and `revoked` records go through `grants::write_record_durable` and the routes answer `503 not_durable` (finding 1); `issued` and `refused` go through `write_record_durable` too (round 2), the issue guarded first by its checkpoint — a plant of `issued` back on `write_record` cannot be told from the checkpoint's refusal without a seam, so `issued` is stated durable by the same path and `refused` is the one planted; `revoked` alone is likewise stated, not shown — with the checkpoint durable, a plant of `revoked` back on `write_record` passes, because the checkpoint's refusal fires first and after the WAL-only restart the seq-2 checkpoint alone refuses the grant through `feed_grant_checkpoint` (round 4); the decide arms count `issued`/`refused` and move the pending gauge only on `Ok`, as the revoke arm does (round 4); the request stays `append_try` — losing the request fails safe, the agent asks again. **Residual, stated (§77):** at `/mcp` a use is spent at the door that admitted it — the gateway's `used/gateway` landing and the provider's write failing leaves the gateway's use spent while a direct call at the provider runs; one execution at most. *"`what-is-proven.md:64` and the comment at `state_machine.rs:881` say what rung the records reach; `grants.md:128` is corrected"*: all three, plus `runtime-invariants.md`'s write-site list, `contracts-receipts.md` §2.2's user list, guide 21, `deprecations.md` §77, the CHANGELOG and a dated `.log/` entry. **Rungs:** request — buffered (`append_try`); `issued` — receipt (acked, its rung reported); `consumed`, `used/{door}`, `checkpoint`, `revoked` — on disk before the step they admit, or that step refused. **Exit met:** `a_protected_call_is_refused_when_its_grant_use_cannot_be_made_durable` (the writer poisoned: refused by name, nothing ran, the denial on the record; the writer repaired by `trigger_snapshot`: the retry runs once and the second presentation is `GrantUsed`); `a_consumed_marker_the_disk_will_not_take_refuses_the_invocation_and_leaves_the_grant_unconsumed`; `a_revocation_the_disk_will_not_take_is_refused_and_the_retried_one_survives_a_wal_only_restart`. Plants: the use marker back on `append_try` (the call ran); the release removed (the post-repair retry answered `GrantUsed`); `consumed` back on `append_try` (the first call ran); the revocation back on `kv_set_with_receipt` (the poisoned revoke answered `200` — the pair; `revoked` alone is not shown). R2's test passes with R3 in place. | #713 |
| R4 | *a typed result* — `fetch_keys` → `Result<Vec<_>, FetchFailed>` (`src/agent/oidc.rs`); *a successful empty set replaces the cache with nothing* — the fetch task's `Ok(keys)` arm, `a_successful_empty_jwks_withdraws_the_keys` (failed on the old code: the withdrawn key still verified); *a failure keeps the old keys for at most `JWKS_MAX_STALE`, after which `plan()` refuses rather than `UseAndRefresh`* — `CachedKeys::past_ceiling`/`usable`, `keys_older_than_the_stale_ceiling_are_refused` (failed on the old code: keys of any age verified); a non-2xx answer is a failure — `a_non_2xx_jwks_is_a_failure_not_a_withdrawal` (passes on the old code, where a 503's empty body read as a failure; fails with `error_for_status` planted out); a replace-rotation withdraws the old key — `a_rotated_key_is_accepted_after_the_cooldown`'s new last assertions (a pin, not a proof: a non-empty fetch already replaced the cache before this row); a key the parser cannot read is dropped, not the set — `a_key_the_parser_cannot_read_is_dropped_not_the_whole_set` (failed first: the set failed as a whole); a 3xx is a failure — `a_3xx_jwks_is_a_failure_not_a_withdrawal` (failed first); the discovery document only from a 2xx — `a_non_2xx_discovery_document_is_not_read` (failed first); metadata the verifier ignores does not drop a key — `metadata_the_verifier_ignores_does_not_drop_a_usable_key` (failed first: the fixture's RSA key under `"alg":"ML-DSA-65"` with a string `x5c` was dropped); a non-string `kid` stays dropped — `a_kid_that_is_not_a_string_is_dropped`; *`sso.md` states both* — § `jwks_uri`; `deprecations.md` §75 | #712 |
| R1 | *every raw route that takes an RPC kind … requires that companion's family scope* — `companion_kind_gate` (`authz.rs`), `companion_kind_scope` (`agent/mod.rs`), six routes; *refused 403 naming the scope* — the refusal body; *the wiki handlers … the same treatment at the route* — the gate, and the handlers' comments; *`rbac.md` says which scope* — the `mesh:write`, `board:*`, `tuple:*`, `wiki:*` rows; *`confined-fleet.md` names the companion kinds* — § the raw routes; *the signal streams … `:read`; the serve stream … `:write`* — `PATH_KIND_ROUTES`, `companion_kind_family`; `deprecations.md` §76; `rbac.md`'s `mesh:read`/`mesh:write`, `mesh:serve` and `cap:write` rows. Exit: the four tests named in the row, each seen failing first. Not built: a scope check inside the companions; `cap:write` advertising a companion role. | #714 |
| R5 | *"`serve_https` wraps `acceptor.accept` in a timeout (reusing `handshake_timeout_ms`)"*: `serve_https` is gone; both gateway listeners run one accept loop, `http::listener::serve` (`src/agent/http/listener.rs:120`, spawned from `prepare_gateway`, `src/agent/http/mod.rs:520`), and the TLS handshake runs under `tokio::time::timeout(bounds.preface, acceptor.accept(tcp))` (`listener.rs:178`). The request's preface has the same bound, because hyper's header timer starts only after hyper-util has sniffed the protocol (`PrefaceDeadline`, `listener.rs:242–293`). On HTTP/1 that is the first byte; on HTTP/2 it is the 24-byte preface plus the next frame's 9-byte header (#719's review, finding 1). *"acquires a semaphore sized from a new `gateway_max_connections` before spawning"*: the permit is taken before accept (`listener.rs:130–142`); `gateway_max_connections` defaults to 1024, `validate()` accepts `1..=65535`, and the env var is `GOSSIP_GATEWAY_MAX_CONNECTIONS` (`mycelium-core/src/config.rs:1505`, `:2414`, `:2786`). *"and releases on shutdown"*: the connections are held in a `JoinSet` (`listener.rs:131`). On shutdown a connection still in its handshake or preface is dropped, the rest shut down gracefully, and the node's drain abort drops them all. *"both hyper builders get a timer so `header_read_timeout` is live on the plaintext listener too"*: `TokioTimer` on HTTP/1 and HTTP/2 (`listener.rs:217`, `:222`). HTTP/2 also gets a keep-alive PING whose interval and timeout are `h2_keep_alive(handshake_timeout_ms)`, never under 1 s (`listener.rs:74`, `:224–225`; review round 2). *"`configuration.md` says which bounds are the gossip listener's and which the gateway's"*: the `handshake_timeout_ms` row (both listeners), the `gateway_max_connections` and `max_connections` rows, and the rows marked gossip-listener only (`max_connections_per_source`, `tcp_accept_backlog`, `inbound_idle_timeout_secs`, `peer_read_stall_timeout_ms`, `peer_min_rate_bytes_per_sec`) in `docs/reference/configuration.md`; also `gateway-tls.md` and `deprecations.md` §78. Beyond the row, from the review: accept errors back off for 1 s instead of spinning (`accept_error_backoff`, `listener.rs:98`), and `SystemStats` gained `gateway_connections_timed_out`, `gateway_connection_cap_waits` and `gateway_connections_open` (`src/agent/mod.rs:559–567`, with metrics). **Exit met:** *"a socket that connects and sends nothing is closed within the timeout on both listeners"*: `a_silent_socket_is_closed_within_the_handshake_timeout_on_the_plaintext_gateway`, `…_on_the_tls_gateway` and `a_socket_silent_after_the_tls_handshake_is_closed` (`src/agent/http/listener_tests.rs`). *"N+1 silent sockets do not block a real request"*: `n_plus_one_silent_sockets_do_not_block_a_real_request_on_the_plaintext_gateway` and `…_on_the_tls_gateway`. *"each seen failing first"*: on the unfixed code, seven of the first eleven tests failed: both silent sockets, the partial HTTP/2 preface, silence after the TLS handshake, the cap on each listener, and shutdown during the TLS handshake. The two N+1 tests pass on the old code, which had no cap and so blocked nothing; a plant of the cap without its deadline fails both. Round 1's `a_bare_http2_preface_then_silence_does_not_hold_the_only_permit` and `an_http2_client_silent_after_its_settings_is_closed_by_the_keep_alive` failed on the first head. `an_accept_loop_out_of_descriptors_backs_off` failed by plant (31 160 accepts in 500 ms), and `the_http2_keep_alive_has_a_one_second_floor` failed with the floor removed. **Not built, stated (§78, `configuration.md`):** a connection that keeps talking holds its permit, such as an HTTP/2 client that answers PINGs and sends no request, or a long SSE stream. Enough long-lived streams fill the cap, and `/health` and `/ready` then wait, because no slot is reserved for the probes. The gateway has no per-source cap. | #719 |
| R6 | *"`Journal::open_inner` syncs the parent after creating the file (and after `create_dir_all` created a parent)"*: `open_inner` calls `create_dir_all_synced_observed` on the parent chain and `fsync_parent` once the file exists, on every open and before the writer is spawned (`src/agent/journal.rs:199`, `:206`). The helpers are `mycelium_core::persistence::create_dir_all_synced`, `create_dir_all_synced_observed`, `sync_parent_dir` and `sync_parent_dir_observed` (`mycelium-core/src/persistence.rs:726`, `:742`, `:800`, `:806`), re-exported from `mycelium`. From #718's first review round, every path component's holder is synced on every call, not only directories this call created, so an earlier failed sync or a concurrent opener cannot leave an ancestor unsynced. These six consumers share the path: the evidence journal, the rights ledger, the revocation journal, `DurableEpochs`, the knowledge journals and the external-effects ledger. *"the tuple-space store and the blackboard WAL do the same"*: `mycelium-tuple-space/src/store.rs:1166`, `:1224` and `mycelium-blackboard/src/wal.rs:190`, `:226`, on every open. Beyond the row: `start()` creates `{base_path}/{node}/kv/` through the helper on every start (`src/agent/lifecycle.rs:318`). *"`journal.rs:59`'s guarantee holds as written"*: the guarantee now sits at `journal.rs:68`, and § *A durable name* (`journal.rs:52–59`) says what makes it true. **The pass-over (review round 2):** an ancestor that existed before the call is passed over when its sync answers `EINVAL`, `ENOTSUP` or `PermissionDenied` (`passes_over`, `persistence.rs:777`). A filesystem without directory fsync is one case, such as a squashfs root under a writable data mount. Each pass-over is logged once and counted: `dir_syncs_passed_over()` (`persistence.rs:734`), `SystemStats::dir_syncs_passed_over` (`src/agent/mod.rs:624`) and `gossip_dir_syncs_passed_over_total`. A directory the call created, the file's own directory and every other error still refuse, by name. §79 states the exposure: a pre-existing directory on the data filesystem whose daemon answers `EINVAL` only sometimes. **Exit met:** *"an ordering witness … asserting the parent sync precedes the first append on a fresh journal"*: `a_fresh_journal_syncs_its_directory_before_the_first_append_is_acknowledged`, `a_journal_in_new_directories_syncs_each_one_it_created`, `a_journal_in_existing_unsynced_directories_syncs_their_holders` and `reopening_a_journal_syncs_its_directory_and_creates_none` (`src/agent/journal_repair_tests.rs`). These run on a trace the journal reports from inside the sync helper once the sync has returned (`Journal::open_traced`, `journal.rs:357`; #718's review, finding 1: the first trace recorded a label beside the sync). *"for the journal and both companion WALs"*: `a_fresh_wal_syncs_its_directories_before_the_first_record` and `reopening_a_wal_syncs_its_directory`, in `mycelium-tuple-space/src/store.rs` and `mycelium-blackboard/src/store.rs`. *"a real-directory check that the sync is wired"*: `create_dir_all_synced_syncs_every_component_outermost_first`, `create_dir_all_synced_syncs_the_holder_of_an_existing_unsynced_directory`, `an_existing_ancestor_without_directory_fsync_is_passed_over` and `a_created_directory_whose_holder_cannot_sync_refuses` (`mycelium-core/src/persistence.rs:2069–2114`). Each witness failed on the unfixed code: the journal's trace held only `append.sync`, and the companions' held nothing. Plants: the journal's open-time `fsync_parent` deleted fails 3 of the 11 journal repair tests; the core helper skipping existing directories fails 3 journal tests and both companions' reopen tests. Each was restored by copy and `touch`. **Not built, stated:** the power-loss property itself, which needs a filesystem fault adapter; no test observes the syncs of `start()`'s call site on a started node; a relative path is synced only up to the working directory. | #718 |
| R7 | *"the bulk fetch scheme follows `gateway_tls`"*: a caller whose gateway serves TLS marks its `invoke.bulk` ticket with `0xFF` and a flags byte, bit 0 meaning https (`encode_ticket`/`decode_ticket`, `src/agent/bulk.rs:106`, `:122`); the responder builds its URL from the ticket (`fetch_url`, `bulk.rs:495`). A plaintext caller's ticket is byte-for-byte the 2.39.0 layout. *"and the client trusts the fleet CA (or the operator's)"*: `BulkServerVerifier` (`bulk.rs:752`) takes the fleet check (the CA chain, removal and revocation; `NodeTls::fleet_server_verifier`) together with `gateway_tls.bulk_ca_pem_path` (`mycelium-core/src/config.rs:163`). It falls through to the operator roots only on a chain refusal, never on a removal or revocation (review round 1). `install_bulk_trust` sets this up once the identity is loaded (`bulk.rs:698`, `lifecycle.rs:571`), and `bulk_tls_client_config` builds the client (`bulk.rs:723`). The client pools no connections, resumes no TLS session and ignores the environment's proxy (`.no_proxy()`, `bulk.rs:239`, `:287`). A node-certificate gateway presents its live certificate after a rotation (`LiveGatewayCert`, `mycelium-core/src/tls.rs:1102`). *"a node with `gateway_tls` set and no usable bulk trust refuses `start()` by name"*: `check_bulk_trust` (`bulk.rs:604`, from `preflight`, `lifecycle.rs:141`, and so from `mycelium doctor`) refuses with `gateway_tls.bulk_ca_pem_path` named in four cases. The first three: an operator certificate with no `bulk_ca_pem_path`; a `bulk_ca_pem_path` that cannot be read or holds no certificate; an operator certificate that does not verify against it for the node's own IP. The fourth is a fleet-issued gateway certificate that is not a mesh certificate (round 2). The check applies only when `http_port` is set. *"a `max_bulk_payload_bytes` cap on the fetch"*: default 64 MiB, and `validate()` accepts 1 to 16 GiB (`config.rs:1395`, `:2135`). `bulk_call` refuses a larger payload before staging, `BulkError::PayloadTooLarge` (`bulk.rs:437`). The fetch refuses a declared `Content-Length` over the cap and stops reading a streamed body once it passes the cap (`BulkTransport::fetch`, `bulk.rs:327–347`). A status other than 2xx is not a payload. *"`gateway-tls.md` says bulk is covered"*: `docs/operations/gateway-tls.md:70`, plus `configuration.md`'s `gateway_tls` and `max_bulk_payload_bytes` rows and `deprecations.md` §80. **Exit met:** *"a `gateway_tls` node completes a `bulk_call`"*: `a_bulk_call_completes_between_two_gateway_tls_nodes` (`src/agent/bulk.rs`) failed on the unfixed code (the call timed out) while its plaintext control passed. The rest: `a_mixed_fleet_of_node_and_operator_certificates_completes_bulk_both_ways`, `a_bulk_call_after_rotate_and_revoke_presents_the_new_certificate` (`src/lib_tests.rs`) and `a_bulk_fetch_ignores_the_environment_proxy` (`tests/bulk_no_proxy.rs`). *"a fetch over the cap is refused"*: `a_fetch_over_the_cap_is_refused_declared_or_streamed`, `a_streamed_fetch_stops_reading_at_the_cap`, `a_declared_length_over_the_cap_is_refused_before_the_body_is_read` and `a_bulk_call_over_the_cap_is_refused_before_it_is_staged`. *"each seen failing first"*: four plants run together (the fetch cap off, the caller's cap off, the start refusal off, the URL always `http`) failed seven tests. Round 1's new tests failed on the first head or by plant: the rotation test failed with the gateway put back on its start-time certificate and resumption off. Round 2's revocation-after-fetch test (`a_fetch_after_the_callers_key_is_revoked_is_refused_on_a_client_that_fetched_before`) failed with resumption re-enabled, and separately with the idle pool re-enabled. Round 3's proxy test failed with `.no_proxy()` removed. **Not built, stated (`gateway-tls.md:104–121`, §80):** payload integrity beyond TLS (no per-payload signature). The fetch target remains the sender's advertised address, taken as given: a node advertising `0.0.0.0` or `::` cannot be fetched from and is not refused for it. A plaintext gateway still carries bulk in the clear. A TLS-terminating proxy in front of a plaintext gateway is bypassed for bulk. A bulk trust setting separate from the node's own `gateway_tls` is not built either: `bulk_ca_pem_path` lives there, so setting it also makes the node's own tickets `https`. A responder older than R7 drops an `https` ticket. | #722 |
| R8 | *"an `inbound_in_flight` counter bumped before `apply_inbound` and dropped after the append call returns on both inbound sites"*: the sites are three, not two: the `Data` arm, the `SignedData` arm and the anti-entropy `StateResponse` entry. All three go through one helper, `apply_inbound_then_append` (`mycelium-core/src/connection.rs:170`, called at `:868`, `:1064`, `:1258`). It opens `WalHandle::begin_inbound` (`mycelium-core/src/persistence.rs:209`) before the apply and closes it in `append_inbound` (`persistence.rs:230`) once the record is **on the WAL channel**, not when the append call returns. This was #720's review, finding 3: `answer()`'s sync is a `WalMsg::Sync` on the same channel, served in order by the one writer loop, so enqueue suffices. A guard dropped without its append (a cancelled task, a panic) counts a dropped append (`InboundApply`, `persistence.rs:174`). *"`answer()` returns `Failed` while it is non-zero"*: `replica_sync::answer` (`src/agent/replica_sync.rs:175`, the check at `:215`) loads the in-flight count (Acquire) before the dropped-append count. That order is round 1's finding 1: in the reverse order, an abandoned window could read as `Persisted`. The count is visible on `/health` as `persistence.inbound_in_flight` (`src/agent/http/node.rs:43`) and as the process-global gauge `gossip_wal_inbound_in_flight`. *"`replica_sync.rs`'s module doc says what the counter does and does not cover"*: `replica_sync.rs:41–60`. It covers the three inbound sites. It does not count per record (one number for the node), does not cover local writes (a receipt never asks the origin), and does not promise more than `fdatasync`. **Exit met:** *"a record applied but not appended answers `Failed`, and `Persisted` once appended"*: `a_record_applied_but_not_appended_is_not_reported_persisted` and `…_under_flush` (`src/agent/kv_handle_tests.rs:504`, `:508`), on two live nodes, with B paused between apply and append by `WalHandle::pause_inbound_after_apply` (`persistence.rs:280`, `test-support`). Both failed on the code without the counter (`Persisted` while paused), and with the guard planted out of the helper. `an_append_dropped_between_the_answers_two_reads_is_not_reported_persisted` (`kv_handle_tests.rs:600`, via `pause_answer_between_counts`) answered `Persisted` on the first head. `the_inbound_window_closes_at_enqueue_not_at_the_flush_ack` (`connection.rs:1306`) failed on the first head. `the_inbound_window_closes_on_every_exit_and_an_abandoned_one_is_a_dropped_append` (`persistence.rs:1680`) covers completion, abandonment and a panic. **Not built, stated (§81, the module doc):** per-record durability tracking, since a busy peer answers `Failed` for any record while any inbound record is in flight; the apply-to-append window of a node's own local writes; and a guard abandoned at shutdown counts as dropped even if its record was enqueued, which fails safe. | #720 |
| R9 | *"`audit.toml` with a dated ignore and reason for each warning CI passes"*: delivered without `audit.toml`. `.cargo/audit.toml` was read for every lockfile, so it was removed in #721's review round 1. The `audit` job runs `cargo audit --deny warnings` on every tracked lockfile, and a lockfile accepts a warning only on its own line with a date and a reason (`.github/workflows/ci.yml:1082–1101`). The one accepted warning is `examples/conway-gpu`'s `paste` RUSTSEC-2024-0436 (`--ignore`, `ci.yml:1099`): unmaintained, reached only through wgpu's Metal backend in an excluded demo crate, and a build-time macro. The root and fuzz lockfiles accept nothing, so a `paste` reaching a shipped lockfile fails. *"`rustls-pemfile` replaced by `rustls-pki-types`' PEM parsing under `tls`"*: `pem_certs` and `pem_private_key` on `PemObject` (`mycelium-core/src/tls.rs:1272`, `:1279`); `git grep rustls_pemfile` in `*.rs` and `*.toml` is empty. *"Tokio bumped to 1.53.3 or later"*: tokio is 1.53.3 in all three lockfiles, and every manifest requires `tokio = "1.53.3"`, so an adopter cannot resolve below the patched release (round 1; the vendored a2a probe under `docs/plans/external/` is left as vendored; `deprecations.md` §82). *"the stale `bincode` comment in `ci.yml` removed"*: replaced by the job's comment at `ci.yml:1082`. *"`what-is-proven.md` gets one line naming the accepted warnings"*: `docs/operations/what-is-proven.md:59`, plus `production-readiness.md:293–295` and `examples/conway-gpu/README.md`. Beyond the row: `async-openai`, an unused dev-dependency, was removed, and `backoff` and `instant` went with it (`Cargo.toml:189`). The review also added `cargo clippy -p mycelium-core --lib --tests --features tls -- -D warnings`, a line no gate had run, to `make check` and CI (`Makefile:178`, `ci.yml:209`); it found three errors. **Exit met, as corrected:** *"`cargo audit` reports no warning that `audit.toml` does not name with a reason"*: `cargo audit --deny warnings`, run as the job runs it, exits 0 on all three lockfiles (#721's body quotes the run). *"with the ignores removed shows only the dev-only pair"*: that pair is out of the tree, not ignored. With its `--ignore` removed, the conway-gpu lockfile fails on `paste` RUSTSEC-2024-0436 alone (`error: 1 denied warning found!`). The PEM readers: `tls::pem_parse_tests` (`tls.rs:1453`, eight cases). The first five passed on `rustls-pemfile` before the swap, and all eight pass on `rustls-pki-types`. The plant "`pem_private_key` maps every error to `Ok(None)`" fails `a_malformed_key_section_is_an_error_naming_the_parse` alone. **Not built, stated:** one PEM reader for the whole tree, since `load_key_from_pkcs8_pem` still reads a node's own key as PKCS#8 only, and `rcgen` and `reqwest` parse PEM themselves. A newly published advisory turns the job red without a code change, which is intended. | #721 |

**Follow-ups recorded at rev 0.8, none of them a row's promise:**
- `mycelium-reason` and `mycelium-guardrails` carry R9's Tokio floor (`tokio = "1.53.3"`) on `main` and note it in their
  CHANGELOGs' `[Unreleased]`. Each needs a version bump at its next release, or an adopter of the published crate keeps
  the old floor.
- `mycelium-core`'s rustdoc has unresolved intra-doc links: 35 of them across the crate (`error.rs:6–13`'s module doc
  holds 11, the error variants and `GossipConfig::validate`; the rest are in `kv_handle.rs`, `signal.rs`, `store.rs`,
  `framing.rs`, `schema_handle.rs`, `locality.rs` and `ops.rs`), counted by `RUSTDOCFLAGS='-D
  rustdoc::broken_intra_doc_links' cargo doc -p mycelium-core --no-deps` on `6e491b50`. They predate this plan, and no
  CI step or `make` target runs rustdoc.
