# The external code security review of 2026-10-31 — verification and repairs

**Status:** agreed with the maintainer on 2026-10-31, rev 0.4 (rev 0.2 takes the reviewer's two qualifications of the verification, 2026-10-31: F4 is P1 regardless of token lifetime, and F3's window is unmeasured; rev 0.3 takes the independent review of this plan's PR, #711: the journal consumers re-enumerated, R6's exit restated, R2 led by the poisoned writer, the raw routes enumerated; rev 0.4 records R1 delivered, 2026-10-31). Nine rows; **R1 delivered** (#714); R2+R3 and R4 started. The review is vendored at
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
(`/gateway/signal/sse/{kind}`, `/signals/{kind}`) under `mesh:read` and the serve stream (`/gateway/rpc/serve/{kind}`)
under `mesh:serve` — of a companion-namespaced kind requires that companion's family scope (D1): in a body, the
kind's own verb's; on a signal stream, the family's `:read` (a tap receives every frame); on the serve stream, the
family's `:write` (serving is answering). A caller without it is refused 403 naming the scope. The wiki handlers that gate on `req.sender()` get the same treatment at the route. `rbac.md` says
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
board kind naming `board:write`, with `board:write` a stream.

**Not built, stated:** a provider-side scope check inside the companions (the route is the door; a member's direct
`rpc_call` is not a client); and `cap:write`'s half of the serve finding — a `cap:write` client can advertise any
capability, a companion's `{ns}.primary` role included, which is that scope's meaning; what it is then sent is
gated at the serve door, so a client without the family's `:write` cannot be sent a companion's requests.

**As delivered** *(rev 0.4; the path doors from #714's review)*. **The path doors:** `PATH_KIND_ROUTES` in
`authz.rs` — the two signal streams (`:read`) and the serve stream (`:write`), the kind read percent-decoded as the
handler's `Path` reads it; `companion_kind_family` and `companion_family_scope` in `src/agent/mod.rs`; tested by
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
`read`/`depth`, `depth`, `access` to `:read`, every other verb under the prefix — an unknown one included — to
`:write`), and refuses `403 {"error": "insufficient scope", "required_scope": …, "kind": …}`. The six routes:
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
ignores removed shows only the dev-only pair.

## 5 · Evidence

Filled as rows close: each row's promises quoted beside the code and test that deliver them, or *not built*.

| Row | Delivered | PR |
|---|---|---|
| R1 | *every raw route that takes an RPC kind … requires that companion's family scope* — `companion_kind_gate` (`authz.rs`), `companion_kind_scope` (`agent/mod.rs`), six routes; *refused 403 naming the scope* — the refusal body; *the wiki handlers … the same treatment at the route* — the gate, and the handlers' comments; *`rbac.md` says which scope* — the `mesh:write`, `board:*`, `tuple:*`, `wiki:*` rows; *`confined-fleet.md` names the companion kinds* — § the raw routes; *the signal streams … `:read`; the serve stream … `:write`* — `PATH_KIND_ROUTES`, `companion_kind_family`; `deprecations.md` §76; `rbac.md`'s `mesh:read`/`mesh:write`, `mesh:serve` and `cap:write` rows. Exit: the four tests named in the row, each seen failing first. Not built: a scope check inside the companions; `cap:write` advertising a companion role. | #714 |
