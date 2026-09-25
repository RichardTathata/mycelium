# Boundary H closure: authority at every door (implementation plan)

**Status:** draft rev 0.2, 2026-09-25 (rev 0.2 folds in an external review of #402; see §7). Follows [`boundary-h.md`](boundary-h.md) (rev 0.5), whose items have all
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

### C1: Close `/gateway/rpc/call` to protected work (S–M, urgent: it breaks H7)

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

### C2: Carry the mandate to the provider (M; needs nothing)

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

### C3: A1 at the provider (M; needs C2)

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

### C4: Wire H6 at the provider (M; needs C3's hook)

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

### C6: `FsStore` gets the `WriteAuthority` seam (S; lowest priority)

- `FsStore` asks the same `WriteAuthority` before each mutating publish (`write_section`, `update_manifest`,
  `write_page`, `remove_page`). It gets A1's time and revocation checks; it still has no appointment fence,
  which is stated.
- **Gate:** the #402 cases that do not depend on git (no checkpoint, expired, revoked, silence, plant).
- **Why last:** no deployment uses `FsStore` with authority on the line (F8). It is cheap, and it removes the one
  store where the seam is missing.

### C7: The closure gate, the bypass matrix (M; last)

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

### C8: Restart-safe revocation state (M; closes F10)

- **Cumulative checkpoints.** The checkpoint format is declared **cumulative**: each lists every revocation in
  force in its scope, not only new ones. An authority that cannot produce that refuses to start. The view keeps
  its union of revoked terms as a defence, and stays order-independent (done in #402).
- **An issued-after-start rule.** After a restart, a reader accepts a checkpoint only if it was issued after the
  reader started, allowing for skew: `a ≥ start − 2s`. A pre-revocation checkpoint replayed after the restart is
  then refused, and because the first accepted checkpoint is cumulative, it carries every revocation.
- **The fallback, if cumulative checkpoints are not acceptable:** persist the view (newest `seq` and the revoked
  set) in the node-local journal, as K3a did for checkpoints, and reload it on start.
- **Gate:**
  - restart, then replay a fresh pre-revocation checkpoint → refused, the term stays revoked;
  - restart, then a new cumulative checkpoint → accepted and correct;
  - a plant without the rule fails the first case.

### C9: The check-to-transaction window (M; addresses F9)

- **Record, locally.** After the ref transaction, the store re-checks authority. If it lapsed during the
  transaction, the commit is recorded as a **late write**, with its commit id and the lapse, in the evidence
  journal, and the curator raises it. This is detection, not prevention, and the design says so.
- **Prevent, at the remote.** For published writes, the pre-receive hook checks the pushing curator's mandate
  window and revocation standing against the **remote's** clock, which the curator does not control. The
  scoped-mandates ADR already puts a hook there; this gives it the time check.
- **Gate:** `a_pause_after_the_check_is_not_caught_locally` gains its detection assertion (the late write is
  recorded). A hook test refuses a push from a curator whose mandate expired before the push arrived.

### C10: Cancelling admitted work (L; addresses F11)

- **What.** Turn `StopContract` from a model into a mechanism: a running handler holds a cancellation token tied
  to its mandate's term. A revocation, or expiry with `Continuation::ReauthorizeAt`, cancels it, and the
  handler's confirmation is recorded, so `drain_report` measures a real stop.
- **Scope.** Rust handlers first (the serve paths from C3); the SDK serve route forwards the cancellation to the
  agent; handlers that cannot confirm are reported `Unbounded`, as the model already does.
- **Gate:** a long-running handler is cancelled within its declared bound after revocation, and T_drain is
  measured end to end.

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
| 9 | C7 | Proves the whole, including C8–C10's cases |

**Estimate.** Steps 1–3: about two weeks. C5: a week after its ADR is reviewed. C6 and C7: about four days. C8 and
C9: about a week together. C10: a week or more.

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
| The gateway's clock | **Accepted, fixed separately** on `fix/preflight-reads-a-live-clock` (`Hlc::decision_now_ms()`, a live clock that advances nothing, at the three sites that read `Hlc::current()`). #402 briefly carried its own copy of the same fix and dropped it in favour of that branch |
| The *F* − 2*s* partition bound | **Accepted, corrected in #402.** *F* − 2*s* is the reader-clock threshold; in real time, work stops when the checkpoint is at most *F* old |
| Fleet-stop coverage is incomplete | **Accepted.** Already C1–C3; runtime cancellation added as F11, C10 |

