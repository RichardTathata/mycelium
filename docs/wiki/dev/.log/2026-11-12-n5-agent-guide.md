# 2026-11-12 — N5, the agent guide the binary serves

## [2026-11-12] ingest | identity-and-lifecycle N5: `mycelium skills get fleet`, the root `AGENTS.md`

The binary embeds a guide for a coding agent (`docs/reference/agent-skills/fleet.md`, `src/agent_guide.rs`) and prints
it with `mycelium skills get fleet`; `--format json` is `mycelium.skills/1` under `cli-json.md`'s additive contract. A
root `AGENTS.md` (at most 2 KB) points Codex, Cursor and Orca's workers at it, and `CLAUDE.md`'s *Where to read what*
names both, because Claude Code reads a root `AGENTS.md` only where there is no `CLAUDE.md`.

The durable part is how a served text is kept true. The guide is compared with its sources by tests in
`src/agent_guide.rs`, each a set or sequence equality in both directions, so a change on either side fails until the
other follows:

- the walk's fenced commands against the invocations `.github/workflows/one-hour.yml` runs, in order;
- the subcommand table against `main`'s dispatch arms and against the usage text;
- the schema ids against `docs/reference/cli-json.md`;
- the sizes, the `CLAUDE.md` line, and no machine path or credential in what is served.

A new subcommand therefore needs its dispatch arm, its usage line and its row in the guide in one change, and a new
step in the `one-hour` job needs its command in the guide. The comparison is of texts: the six steps run only in the
`one-hour` job, against a published release.

Two names to keep apart: a *skill* served by `mycelium skills` is a guide an agent reads; a prompt skill
(`skill.invoke`) is something a node serves. And the root `AGENTS.md` is not `docs/wiki/AGENTS.md`, the wiki's schema.

Touched: `docs/features/agent-guide.md` and the registry, `docs/guide/00-concepts.md` (`#agent-guide`),
`docs/capabilities.md`, `docs/reference/cli-json.md`, the plan's N5 evidence row, CHANGELOG. No wiki page changed.
