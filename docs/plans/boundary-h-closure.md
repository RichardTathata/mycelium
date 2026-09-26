# Boundary H closure: authority at every door (implementation plan)

**Status:** draft rev 0.3, 2026-09-25. Rev 0.2 folded in an external review of #402; rev 0.3 closes the gaps a
re-read found in rev 0.2's own answers (§7). Follows [`boundary-h.md`](boundary-h.md) (rev 0.5), whose items have all
shipped except K3b, K3c, H4 and D. This plan closes what the A1 wiring (#399 gateway, #402 wiki store) left open,
and answers one question with evidence: **once an agent's authority is revoked, can it still act?**

> Posture, once: an authority check at one door is only as strong as the doors that do not have it. This plan
> finds every door by reading the code, closes each at the point where the work happens, and gates the result
> with one test that tries them all.

---

## 1. What verifying the code found (2026-09-25)

Each finding was checked in the code, not inferred from the docs.

| # | Finding | Evidence |
|---|---|---|
| F1 | **Providers check who is calling, never whether they may.** The MCP tool serve loop resolves the principal and runs the handler; there is no mandate, revocation or budget check. The generic serve receiver (`RpcRequestRx`) and the SDK serve route (`/gateway/rpc/serve`) are the same: they verify the caller context and hand the request on. Authorising the principal "stays the loop's job" (`request_authorized`, a static allowlist, opt-in, feature `compliance`) | `src/agent/mcp.rs:73` (`run_mcp_tool_task`); `src/agent/rpc.rs:84` (`RpcRequestRx`); `src/agent/http.rs` `gw_rpc_serve`; `src/agent/gateway_caller.rs:777` |
| F2 | **A member can reach any provider without the gateway.** `agent.service().rpc_call(provider, MCP_INVOKE, …)` is public API; the MCP tests use exactly this. So in a fleet of full nodes, revoking an agent's mandate stops it at the gateway only | `src/agent/service_handle.rs` `rpc_call`; `src/agent/mcp.rs:406` |
| F3 | **The confined profile has a hole: `/gateway/rpc/call`.** It dispatches any `method` to any `target`, including `mcp.invoke` and `skill.invoke`, framed with the client's principal, so the provider accepts it. `ae_preflight` never runs. The route needs only `mesh:write`, and **an SDK agent that serves skills needs `mesh:write` for `/gateway/rpc/respond`**, so the scope cannot simply be withheld | `src/agent/http.rs` `gw_rpc_call` (≈ line 2022); scope table ≈ line 698–700 |
| F4 | `/gateway/signal/emit` is **not** a hole. A secure-profile gateway promises an envelope on every RPC it sends, so a bare RPC-shaped signal emitted through it is refused at the provider (`CallerError::Missing`), and a client-supplied frame is refused at emission (`carries_caller_frame`) | `src/agent/gateway_caller.rs:36–52`, `:289`; `gw_signal_emit` |
| F5 | **H6 is a primitive, not a mechanism.** `CohortBudget` (#394) has no caller outside its own module and tests. No provider admits against it | `grep CohortBudget` → only `src/knowledge/cohort_budget.rs` |
| F6 | **The mandate stops at the gateway.** The gateway forwards `{name, arguments}` to the provider, not `_meta.mandate`; the provider could not check a mandate even if it wanted to | `src/agent/http.rs` `tools/call` dispatch (`tool_req`) |
| F7 | **An operator cannot cut a member off.** Identity-key revocation lets a node revoke only **its own** key, and it affects signature verification, not whether RPCs are accepted. At the transport, any certificate chaining to the fleet CA is a member: no revocation list or deny-list was found. Removing a member today means rotating the CA | `src/agent/revocation.rs` (module docs, "Validation rule"); `mycelium-core/src/tls.rs` |
| F8 | The wiki's `FsStore` has neither the mandate fence nor A1. No deployment uses it with authority on the line: the council wiki uses `GitStore`; NovusLens keeps its own Postgres store behind the wiki contract | `mycelium-wiki/src/fs.rs`; Novus-i2 `apps/meaning`, `config/pins.md` |
| F9 | **The wiki store checks authority before the git transaction, not inside it.** Normally milliseconds apart, but normal latency is not a safety bound: a process that pauses between the two writes after expiry or revocation, by the pause's length. The fence catches only a moved appointment ref. *(rev 0.1 called this "not a gap"; the external review was right, and it is now pinned by a test)* | `mycelium-wiki/src/git_store.rs` `commit_files`; `a_pause_after_the_check_is_not_caught_locally` |
| F10 | **A restart can restore revoked authority.** The revocation view is in memory; after a restart an old checkpoint issued before the revocation, and still fresh, is accepted, and the revoked term reads as not revoked, for up to *F* after the revocation | `src/mandate/authority.rs` `RevocationView` |
| F11 | **Admitted work is checked, not stopped.** A1 re-checks at admission, dequeue, retry and re-authorisation, and `StopContract`/`drain_report` model a stop. Nothing cancels a handler already running when its authority is revoked | `src/mandate/authority.rs` (`StopContract` is a model) |
| F12 | **A restart also forgets epochs.** The resource's installed epoch and the grant verifier's highest verified epoch per scope are in memory. After a restart they fall back to what is configured, so a mandate from a superseded epoch passes again until an operator reinstalls the newer one | `src/mandate.rs` `ResourceAuthority::installed_epoch`; `src/mandate/grant.rs` `GrantVerifier::highest` |
| F13 | **Every check-then-act site has the pause window, not only the wiki.** The gateway checks in `ae_preflight`, then dispatches; C3's provider check will run before the handler. A process that pauses in between acts late by the pause | `src/agent/http.rs` (`ae_preflight` → `gateway_rpc_call`); C3 |
| F14 | **Frozen-clock decisions beyond the three #405 fixes are unaudited.** `hlc.current()` is still read at eleven sites, including a nonce replay window (`mycelium-core/src/connection.rs:215`). The wall-clock helpers (`intent::now_ms`, `capability_ops::now_ms`, `federation::edge::now_ms`) read the wall clock and are not affected | `grep "hlc.current()"` |
| F15 | **Stop times are modelled, never measured.** `drain_report` computes T_admit and T_drain from the caller's timestamps; no deployment measures them | ADR `authority-at-execution.md` §4 |

**What this means for the claim.** Today "revocation stops the fleet" holds only for callers that use `/mcp`, `/a2a`
or federation calls. It does **not** hold for full-node fleets (F2), and it does **not** fully hold under the
confined profile either (F3), because any agent that serves skills can walk around the mandate check through its
own gateway.

---

## 2. Decisions

1. **Enforce at the provider, not only at the route.** The gateway stays a route-level preflight (AE0 §7). The
   binding check moves to where the work happens: the provider's serve path. Every door then leads to the same
   check, and a door nobody has found yet is covered too.
2. **The provider verifies for itself.** It re-verifies the grant and the possession proof. It does not take a
   gateway's word that a mandate was established: Boundary H's adversary is a colluding population of admitted
   members, and a gateway is a member.
3. **Raw routes are for coordination, not for protected work.** Protected kinds are refused on raw emission and
   raw call routes. Scopes separate *serving* from *calling*.
4. **Cutting a member off is an operator act**, signed by a configured membership authority, and enforced at
   every layer that admits a peer.
5. **Opt-in stays opt-in.** With nothing configured, behaviour is unchanged, as with #399 and #402.

---

## 3. Work items

Sizes: S ≈ a day, M ≈ two to four days, L ≈ a week or more. Each item ships as its own PR with the five-part
statement, a wiki `.log` entry, lock-order rows for any new lock, and SDK and operator-doc parity where a gateway
route changes.

### C0: Honesty fixes (S, first) — **done in #402**

- ~~ADR §7 and PR #402: call the check-to-transaction distance "not a gap" (F9)~~. **Reversed in rev 0.2:** it is
  a stated limit, pinned by a test, and C9 addresses it.
- `boundary-h.md` §16: H6 is recorded as **primitive only, not wired** until C4 lands (F5).
- Threat model, A1 entry: state that the fleet-stop claim currently holds **only through `/mcp`, `/a2a` and
  federation**, and name F2 and F3 until C1 and C2 close them.
- **Gate:** documentation review. No code.

### C1: Close `/gateway/rpc/call` to protected work (S–M, urgent: it breaks H7) — **implemented, see §8**

- **Protected kinds.** A registry of RPC kinds that are protected work: `mcp.invoke`, `skill.invoke`, and any
  kind an operator registers (`GossipConfig::protected_rpc_kinds`). `/gateway/rpc/call` **refuses** a protected
  kind with an error naming the right door (`/mcp` or `/a2a`). This mirrors how `signal/emit` refuses a caller
  frame.
- **Scope split.** `mesh:serve` covers `/gateway/rpc/serve` and `/gateway/rpc/respond`; `mesh:write` keeps
  emission and raw calls. A serving agent's token then needs no calling power. A legacy token holding
  `mesh:write` keeps both for one release, logged at `warn!`.
- **SDK parity.** `mycelium-py` and `mycelium-ts` serve helpers document the new scope; the confined-fleet
  manifests and runbook issue `mesh:serve` to agents, not `mesh:write`.
- **Gate:**
  - `/gateway/rpc/call` with `mcp.invoke` and with `skill.invoke` → refused, and the provider's handler is never
    reached;
  - a non-protected kind still passes (the plant);
  - a `mesh:serve` token can serve and respond, and cannot call;
  - in the confined-fleet deployment test, an agent pod's attempt is refused.

### C2: Carry the mandate to the provider (M; needs nothing) — **implemented, see §8**

- **Through the gateway.** The gateway puts the presented grant and possession proof into the caller envelope
  it already signs (`gateway_caller`), next to the principal. Its own assessment stays as evidence; it is not
  what the provider relies on.
- **Direct member calls.** `ServiceHandle::rpc_call_with_mandate(target, kind, payload, grant, proof)` for a node
  acting under its own grant (holder `node:{id}`).
- **Possession binding at the provider.** The provider knows its own node id, so the proof can now bind the
  **full** resource, including the provider after `@`. The gateway's resolution of `@` is recorded for the
  client, so SDK callers keep signing what they sign today. Golden vectors are extended, not replaced.
- **Wire compatibility.** The envelope gains an optional field; an older provider ignores it. The gateway
  dispatches a mandate-bearing call only to a provider whose `sys/caller-context` marker says it verifies
  mandates, otherwise it answers `ProviderWithoutMandateSupport`, as it does today for providers without caller
  context.
- **Gate:** envelope round-trip; tampering with the grant, the proof or the principal breaks verification;
  golden vectors identical in Rust, Python and TypeScript.

### C3: A1 at the provider (M; needs C2) — **implemented, see §8**

- **One hook, every serve path.** `GossipAgent::with_provider_authority(ExecutionAuthority)` (the #399 type,
  reused). It is checked in the three places a request reaches work:
  - the MCP tool serve loop (`run_mcp_tool_task`);
  - `RpcRequestRx::recv`, which every Rust serve loop, including companion crates, already goes through;
  - `/gateway/rpc/serve`, before a request is streamed to an SDK agent.
- **The check.** Holder = the verified principal from the envelope; P2 (issued, entitled, current, possessed for
  this call); A1 (epoch, scope, window, operation, fresh revocation standing). A protected kind without an
  established mandate is refused with an error reply, and the handler never runs. The decision is written to
  the evidence journal on the provider, so a refusal is recorded where the work would have happened.
- **Revocation checkpoints** reach providers the way they reach gateways (`offer_revocation_checkpoint`); the
  gossip transport for checkpoints is K3b's concern and is used here once it exists, fed by the operator
  meanwhile.
- **Gate:**
  - direct member `rpc_call` without a grant → refused, handler not reached (closes F2);
  - with a valid grant → runs;
  - after a revocation checkpoint → refused;
  - with no fresh checkpoint → refused (silence denies);
  - an SDK-served skill: `/gateway/rpc/serve` never streams a refused request.

### C4: Wire H6 at the provider (M; needs C3's hook) — **implemented, see §8**

- `CohortBudget::admit` at the same hook, keyed by the **authenticated** principal, resolved to a cohort through
  the provider's H5 view. It is never taken from a caller-supplied label. Undeclared callers share one budget.
- Refusal: `AtCapacity` (JSON-RPC −32004), the same code the federation edge uses; recorded in the rights
  ledger.
- **Gate:** the four cases in `boundary-h.md` §8 H6 (fifty members within their own caps exceed the cohort cap;
  a caller-supplied label is ignored; another cohort is unaffected; refusals appear in the ledger), exercised
  through a real serve loop rather than the primitive alone.

### C5: Cutting a member off (L; needs its own ADR and review)

- **The act.** An operator-signed `MemberRevocation { node, key, issued_at, seq }`, signed by a configured
  membership authority (P1 external-issuer verification), written to `sys/membership/revoked/{node}`.
  Monotonic, like A1's revocation view after #402.
- **Where it is enforced:**
  - RPC receive: a request from a revoked member is refused before any serve path;
  - gossip ingest: writes from a revoked member are not applied;
  - the TLS handshake: a custom client and server certificate verifier rejects a revoked member's key, so it
    cannot re-join by reconnecting;
  - knowledge: its signatures verify as `Revoked` (the existing P1 path).
- **Freshness.** A node that has not heard from the membership authority within *F* treats the list as unknown.
  The ADR must decide what that means for peer admission (failing closed on membership would partition the
  fleet), which is why this item needs review before code.
- **Gate:** a revoked member's RPC, gossip write and reconnect are each refused; an unrevoked member is
  unaffected; a forged revocation is ignored.

### C6: `FsStore` gets the `WriteAuthority` seam (S; lowest priority) — **implemented, see §8**

- `FsStore` asks the same `WriteAuthority` before each mutating publish (`write_section`, `update_manifest`,
  `write_page`, `remove_page`). It gets A1's time and revocation checks; it still has no appointment fence,
  which is stated.
- **Gate:** the #402 cases that do not depend on git (no checkpoint, expired, revoked, silence, plant).
- **Why last:** no deployment uses `FsStore` with authority on the line (F8). It is cheap, and it removes the one
  store where the seam is missing.

### C7: The closure gate, the bypass matrix (M; last) — **implemented in-process, see §8**

One end-to-end test, and a deployment variant, that revokes an agent and then tries **every door**:

| Door | Expected after revocation |
|---|---|
| `/mcp` `tools/call` | refused at the gateway (#399) and at the provider (C3) |
| `/a2a` send and stream | refused at the gateway and at the provider |
| federation call | refused at the gateway and at the provider |
| `/gateway/rpc/call` with a protected kind | refused at the route (C1) |
| `/gateway/signal/emit`, RPC-shaped | refused at the provider (F4, already) |
| direct member `rpc_call` | refused at the provider (C3) |
| `/gateway/rpc/serve` delivering to an SDK agent | never streamed (C3) |
| wiki write and publish | refused at the store (#402) |
| reconnect as a revoked member | refused at the handshake (C5) |

The pass condition is that **no handler runs**, observed by a counter inside each handler, not by the absence of
an error. The deployment variant runs in the confined-fleet CI job, so the network half is tested too.

---

### C8: Restart-safe authority state (M; closes F10 and F12) — **implemented, see §8**

- **Cumulative checkpoints.** The checkpoint format is declared **cumulative**: each lists every revocation in
  force in its scope, not only new ones. An authority that cannot produce that refuses to start. The view keeps
  its union of revoked terms as a defence, and stays order-independent (done in #402).
- **An issued-after-start rule.** After a restart, a reader accepts a checkpoint only if it was issued after the
  reader started, allowing for skew: `a ≥ start − 2s`. A pre-revocation checkpoint replayed after the restart is
  then refused, and because the first accepted checkpoint is cumulative, it carries every revocation.
- **The fallback, if cumulative checkpoints are not acceptable:** persist the view (newest `seq` and the revoked
  set) in the node-local journal, as K3a did for checkpoints, and reload it on start.
- **Epochs survive a restart too (F12).** The resource's installed epoch and the grant verifier's highest
  verified epoch per scope are persisted in the node-local journal, written **before** the new epoch takes
  effect, and reloaded on start. The configured epoch becomes a floor, never a reset. A journal that cannot be
  read fails closed: the gate refuses until an operator installs an epoch.
- **Gate:**
  - restart, then replay a fresh pre-revocation checkpoint → refused, the term stays revoked;
  - restart, then a new cumulative checkpoint → accepted and correct;
  - install epoch 2, restart with epoch 1 configured → a mandate at epoch 1 is still refused;
  - a plant without the rule fails the first case;
  - **the trap to avoid** (from #405): any freshness or clock test must force the clock's state into the
    past. `Hlc::new()` seeds from the wall clock, so a test on a freshly created clock passes against a frozen
    implementation too, and proves nothing.

### C9: The check-then-act window, at every A1 site (M; addresses F9 and F13) — **implemented, see §8**

**The general limit, stated once.** Wherever authority is checked and then acted on as two steps, a process that
pauses between them acts late by the length of the pause. Normal latency is not a bound. No local fix exists
where the effect has no clock of its own, so each site gets the strongest of three answers it can have:
**prevent** at a component with a trusted clock, **cancel** the action once it is running, or **detect** a late
act afterwards.

| Site | Check → act | Answer |
|---|---|---|
| Wiki git store | `authorize()` → `update-ref` | Detect locally; prevent at the remote hook for published writes (below) |
| Gateway | `ae_preflight` → dispatch | Carry the deadline in the caller envelope (C2) so the provider re-checks (C3): the provider's check is later, which shrinks the window; it does not remove it |
| Provider (C3) | check → handler | Cancel: the handler holds C10's token, so a lapse during the run cancels it |
| Queued and retried work | dequeue check → run | Already re-checked at dequeue and retry (A1 rule 4); the window per attempt is as above |


For the wiki store specifically:
- **Record, locally.** After the ref transaction, the store re-checks authority. If it lapsed during the
  transaction, the commit is recorded as a **late write**, with its commit id and the lapse, in the evidence
  journal, and the curator raises it. This is detection, not prevention, and the design says so.
- **Prevent, at the remote.** For published writes, the pre-receive hook checks the pushing curator's mandate
  window and revocation standing against the **remote's** clock, which the curator does not control. The
  scoped-mandates ADR already puts a hook there; this gives it the time check.
- **Gate:**
  - `a_pause_after_the_check_is_not_caught_locally` gains its detection assertion (the late write is recorded);
  - a hook test refuses a push from a curator whose mandate expired before the push arrived;
  - a gateway test pauses between preflight and dispatch and shows the provider's own check refusing (needs C3);
  - the ADR states the general limit in one place, and each site's docs link to it.

### C10: Cancelling admitted work (L; addresses F11) — **implemented, see §8**

- **What.** Turn `StopContract` from a model into a mechanism: a running handler holds a cancellation token tied
  to its mandate's term. A revocation, or expiry with `Continuation::ReauthorizeAt`, cancels it, and the
  handler's confirmation is recorded, so `drain_report` measures a real stop.
- **Scope.** Rust handlers first (the serve paths from C3); the SDK serve route forwards the cancellation to the
  agent; handlers that cannot confirm are reported `Unbounded`, as the model already does.
- **Gate:** a long-running handler is cancelled within its declared bound after revocation, and T_drain is
  measured end to end.

### C11: Audit every time-based decision for a frozen clock (S–M; addresses F14; after #405) — **implemented, see §8**

- **What.** Every read of `hlc.current()` is classified: a **decision** (a deadline, an expiry, a freshness or
  replay window), which must read `Hlc::decision_now_ms()` (#405), or a **stamp** (an ordering or record
  timestamp), which may keep `current()`. The eleven remaining sites are the starting list:
  - `src/consensus.rs:2107`;
  - `mycelium-core/src/connection.rs:215` (the nonce replay window), `:504`, `:619`, `:712`;
  - `mycelium-core/src/ops.rs:65`, `:103`, `:138`;
  - `mycelium-core/src/persistence.rs:669`, `:677`;
  - `src/consensus.rs:468`, which already floors at the wall clock.
- **A guard against regressions.** A check script, like `check-sim-seams.sh`, that fails the build on a new
  `hlc.current()` read outside an allowlist of classified stamp sites.
- **End to end (the reviewer's request).** A quiet node: a gateway whose gossip is stopped, with its HLC state
  forced an hour into the past (#405's trap: a freshly created clock proves nothing). A mandate that expires
  during the silence is refused, and a checkpoint that goes stale is `Unknown`. The same test with a planted
  `current()` read passes the call, which proves the test can fail.
- **The assumption, made explicit.** The clock model's *s* bounds the host wall clock's error. The operator
  runbooks state the NTP (or equivalent) requirement, and the confinement self-report gains a `clock_sync`
  line it reports as `Unverified`, as it does for the network, because a node cannot vouch for its own clock.

### C12: Measure the stop in a deployment (M; addresses F15; after C10) — **measured on a live node, see §8**

- **What.** The confined-fleet deployment test gains a stop measurement: start long-running work under a
  mandate, revoke it, and record T_admit (revocation to the last admission) and T_drain (revocation to the last
  **confirmed** stop) from the CNI's and the journal's own timestamps, not the caller's.
- **The pass condition.** Both within the declared bounds (`StopContract::drain_bound` plus *F*), unconfirmed
  stops reported as unconfirmed, never as stopped.
- **Why after C10.** Without cancellation, T_drain is "when the work happened to finish"; measuring it first
  would record the absence of a mechanism as a number.

## 4. Order

| Step | Items | Why this order |
|---|---|---|
| 1 | C0, C1 | C1 closes a hole in a profile already documented as confining; C0 stops the docs overclaiming meanwhile |
| 2 | C2 → C3 | The provider cannot check what it is not given |
| 3 | C4 | Reuses C3's hook |
| 4 | C5 | ADR and review first; the only item with a real design question |
| 5 | C6 | Cheap, independent, low value |
| 6 | C8 | Restart safety; independent, can start now |
| 7 | C9 | Needs the remote hook's ADR update |
| 8 | C10 | Needs C3's serve-path hook |
| 9 | C11 | After #405 merges; the audit and guard are independent of everything else |
| 10 | C12 | Needs C10 |
| 11 | C7 | Proves the whole, including C8–C12's cases |

**Estimate.** Steps 1–3: about two weeks. C5: a week after its ADR is reviewed. C6 and C7: about four days. C8 and
C9: about a week and a half together. C10: a week or more. C11: two to three days. C12: two to three days.

---

## 5. What this plan does not claim

- **Acts off the substrate.** An agent that is a full node on an unconfined network can still make direct network
  calls the substrate never sees. That is H7's job, and C1 makes H7 hold.
- **Rate.** Cohort budgets bound concurrency, not rate (as in #365).
- **Revocation latency below *F*.** Every check here enforces revocation within the freshness window, not
  instantly.
- **Providers outside Mycelium.** An MCP server bridged in with `connect_mcp_server` is protected at the bridging
  node's serve loop (C3), not inside the external server.

## 6. Out of scope here, still open in `boundary-h.md`

K3b (head transport), K3c (body authorisation and storage caps), H4 (source-signed audit checkpoints), and demo D.
The NovusLens pin bump (Novus-i2 is on `mycelium-wiki` 2.4.4) is tracked downstream.

## 7. Review disposition (external review of #402, 2026-09-25)

| Finding | Disposition |
|---|---|
| A restart can restore revoked authority | **Accepted.** F10, C8 |
| Out-of-order checkpoints can lose revocations | **Accepted, fixed in #402.** An authentic revocation is recorded before the replay and future-dating checks; gate `a_revocation_in_a_late_arriving_older_checkpoint_still_revokes` |
| The wiki's check-to-mutation window | **Accepted; rev 0.1 was wrong** to call it not a gap. F9, C9; pinned by `a_pause_after_the_check_is_not_caught_locally` |
| The gateway's clock | **Accepted, fixed separately** in #405 (`fix/preflight-reads-a-live-clock`) (`Hlc::decision_now_ms()`, a live clock that advances nothing, at the three sites that read `Hlc::current()`). #402 briefly carried its own copy of the same fix and dropped it in favour of that branch |
| The *F* − 2*s* partition bound | **Accepted, corrected in #402.** *F* − 2*s* is the reader-clock threshold; in real time, work stops when the checkpoint is at most *F* old |
| Fleet-stop coverage is incomplete | **Accepted.** Already C1–C3; runtime cancellation added as F11, C10 |

### Rev 0.3: gaps in rev 0.2's own answers

Asked whether rev 0.2 fully addressed the review, a re-read found four gaps:

| Gap | Change |
|---|---|
| C8 covered the revocation view but not epochs; a restart also forgets superseded epochs (F12) | C8 renamed *restart-safe authority state*, with persisted epochs as a floor and a gate case |
| C9 treated the check-then-act window as a wiki problem; it exists at every A1 site (F13) | C9 generalised, with a per-site table of prevent, cancel or detect |
| The clock fix (#405) covers three sites; nothing audits the rest, and no end-to-end test covers a quiet node (F14) | New C11: classification, a regression guard, the quiet-node test, and the wall-clock assumption made explicit |
| Stop times are modelled, never measured (F15) | New C12: T_admit and T_drain measured in the confined-fleet deployment, after C10 |

Three items rev 0.2 marked done stay done: order-independent revocation, the *F* − 2*s* correction, and `main`'s
monotonicity bug (all in #402, merged).

## 8. Delivery record

| Item | PR | Departures |
|---|---|---|
| C0 | #402 | none |
| C1 | #409 | **Six routes, not one.** Reading the gateway found the same hole in `scatter` and `overlay/emit_reliable` (both dispatch an arbitrary kind through `gateway_rpc_call`), and `signal/emit`, `mailbox/deliver` and `shard/emit` take a kind too; all six refuse. **`llm.invoke` is protected by default**, because the raw routes also walked around `llm:invoke`. **The deployment variant of the gate moved to C7/C12**: the confined-fleet deployment test runs `busybox` in place of the gateway (it tests the network policy), so a real-gateway refusal needs the gateway image C7 and C12 need anyway. **Found on the way:** the TypeScript SDK's `rpcCall` sent `kind` where the route reads `method`, so every call was a `400`; fixed and pinned |
| C2 | #410 | **No provider marker.** The plan had the gateway dispatch a mandate-bearing call only to a provider whose marker says it verifies mandates. Dropped: an older provider ignores the new envelope field, which is exactly today's behaviour, so dispatching to it is no regression, and C3 is where a provider starts to rely on the field. **The possession proof still binds the resource before `@`**, not the resolved provider: a caller cannot know the provider in advance, so binding it would change what every SDK signs; the provider recomputes the same bytes. **Found on the way:** the streaming door (`tasks/sendSubscribe`) passed no params to the preflight, so a mandate on a stream was never read (a #399 defect); fixed and gated |
| C3 | #411 | **The provider runs the gateway's own preflight** rather than a separate check, as the enforcement point `provider`: one policy, one mandate assessment, one evidence journal. Opt-in with `with_provider_enforcement()`, failing closed without an evaluator. **A resource claim travels in the envelope** (field `r`): a skill's payload is only its text, so the provider cannot name the skill itself; it accepts the claim only as `…@{self}` for a capability it advertises. `rpc_call_with_mandate` gained a `resource` argument (C2's API, unreleased). **`llm.invoke` is not checked at the provider**: its loop registers its own receiver and no LLM door carries mandates. **Decision recorded, not outcome**, for `rpc_rx` work |
| C4 | #412 | **Refusals are counted and logged, not written to the rights ledger**: that ledger records governor rights, not per-call admissions (`cohort_budget_refusals()`, metric `mycelium_provider_cohort_refusals_total`). **The place lives as long as the call:** held across the handler in the MCP loops; carried by the `RpcRequest` through `rpc_rx`, so it is released when the serve loop drops the request; parked until `/gateway/rpc/respond` (or 300 s) for the SDK serve stream. **Independent of C3's enforcement**, and after it when both are on, so a refused call never holds budget |
| C6 | #413 | **The trait moved** from the git-only `mandate_fence` module to `store` (always compiled), re-exported at its old path and at the crate root, so no caller changes. **Asked after the store's own mutation lock**, before anything is written, in all four mutators |
| C8 | #414 | **Epochs: the gate's installed epoch is persisted, not the verifier's retained epochs.** The gate refuses a mandate below its installed epoch whatever the verifier remembers, so that is what supersession safety needs. **The start is marked where the reader is created**: at `with_execution_authority` for the gateway and provider, and at construction for the wiki store. **The cumulative-checkpoint contract is documented, not checked**: a reader cannot tell a cumulative checkpoint from a partial one |
| C9 | #416 | **The hook judges the appointment's window, not revocation.** A pre-receive hook has a trustworthy clock but no revocation feed; it refuses pushes past the appointment's `valid_until_ms` recorded on the mandate ref (`appointment.json`), and revocation at the remote stays the fence's job (moving the ref). **The gateway row relies on C3's provider re-check** rather than a separate test of a pause between preflight and dispatch: the provider's check is the later one, and C3's gates cover it |
| C10 | #417 | **Cancellation rides on the provider check, not a separate hook:** work is registered when C3's check admits a call under an established mandate, and cancelled by a periodic sweep that re-runs A1's check (one mechanism for expiry, revocation, staleness and supersession). **The MCP loop cancels hard** (the handler future is dropped); **`rpc_rx` is cooperative** (`RpcRequest::authority_lapsed()`). **Not built: forwarding cancellation to SDK agents** through the serve stream, and cancelling the bridged external-MCP loop; both stay unconfirmed, as the model already reports |
| C11 | #418 | **The seen-set sites stay on `current()`, classified**: a decision, but one that fails closed on a frozen clock (entries age slower), on the per-message hot path, where a wall-clock read per message is a cost with no safety gain. **The regression guard is a per-file count** (`scripts/check-hlc-current.sh`, like the sim-seam gate), not an allowlist of lines. **The quiet-node test's plant is an assertion on the mandate** (it *is* current at the frozen time) rather than a planted code path |
| C7 | #419 | **In-process, not yet in a deployment.** One node, both gateway and provider, with enforcement and an authority; every door the code has is tried after revocation, and the pass condition is the handlers' own counters. **Out of the matrix, gated where built:** federation calls (the same `ae_preflight`; `federation_transport` tests), the wiki store (`git_store_authority`), and reconnecting as a removed member (C5, proposed). **The deployment variant** waits on the node image C12 needs |
| C12 | this PR | **Measured on a live node, not yet in the kind cluster.** `examples/authority_drain` (run in CI) admits calls under a mandate on a real node, revokes it, and checks T_admit and T_drain from the node's own records against the declared bound; a first local run gave T_drain = 25 ms against a bound of 300 ms, T_admit none, no unconfirmed stops. **Open:** running it inside the confined-fleet kind job needs a node image built in that job; the deployment variants of C7 and C12 wait on it. **Found:** an MCP tool loop serves one call at a time, so queued calls meet the revocation at admission. **Found:** Rust had no public `arguments_digest` (the SDKs did); now `mycelium::arguments_digest`, pinned to the same golden vector |

