# Declare

## What it is

`mycelium declare [project-dir] [--watch]` sends a project's declaration to a gateway and, under `--watch`, keeps it
current as the files change — the [declaration watch](../guide/00-concepts.md#declaration-watch). The project's
`units/*.toml` go through `POST /gateway/units/declare`, each unit's previous handle retracted first so a renamed
capability does not linger; its `skills/<ns>/<name>.md` become prompt templates through the route that owns
`prompts/`. Every tick runs the same `wire-check` the subcommand runs first: a finding leaves the last good
declaration standing and says so. Nothing is restarted — the loop is over data the node accepts live.

## When to use it, and when not

- **Use it when** an SDK agent's unit file or a skill's text is what you are editing, against a node with a
  gateway (the [Python scaffold](../../templates/governed-call-py/README.md)'s node, or any node with `http_port`),
  and you want the fleet to see each save without restarting anything.
- **Not when** the program declares in-process, as the Rust scaffold's nodes do through `run_capability_probes`
  (a change to `src/` is `cargo run`); or when you need a watch that records what it did for replay — this one polls
  and runs no node.

## Turn it on

This feature is turned on in code.

- [`Watch`](../../src/declare.rs) — holds the handles of one project's declarations against one gateway; `tick`
  reads, compares, checks and declares; `heartbeat` keeps the handles alive within their lease; `retract` drops them.
- [`run`](../../src/declare.rs) — the command: once, or polled until Ctrl-C, which retracts.
- The token is `--token` or `MYCELIUM_GATEWAY_TOKEN`; the routes need `cap:write` for units and `llm:write` for
  skills ([rbac](../operations/rbac.md)).
- `a_unit_edit_is_redeclared_and_the_old_handle_retracted`, in [`src/declare.rs`](../../src/declare.rs): a real
  gateway node, a unit declared, resolved over HTTP, renamed, re-declared, and the old capability gone.

## See it work

No runnable example exists: the [example index's capability matrix](../../examples/README.md#the-capability-matrix)
has no row for this feature. The CI job `Scaffold` runs it against the Python scaffold's node: declare, watch, an
edit reaching the fleet, a refused unit leaving the previous declaration, Ctrl-C retracting. The tests are in
[`src/declare.rs`](../../src/declare.rs): `a_unit_edit_is_redeclared_and_the_old_handle_retracted`,
`a_unit_wire_check_refuses_keeps_the_previous_declaration`, `a_refused_token_is_an_error_naming_the_route` and
`a_skill_file_becomes_a_prompt_template`.

## What it guarantees, and what it does not

- The declaration the fleet holds is the last one `wire-check` passed; a refused edit changes nothing.
- A declaration lives under a lease the watch heartbeats; if the watch dies, the node retracts it when the lease
  lapses, as for any SDK agent ([units/declare](../reference/unit-file.md#who-reads-which-section)).
- It declares; it does not enforce. A unit's mandate and rule come back `not_enforced` from a node that attaches no
  evaluator, and the tick prints them.
- **Not built:** the plan's *Input*-class seam for a filesystem event source — the watch polls modification time and
  size, and a watched session is not something it records. The plan row is
  [`adopter-surface.md`](../plans/adopter-surface.md) X10.

## Go deeper

- Reference: [the unit file](../reference/unit-file.md) and [who reads which section](../reference/unit-file.md#who-reads-which-section).
- Guide: [language bridges](../guide/10-language-bridges.md), where `POST /gateway/units/declare` is explained.
- No runbook and no design record; the plan row is the record.
- Code: [`src/declare.rs`](../../src/declare.rs), [`src/main.rs`](../../src/main.rs).
