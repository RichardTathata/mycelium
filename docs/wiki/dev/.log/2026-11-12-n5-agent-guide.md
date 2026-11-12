# 2026-11-12 — N5, the agent guide the binary serves

## [2026-11-12] ingest | identity-and-lifecycle N5: `mycelium skills get fleet`, the root `AGENTS.md`

The binary embeds a guide for a coding agent (`docs/reference/agent-skills/fleet.md`, `src/agent_guide.rs`) and prints
it with `mycelium skills get fleet`; `--format json` is `mycelium.skills/1` under `cli-json.md`'s additive contract. A
root `AGENTS.md` (at most 2 KB) points Codex, Cursor and Orca's workers at it, and `CLAUDE.md`'s *Where to read what*
names both, because Claude Code reads a root `AGENTS.md` only where there is no `CLAUDE.md`.

The durable part is how a served text is kept true. The guide is compared with its sources by tests in
`src/agent_guide.rs`:

- the walk's fenced commands against the invocations `.github/workflows/one-hour.yml` runs: one sequence, equal both
  ways, apart from three lines the test names (the guide's virtual-environment line; the workflow's two queries of
  its own environment). The extraction fails on a workflow line it cannot classify, and the guide may hold no fence
  but `sh` and `toml`;
- what each step prints and writes, per step: every expectation the guide quotes is one that step greps for, every
  grep is quoted or in a list the test holds, and each TOML block equals that step's `printf`;
- the subcommand table against `main`'s dispatch arms and against the usage text: one set;
- the schema ids against `docs/reference/cli-json.md` and against the literals in each command's source;
- the sizes, the `CLAUDE.md` line, the release's feature set, and no machine path or credential in what is served.

Not compared: a subcommand's sub-verbs and flags. The adversarial review of #770 (nine findings) is why the
extraction fails closed and the expectation check is per step; its first form skipped what it did not recognise and
checked expectations by `contains` over the whole workflow.

A new subcommand therefore needs its dispatch arm, its usage line and its row in the guide in one change, and a new
step in the `one-hour` job needs its command in the guide. The comparison is of texts: the six steps run only in the
`one-hour` job, against a published release.

Two names to keep apart: a *skill* served by `mycelium skills` is a guide an agent reads; a prompt skill
(`skill.invoke`) is something a node serves. And the root `AGENTS.md` is not `docs/wiki/AGENTS.md`, the wiki's schema.

Touched: `docs/features/agent-guide.md` and the registry, `docs/guide/00-concepts.md` (`#agent-guide`),
`docs/capabilities.md`, `docs/reference/cli-json.md`, the plan's N5 evidence row, CHANGELOG. No wiki page changed.
