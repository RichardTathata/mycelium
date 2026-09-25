## [2026-09-25] ingest | closure plan C4: H6 stops being a primitive

Up: [dev](../dev.md) · page: [security](../security.md) · record `docs/design/knowledge-cohorts.md` §6 (wired),
`docs/plans/boundary-h-closure.md` C4 · code `src/agent/provider_enforcement.rs` (`ProviderBudget`, `Admission`,
`park`/`release_parked`), `src/agent/rpc.rs` (`RpcRequest` carries a `Held` admission) · lock-order rows 45, 46.

**Before.** `CohortBudget` shipped in #394 with its own gate and no caller. A colluding population inside every
member's cap still met no aggregate limit anywhere.

**The one design question: how long is a call in flight?** A slot is an RAII guard, so it must live exactly as long
as the work. In the MCP loops that is the handler's `await`. Through `rpc_rx` the handler is the application's, so
the request carries the slot and it is released when the application drops the request (a `Clone` shares it; the
last copy releases). For the SDK serve stream the request leaves the process, so the slot is parked until
`/gateway/rpc/respond`, and a reply that never comes gives it back after the gateway's 300 s RPC ceiling.

**Kept honest.** Refusals are counted and logged, not written to the rights ledger: it records governor rights, and
putting per-call admissions there would change what that ledger means. The plan said otherwise; the departure is in
§8.
