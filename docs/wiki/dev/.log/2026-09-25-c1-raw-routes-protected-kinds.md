## [2026-09-25] ingest | closure plan C1: the raw routes stop carrying protected work

Up: [dev](../dev.md) · page: [security](../security.md) · record `docs/plans/boundary-h-closure.md` C1, §8 ·
code `src/agent/http.rs` (`refuse_protected_kind`, `legacy_mesh_serve`), `mycelium-core/src/config.rs`
(`protected_rpc_kinds`), SDK `ProtectedKindError`.

**The hole.** `/gateway/rpc/call` took any `method` and dispatched it, framed with the client's principal, so a
provider accepted `mcp.invoke` or `skill.invoke` from it and the AE preflight never ran. Any agent that served
skills held `mesh:write` (for `rpc/respond`), so under the confined profile it could walk around its own gateway's
mandate check.

**What reading the gateway added.** Six routes take a kind from the body, and two more of them (`scatter`,
`overlay/emit_reliable`) dispatch through the same framed call. All six refuse protected kinds now. `llm.invoke`
is protected too: the raw routes skipped `llm:invoke` the same way.

**The scope split.** `mesh:serve` for the serve stream and `rpc/respond`, with a one-release window for old
tokens.

**Found on the way.** The TypeScript SDK's `rpcCall` sent `kind`, and the route reads `method`: every call was a
400, hidden because the only test that exercised it needs a live gateway. A stub test pins it now.

**Moved.** The confined-fleet deployment test runs `busybox` for the gateway, so the real-gateway refusal in a
deployment belongs with C7/C12.
