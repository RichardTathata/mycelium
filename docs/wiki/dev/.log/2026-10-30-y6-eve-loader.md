# 2026-10-30 — Y6: an Eve agent directory, unchanged, on a fleet

Adopter-surface row Y6 (D24): `mycelium-ts`'s Eve loader (`mycelium-ts/src/eve.ts`) reads an Eve `agent/` directory
installed beside one pinned `eve` version, 0.80.0, and carries its contents onto the fleet's doors. The demonstration
is `examples/eve_weather/`, run by the CI job `eve-fleet`.

**Durable knowledge.**

- **An SDK cannot serve an MCP tool.** `GET /gateway/rpc/serve/{kind}` refuses `mcp.invoke` (403 `protected_kind`,
  2.36.0). An SDK-hosted tool reaches the fleet as an MCP server a node bridges with `McpHandle::connect_mcp_server`.
  The node's `/mcp` door decides the call, and the bridge reads the server's tool list once, at connect. The published
  `mycelium` binary bridges nothing and attaches no evaluator from configuration, so the demonstration's node is an
  example binary (`examples/eve_fleet_node.rs`), like the Strands fleet's.
- **A refused slot is named, never skipped.** The loader's slot table (`EVE_SLOTS`) follows Eve's own
  `docs/reference/agent-files.md`. A slot the loader does not load fails the whole load, with every such entry in one
  `EveRefusedError`. The reason is practical: an agent that relied on a hook or a sandbox would otherwise run without
  it and look as if it worked. The example README's table and `EVE_SLOTS` are compared cell for cell by a test.
- **The reference evaluator answers *indeterminate*, not *deny*, for a call no allowance covers.** The gateway refuses
  both. `mycelium explain --correlation` prints the record as `indeterminate, execution none`. A driver that greps for
  `deny` will not find the refusal.
- **Eve tools are Standard Schema.** The loader reads `~standard.jsonSchema.input` for the tool's JSON Schema and
  `~standard.validate` for its arguments, so it imports neither zod nor eve. A `.ts` tool is imported under Node's own
  type stripping, through a real dynamic `import()` (TypeScript's CommonJS output would make it a `require`).
- **The grant class does not reach the tool.** `mycelium test --faults grant` over the Eve unit with a grant-required
  rule shows the grant held across a restart at the state machine's door, on the declaration's in-process fleet. It
  does not call Eve's `execute`. The README says so.

**Decided.** `schedules/` is refused by name (option B, the maintainer, 2026-10-30). Row Y12 is an SDK route for
running schedules (plan rev 0.15).
