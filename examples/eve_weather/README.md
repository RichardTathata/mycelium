# An Eve agent directory, unchanged, on a fleet

[Eve](https://www.npmjs.com/package/eve)'s own tiny example, the weather assistant with one tool, run on three
Mycelium nodes from its `agent/` directory as Eve's README prints it. `mycelium-ts`'s loader reads the directory
([`src/eve.ts`](../../mycelium-ts/src/eve.ts)), a node serves its tool, and the tool is called through the fleet's
`/mcp` door, once permitted and once refused, with the refusal on the record. A grant on the tool holds across a
restart. Adopter-surface row Y6.

This is **a demonstration, not a compatibility promise** (the plan's D24). Eve is a preview and its runtime is Eve's
product. The loader reads one pinned version, **eve 0.80.0** (published 2026-10-15), and refuses a directory beside any
other. It does not follow the preview as it moves.

```sh
npm ci --prefix examples/eve_weather                      # eve 0.80.0, zod 4.6.7, ai 7.0.142, undici 8.12.2, as locked
npm ci --prefix mycelium-ts && npm run build --prefix mycelium-ts
cargo build --example eve_fleet_node --features compliance,llm
cargo build --bin mycelium --features cli,tls,compliance,llm
node examples/eve_weather/run_demo.mjs                    # Node 24 or later; ends with EVE FLEET OK
```

CI runs exactly this in the `eve-fleet` job.

## The run

[`run_demo.mjs`](run_demo.mjs) prints one line for each step:

1. **`unchanged:`** The three files of [`agent/`](agent/) are compared byte for byte with the example in the README of
   the installed `eve` package, and nothing else is in the directory.
   - [`instructions.md`](agent/instructions.md)
   - [`tools/get_weather.ts`](agent/tools/get_weather.ts), a `defineTool` with a zod schema
   - [`agent.ts`](agent/agent.ts)
2. **`loaded:`** `loadEveAgent` checks every entry of the directory against the table below, then imports `agent.ts`
   and the tool under Node's own type stripping. The tool's schema becomes JSON Schema through the Standard Schema
   interface zod implements. `serveEveTools` serves the tool as an MCP server.
3. **`fleet:`** Three nodes start from [`eve_fleet_node`](../eve_fleet_node.rs). Each has a gateway, the action
   evaluator, an evidence journal and named tokens. The third bridges the loader's server with `connect_mcp_server`,
   so `get_weather` is served from it. The first reaches it by discovery, as any member reaches a tool another node
   serves.
4. **`declared:`** `declareEveAgent` declares the agent's unit through the first node's gateway, and writes its
   instructions as the prompt skill `weather/instructions` (the text as the system prompt).
5. **`forecaster get_weather: PERMITTED`** and **`visitor get_weather: REFUSED`**: two calls through the first
   node's `/mcp`.
   - The policy, the same on every node, allows `tools/call` on `tool:get_weather@<provider>` to
     `token:fleet/forecaster` and names no one else.
   - The forecaster's call reaches Eve's `execute` and answers `Sunny`.
   - The visitor's call is refused at the gateway: *authority not established*. The tool never runs.
6. **`explained:`** `mycelium explain --correlation eve-refused` reads the deciding gateway's evidence journal. It
   prints the record of the refused call: the visitor, the tool and its provider, `indeterminate, execution none`,
   under policy `rev-eve-weather`. The permitted call's record is there too.
   - The reference evaluator answers *indeterminate* for a call no allowance covers, rather than *deny*: it never
     permits a call it has no rule for, and the gateway refuses both.
7. **`grant:`** The loader's unit for the agent is written to a project with
   [`grant-rules.toml`](grant-rules.toml) appended, and `mycelium test --faults grant` runs over it. The appended
   rules make `get_weather` grant-required, with the supervisor as its issuer.
   - This is X3's fault class, run here: silence refuses, the declared issuer's grant admits one call, and the grant
     holds across a restart of the agent's node over the WAL.
   - The class drives the agent state machine's own door on the declaration's own in-process fleet. It does not call
     Eve's `execute`. The grant is on the tool's name, which is the same at every door.

## How each slot of an Eve agent directory lands

Every slot of Eve 0.80.0's agent directory, from its reference `docs/reference/agent-files.md`. A slot the loader does
not load is **refused by name** and never skipped, so an agent that needs a hook or a sandbox does not run on the fleet
without it and look as if it works. `EveRefusedError` names every refused entry in one error. An entry no slot names is
refused as unknown. The test `the README's table is the loader's` in
[`mycelium-ts/tests/eve.test.ts`](../../mycelium-ts/tests/eve.test.ts) checks this table against the loader's
`EVE_SLOTS`. A second test walks a directory holding every slot and checks the status each is given.

| Slot | Status | On the fleet |
|---|---|---|
| `agent.ts` | loaded | the unit; its `model` is read and not routed, because the fleet routes inference through its own router (row R); any other key is refused by name |
| `instructions.md` | loaded | a prompt skill `<agent>/instructions`, the text as its system prompt |
| `instructions.ts` | not built | a module-backed instructions source; the Markdown form is the one loaded |
| `instructions/` | not built | a directory of instruction fragments; the single Markdown file is the one loaded |
| `instrumentation/` | not built | Eve's telemetry providers; the fleet's records are its own (X4) |
| `channels/` | not built | maps to the gateway and the A2A door, by hand; this loader does not translate a channel (Slack does not map) |
| `connections/` | not built | maps to tool discovery (guide 06), by hand; this loader does not translate a connection |
| `extensions/` | not built | mounted packages; the loader reads only the agent's own files |
| `hooks/` | does not map | lifecycle and stream-event subscribers; the fleet has nothing they could subscribe to |
| `skills/` | loaded | each flat `skills/<name>.md` a prompt skill `<agent>/<name>`; a module-backed or packaged skill is refused by name |
| `lib/` | loaded | nothing declared: helper code the tools import, as in Eve |
| `memory.ts` | not built | cross-session memory; the fleet's KV could hold it, and this loader does not translate it |
| `memory/` | not built | cross-session memory; the fleet's KV could hold it, and this loader does not translate it |
| `sandbox.ts` | does not map | the Vercel Sandbox API has no counterpart; confined activations (row M) are a stem's, not a tool's |
| `sandbox/` | does not map | the Vercel Sandbox API has no counterpart; confined activations (row M) are a stem's, not a tool's |
| `tools/` | loaded | each `defineTool` module an MCP tool, its schema the tool's, served for a node to bridge; a workflow tool (`task`, `serve`) does not map, and a tool with `approval` is refused — declare a `[[rule]]` with `requires_grant` (X3) instead |
| `schedules/` | not built | decided (Y6, option B): refused; an SDK route for running schedules is row Y12 |
| `subagents/` | not built | maps differently: another unit found by capability, discovery rather than a parent calling a child; this loader does not translate one |

Within a loaded slot, the loader refuses by name what it does not carry:
- **Tool fields:** a tool's fields other than `description`, `inputSchema` and `execute` (`outputSchema`,
  `toModelOutput`, `endsTurn`, `label`, `deferred`, `availableInSubagents`) are refused, never ignored.
- **Streamed results:** a tool whose `execute` returns a streamed result (an async iterable) answers an `isError`
  result naming the tool.
- **Duplicate names:** two modules that name one tool (`get_weather.ts` and `get_weather.js`) are each refused, as Eve
  refuses a slot collision.
- **The flat layout:** Eve's flat layout, with agent files in the app root beside `package.json`, is not read. The
  loader reads the `agent/` directory, and takes the agent's name from the `package.json` beside it.

**Entries that are never authored content are skipped, as Eve 0.80.0 skips them.**
- **Skipped:**
  - the directories Eve's discovery ignores as generated (`GENERATED_AGENT_DIRECTORY_NAMES` in its
    `discover/filesystem.js`: `.devtools`, `.eve`, `.next`, `.output`, `.vercel`, `node_modules`);
  - any other name beginning with `.`, such as `.DS_Store` or `.gitkeep`, at the root or inside `skills/` or `tools/`;
  - colocated tests (`*.test.*`, `*.spec.*`, `__tests__/`).
- **Refused:** anything else no slot names. That is stricter than Eve, on purpose. Eve ignores a root file no slot
  names, and warns on such a directory and ignores it. An entry the loader skipped would be one an author might be
  relying on.

Eve's approval policies map to the grant, which is stronger: the grant is verified where the call is enforced. The
loader does not translate an Eve `approval`. It refuses the tool by name and points to the `[[rule]]` that declares the
grant, which [`grant-rules.toml`](grant-rules.toml) shows.

## What this does not establish

- **The *does not map* rows:** `sandbox/` and `getSandbox()`, Workflow hooks (`start`, `resumeHook`, `createHook`, a
  `task` or `serve` tool), and `hooks/`.
- **The *not built* rows,** each refused by name rather than translated. Among them is `schedules/`, which was decided
  so (Y6, option B), with the SDK route left as row Y12.
- **Any Eve version but 0.80.0.**
- **Eve's exact dependency tree.** Eve 0.80.0 pins `undici` 8.10.2, which carries eight advisories fixed in 8.12.2.
  [`package.json`](package.json) overrides it to 8.12.2, so `npm audit` reports none. The `agent/` directory is
  unchanged, but what it runs on is not exactly Eve's tree.
- **Eve's runtime:**
  - The loader runs a tool's `execute` with an empty context. A tool that reads Eve's context (the session, the
    sandbox, `ctx.getToken`) fails at its call.
  - `agent.ts`'s model string is not routed. The fleet routes inference through its own router (row R), not through
    a gateway Eve names.
  - No model runs in this demonstration. The tool is called directly through `/mcp`, as an agent's tool call would
    be.
- **Slack and the other channel helpers.**
- **`declareEveAgent` writes the skills; it does not retract them.** It returns their names, and a prompt skill is
  deleted through the prompts route.
- **The server `serveEveTools` runs decides nothing.** The deciding door is the gateway's `/mcp`, in front of it. Bind
  the server where only the bridging node reaches it, which is the default `127.0.0.1`. It answers only a `POST` with
  `content-type: application/json` and no `Origin` header, which is what the bridge sends. So a browser page on the
  same host cannot reach a tool with a simple cross-origin request. A local process can, so the bind address is the
  boundary.
- **A grant on the tool across a restart, through `/mcp` on the fleet.** Step 7 is X3's test over the agent's unit,
  in-process. On the fleet, no call carries a grant, because a grant's holder is the agent's node and a bearer at
  `/mcp` cannot present one. Eve's `execute` is never reached under a grant.
- **The node bridges once, at connect.** A tool added to the directory after the node bridged is not seen until the
  node bridges again.
