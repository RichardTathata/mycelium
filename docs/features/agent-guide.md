# Agent guide

## What it is

`mycelium skills get fleet` prints the [agent guide](../guide/00-concepts.md#agent-guide): a short Markdown text for a
coding agent, embedded in the binary, so an installed `mycelium` describes itself without a checkout. The `fleet`
guide holds the six steps from the binary to a governed fleet — the steps the `one-hour` job runs against each
release — a table of every subcommand with the JSON document it prints, and how to read that output. `mycelium skills
list` names the guides, and `--format json` prints either as one `mycelium.skills/1` document. A root
[`AGENTS.md`](../../AGENTS.md) points Codex, Cursor and a worker Orca starts at the command; Claude Code reads
`CLAUDE.md`, which names both. A skill in this sense is a guide an agent reads, not a
[prompt skill](../guide/00-concepts.md#skill) a node serves.

## When to use it, and when not

- **Use it when** an agent is about to drive the binary: it gets the commands that are measured on every release, the
  subcommands this binary has and no others, and the rule for reading `--format json`.
- **Not when** you want a tool the agent calls rather than a text it reads: an MCP server over the node's operations
  is [`identity-and-lifecycle.md`](../plans/identity-and-lifecycle.md) row N1, and a fleet per worktree its row N4;
  neither is built, and the guide names neither.

## Turn it on

This feature is turned on in code.

- [`SKILLS`](../../src/agent_guide.rs) — every guide, embedded at compile time from
  [`docs/reference/agent-skills/`](../reference/agent-skills/fleet.md).
- [`Skill::text`](../../src/agent_guide.rs) — the guide as served, with `{{MYCELIUM_TAG}}` replaced by the binary's
  own release tag, as `init` does for a scaffold.
- [`get_json`](../../src/agent_guide.rs) and [`list_json`](../../src/agent_guide.rs) — the `mycelium.skills/1`
  documents ([`cli-json.md`](../reference/cli-json.md)).
- `the_guides_commands_are_the_one_hour_jobs`, in [`src/agent_guide.rs`](../../src/agent_guide.rs): the commands in
  the guide's walk are the ones `.github/workflows/one-hour.yml` runs, in its order, with none left out on either side.

## See it work

No runnable example exists: the [example index's capability matrix](../../examples/README.md#the-capability-matrix)
has no row for this feature. The tests that hold the guide to the code are in
[`src/agent_guide.rs`](../../src/agent_guide.rs): `the_guides_commands_are_the_one_hour_jobs`,
`the_workflow_extraction_sees_each_form_of_a_command`, `the_guides_expectations_and_configuration_are_the_one_hour_jobs`,
`the_guides_subcommands_are_the_binarys` (the guide's table, the arms `main` dispatches and the usage `--help` prints
are one set), `the_guides_schema_ids_are_the_references`, `the_sizes_are_bounded`,
`the_root_agents_file_and_the_claude_line_name_the_guide`, `nothing_served_names_a_machine_or_a_secret` and
`every_guide_on_disk_is_embedded`. The command is run through the built binary in
[`tests/cli_json.rs`](../../tests/cli_json.rs): `skills_as_json_match_their_goldens_and_the_text_form_is_the_guide` and
`skills_refuses_on_stderr_with_nothing_on_stdout`.

## What it guarantees, and what it does not

- The guide adds no guarantee to a fleet. What it promises is about itself: its walk is the `one-hour` job's
  commands, its subcommands are the binary's, its schema ids are the reference's, each by a test that fails when
  either side moves alone.
- The walk is the Python path of [the first hour](../guide/tutorials/07-the-first-hour.md). The job that times it runs
  against a published release, not a checkout, so the test compares the texts; it does not run the six steps.
- The guide is at most 6 KB and the root `AGENTS.md` at most 2 KB, gated. Neither carries a path of the machine that
  built the binary, a credential or a key; the one URL is the SDK's archive at the binary's own tag.
- The plan row is [`identity-and-lifecycle.md`](../plans/identity-and-lifecycle.md) N5. Not built: a guide served over
  a gateway route or an MCP resource (N1); an `AGENTS.md` written into a scaffold by `init`; any guide but `fleet`.

## Go deeper

- The guide's source: [`docs/reference/agent-skills/fleet.md`](../reference/agent-skills/fleet.md).
- Reference: [the CLI's JSON documents](../reference/cli-json.md).
- Guide: [the first hour](../guide/tutorials/07-the-first-hour.md), the walk as a tutorial.
- No runbook and no design record; the plan row is the record.
- Code: [`src/agent_guide.rs`](../../src/agent_guide.rs), [`src/main.rs`](../../src/main.rs).
