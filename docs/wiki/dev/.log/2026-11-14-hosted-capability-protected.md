## [2026-11-14] ingest | a hosted capability's kind is protected work at an enforcing provider

- **`cap.invoke/{ns}/{name}` is decided at `rpc_rx`** (the maintainer's decision of 2026-11-14, on #785's review;
  `deprecations.md` §108). `provider_enforcement::protected_here` adds the whole prefix to the provider's protected
  set; `is_protected_kind` stays the exact-kind list, because its other callers each treat the family in their own
  way already (the gateway's routes refuse it with their own message, `ot` counts it, a component's `mesh.emit`
  admits one its host listed).
- **The kind names what runs, so the claim is never read** — as for `mcp.invoke` and `llm.invoke`. `tool/{name}`
  derives what the MCP tool loop derives for the tool `{name}` (`tools/call`, `tool:{name}@{node}`, the payload as
  JSON), so one rule and one arguments digest cover both doors; any other namespace derives `cap.invoke` on
  `cap:{ns}/{name}@{node}`. Not JSON → `{"payload_b64"}` (one stated collision with a JSON payload of that shape).
- **A signal's `sender` is not authenticated by the connection it arrives on.** `connection.rs` builds the `Signal`
  from the frame's own `sender` field and delivers it; only a caller envelope inside the payload is signed. So
  "this request came from this node" cannot be read off a request, and an exemption for it would be forgeable by
  any member. The wasm host's MCP bridge used to call its own node (`rpc_call(self, cap.invoke/tool/x)`); rather
  than exempt that leg, the bridge now hands the admitted arguments to the install's serve loop over an mpsc
  channel created at install (`mycelium-wasm-host/src/runtime.rs`, `serve_loop`). Nothing on the mesh can write
  to it. Reusable rule: **a second leg inside one process is a function call or a channel, not a self-RPC with an
  exemption.**
- **`RpcRequestRx::recv` is not cancel-safe** (it may hold a request whose provider check is mid-flight), so a
  loop that selects it against another source keeps one `recv` future pinned across the other source's work and
  replaces it only after it yields — the shape `serve_loop` now has.
- **A cohort budget counts the family**, with enforcement off too; the bridge's hand-over takes no second place.
- **Still not decided:** `bulk_serve` and a raw `signal_rx` receiver, for this family as for any.
- **Fail-first, both ways round:** family protected + old bridge → `/mcp` returns the provider's refusal as the
  tool's text and the component never runs; new bridge + family unprotected → a member's direct call runs the
  component undecided.

Pages: `docs/wiki/dev/security.md`, `docs/features/provider-enforcement.md`, `docs/features/declared-authority.md`,
`docs/operations/what-is-proven.md` (the row moved from *not built* to *proven*), `docs/guide/deprecations.md` §108,
`docs/plans/identity-and-lifecycle.md` (I5's `ot` list restated), rule `provider.enforcement` rev 7.
