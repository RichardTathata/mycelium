# The Eve loader

## What it is

`mycelium-ts`'s Eve loader runs an [Eve](https://www.npmjs.com/package/eve) agent directory on a fleet without changing
it. `loadEveAgent` reads the directory as one pinned version of Eve publishes it, 0.80.0, and refuses a directory
beside any other. Each slot of the directory has a status. The instructions and the flat Markdown skills become prompt
skills, each `defineTool` module an MCP tool served for a node to bridge, and each schedule a `[[schedule]]` the gateway
runs. Every slot the loader does not load — hooks, a sandbox, memory, channels, subagents — is refused by name, never
skipped, so an agent that relied on one does not seem to work without it. On the fleet the tool is reached through
`/mcp`, so a node with an [action evaluator](action-authorisation.md) decides each call, and one with an
[evidence journal](evidence-journal.md) records it — attached in code (`with_action_evaluator`,
`with_evidence_journal`), as the example's node does; the published node attaches neither. This is a demonstration, not a compatibility promise.

## When to use it, and when not

- **Use it when** an agent written for Eve should run on a fleet and be governed by its doors — its tool calls decided
  and recorded, a grant required where policy says.
- **Not when** the agent depends on a slot the loader refuses, on Eve's own runtime (its `model` is read and not
  routed), or on any Eve version but the pinned one.

## Turn it on

From TypeScript, with `mycelium-ts` (`import … from "mycelium-ts"`), against a node's gateway:

- `loadEveAgent(dir)` in [`mycelium-ts/src/eve.ts`](../../mycelium-ts/src/eve.ts): read the directory. The slots that
  do not map are refused together, one `EveRefusedError` naming every one; what is found later, inside a slot that
  does map, is refused in its own `EveRefusedError` — a key of `agent.ts` other than `model`, a skill whose name is
  not one key segment, a tool or a schedule that cannot be carried — so a directory may take more than one run to
  hear every refusal.
- `serveEveTools(agent)`: the tools as an MCP server, which a node bridges with `connect_mcp_server`
  ([MCP tools](mcp-tools.md)).
- `declareEveAgent(agent, { mycelium, prompts })`: the unit through `POST /gateway/units/declare` (`cap:write`), and
  the instructions and skills as prompt skills through `PUT /gateway/prompts/{agent}/{skill}` (`llm:write`; the route exists on a node built with `llm`) — scopes
  checked only on a node built with `compliance` (the published node is; without it any valid bearer declares).
- `runEveSchedules(agent, mycelium, { onMarkdown })`: its schedules through the gateway's schedule route
  ([recurring work](recurring-work.md), `schedule:run`, checked only on a node built with `compliance`), each cron read only when it is a period and a phase. A
  Markdown schedule's prompt goes to `onMarkdown`; without one it refuses every Markdown schedule before opening a
  stream.
- The slot table is `EVE_SLOTS` in the same file; `mycelium-ts/tests/eve.test.ts` holds it equal to the example's
  README and checks that every slot not loaded is refused by name (jest titles, so not in the form a Rust test takes).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`eve_fleet_node`](../../examples/README.md#the-capability-matrix): Eve's own weather example, its `agent/` directory
compared byte for byte with the one the installed package prints, loaded and run on three nodes. Its tool is called
through `/mcp`, permitted for one token and refused for another, the refusal read back with `mycelium explain`, and a
grant on the tool held across a restart by `mycelium test --faults grant`. CI runs it in the `eve-fleet` job, which
fails unless the run prints `EVE FLEET OK`.

## What it guarantees, and what it does not

- No guarantee or rule id of its own: the tool's calls are [`ae.preflight`](../reference/rule-catalogue.md#aepreflight)'s
  at the gateway that receives them.
- **Not built** (plan row Y6, [`adopter-surface.md`](../plans/adopter-surface.md)): `subagents/`, `connections/` and
  `channels/` are refused, not translated; the TypeScript project template; on the fleet, a grant held across a
  restart through `/mcp` — the run's grant step drives the agent state machine's door in-process, not Eve's `execute`;
  following Eve as its preview moves.
- **Evidence:** the [what-is-proven](../operations/what-is-proven.md) ledger has no row for the loader on its own.

## Go deeper

- **Guide:** [the example's README](../../examples/eve_weather/README.md), which carries the slot table.
- **Runbook:** no runbook of its own.
- **Design:** decision D24 and row Y6 of [`plans/adopter-surface.md`](../plans/adopter-surface.md).
- **Code:** [`mycelium-ts/src/eve.ts`](../../mycelium-ts/src/eve.ts) and [`examples/eve_fleet_node.rs`](../../examples/eve_fleet_node.rs).
