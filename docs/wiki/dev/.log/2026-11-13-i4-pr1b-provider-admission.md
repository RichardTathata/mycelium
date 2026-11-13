## [2026-11-13] ingest | I4, PR 1b: the provider's admission under a `call:` mandate

`docs/plans/identity-and-lifecycle.md` row I4 (rev 0.21) records PR 1b and Q9's answer.

- The caller envelope has a second mandate slot, `cm` (`src/agent/gateway_caller.rs`), carried and not verified, as
  `m` is. `admit_after` (`src/agent/provider_enforcement.rs`) assesses it once (D-3), immediately before it registers
  the call's work, against the envelope's verified `via` in its node's canonical spelling.
- **Q9: a `call:` mandate fits only the envelope it was minted for.** Its possession proof binds the operation, the
  resource and the arguments, not the call, and `cm` is outside the envelope's signature — so a relay could swap in
  another valid `cm` of the same gateway for an identical call, and the gateway's cancel would miss the work. The
  provider requires the term to be `c{n}` of the carrying envelope; `n` is inside the signature. No wire or
  signed-bytes change. Consequence: a term reaches a provider only in its own envelope, which the replay window
  admits once, so "re-presented, refused `Revoked`" is an authority-level property.
- A rule's `cancellable` travels `Decision` → `Preflight::Proceed` → `admit_after`. Without an established `call:`
  mandate a cancellable call is refused `authority_not_established` / `call_mandate_not_established`; a revoked
  term refuses under any rule. Until a gateway mints (PR 2) a cancellable rule refuses every call.
- One call, two registrations: `Admission` holds the caller's guard and the `call:` guard; `run` and
  `RpcRequest::authority_lapsed()` wait on either. The bridged MCP loop and the SDK serve stream reach the same
  check but do not stop a cancelled call (PR 3).

From #781's review and the maintainer's decisions of 2026-11-13:
- Re-presentation holds by three mechanisms in sequence (replay window; `Revoked` from the KV after a provider
  restart inside 300 s; `Stale`); a replay is dropped unanswered; a retry is a new term. A revoked term refuses under
  any rule (best effort where the rule is not cancellable). One detail, with a stable `data.cause` (`CallCause`) that
  also labels the counter.
- The slot is assessed only for a **signed** envelope (the id is what binds `cm`); the refusal gives a claimed grant
  back at the provider and at the gateway's `/mcp` door.
- **Found, not fixed: the built-in `llm.invoke` provider never calls `provider_enforcement::check`**
  (`what-is-proven.md#llm-door-not-checked-at-provider`).
- `owner-signed-records.md`: the `call/{gw}/{term}/checkpoint` row now says the record is read from a live door;
  Status stays Partly.

Pages: `docs/wiki/dev/owner-signed-records.md`, `docs/features/provider-enforcement.md`, `docs/reference/unit-file.md`, rule `provider.enforcement` rev 4.
