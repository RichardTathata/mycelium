## [2026-09-25] ingest | closure plan C3: authority decided where the work runs

Up: [dev](../dev.md) · page: [security](../security.md) · record `docs/design/authority-at-execution.md` §8,
`docs/plans/boundary-h-closure.md` C3 · code `src/agent/provider_enforcement.rs`, hooks in `src/agent/mcp.rs`,
`src/agent/rpc.rs` (`RpcRequestRx::recv`), `src/agent/http.rs` (`gw_rpc_serve`).

**The gap.** A provider checked who was calling and then ran the call; only a gateway asked whether the caller may.
A member's direct `rpc_call` skipped the question entirely.

**The decision that kept it small.** Rather than a second check with its own policy language, the provider runs the
gateway's `ae_preflight` as the enforcement point `provider`. Same evaluator, same mandate assessment, same evidence
journal. It needed only the call's operation, resource and arguments, derived as the gateway derives them.

**What the plan had not seen.** A skill's payload is its text alone, so a provider cannot name the skill it is
serving. The envelope now carries the resource the gateway resolved (or the member names), and the provider accepts
it only as `@{self}` for a capability it advertises, because otherwise a grant for one skill would admit a call
routed to another.

**Found on the way.** The bridged external-MCP loop never verified the caller context. With enforcement on it now
does, through the check; with enforcement off, behaviour is unchanged. That is recorded here rather than fixed
silently.
