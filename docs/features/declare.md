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
- The flags: `--gateway <url>` (default `http://127.0.0.1:8300`), `--token <bearer>` (else `MYCELIUM_GATEWAY_TOKEN`),
  `--watch`, `--poll-ms <n>` (how often the files are read), `--interval-secs <n>` (the heartbeat), `--lease-secs <n>`
  (the lease a declaration is held under). `--poll-ms` at or above a third of the lease is refused: the declaration
  would lapse between heartbeats.
- `a_unit_edit_is_redeclared_and_the_old_handle_retracted`, in [`src/declare.rs`](../../src/declare.rs): a real
  gateway node, a unit declared, resolved over HTTP, renamed, re-declared, and the old capability gone.

## See it work

No runnable example exists: the [example index's capability matrix](../../examples/README.md#the-capability-matrix)
has no row for this feature. The CI job `Scaffold` runs it against the Python scaffold's node: declare, watch, an
edit reaching the fleet, a refused unit leaving the previous declaration, Ctrl-C retracting. The tests are in
[`src/declare.rs`](../../src/declare.rs): `a_unit_edit_is_redeclared_and_the_old_handle_retracted`,
`a_unit_wire_check_refuses_keeps_the_previous_declaration`,
`a_unit_the_gateway_refuses_is_rejected_once_and_its_previous_declaration_put_back`,
`a_lost_handle_is_declared_again_at_the_next_tick`, `a_unit_removed_from_the_directory_is_retracted`,
`a_refused_token_is_an_error_naming_the_route` and `a_skill_file_becomes_a_prompt_template`.

## What it guarantees, and what it does not

- The declaration the fleet holds is the last one `wire-check` passed and the gateway accepted; a refused edit
  changes nothing — a unit the gateway refuses after the check passed (a reserved namespace, a stem-only section)
  has its previous text declared again, and the refusal is printed once, not retried every poll.
- Under `--watch` a declaration lives under a lease the watch heartbeats within; if the watch dies, the node
  retracts it when the lease lapses, as for any SDK agent ([the lease](../guide/10-language-bridges.md)).
  A handle the node no longer knows — after a node restart, or a lapsed lease — fails the heartbeat and is declared
  again at the next tick whether or not a file changed. Once, without `--watch`, no lease is sent: the declaration
  lives until the node stops or it is retracted, as an SDK agent's does.
- A good edit briefly shows no provider: a unit's previous handle is retracted before the new text is declared (one
  KV key per `ns/name`, so the other order would tombstone the new advert), and the window between the two is a
  reader's moment of absence. Inherent, and stated here rather than hidden.
- A removed unit file is retracted; a removed skill file's template is deleted.
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
