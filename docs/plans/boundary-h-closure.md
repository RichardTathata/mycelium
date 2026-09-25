# Boundary H closure: authority at every door (implementation plan)

**Status:** draft rev 0.1, 2026-09-25. Follows [`boundary-h.md`](boundary-h.md) (rev 0.5), whose items have all
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
| F9 | The distance between the wiki store's A1 check and git's `update-ref` is local subprocess time: milliseconds, against a designed revocation latency of up to *F* (≈ 60 s). Appointment moves inside it are caught by the fence's `verify`. **Not a gap** | `mycelium-wiki/src/git_store.rs` `commit_files` |

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

- ADR `authority-at-execution.md` §7 and PR #402: remove the check-to-transaction distance from "Not claimed",
  with one sentence saying why (F9).
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

## 4. Order

| Step | Items | Why this order |
|---|---|---|
| 1 | C0, C1 | C1 closes a hole in a profile already documented as confining; C0 stops the docs overclaiming meanwhile |
| 2 | C2 → C3 | The provider cannot check what it is not given |
| 3 | C4 | Reuses C3's hook |
| 4 | C5 | ADR and review first; the only item with a real design question |
| 5 | C6 | Cheap, independent, low value |
| 6 | C7 | Proves the whole |

**Estimate.** Steps 1–3: about two weeks. C5: a week after its ADR is reviewed. C6 and C7: about four days.

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
