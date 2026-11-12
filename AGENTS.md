# AGENTS.md

For a coding agent that reads this file: Codex, Cursor, a worker Orca starts. Claude Code reads
[`CLAUDE.md`](CLAUDE.md), which holds the full rules and points here. The wiki's schema,
[`docs/wiki/AGENTS.md`](docs/wiki/AGENTS.md), is a different file.

Mycelium is an embedded Rust library for agent fleets with no coordinator; `mycelium` is its node binary.

## Using a fleet

Run `mycelium skills get fleet`. The binary prints the guide for its own version: six steps from the binary to a
governed fleet, every subcommand, and how to read `--format json`. Its source is
[`docs/reference/agent-skills/fleet.md`](docs/reference/agent-skills/fleet.md), and tests hold it to the code.

## Changing this repository

Read `CLAUDE.md` first. It applies to every agent. In short:

- **Query the wiki first:** [`docs/wiki/wiki.md`](docs/wiki/wiki.md). Code is canon; the wiki cites it.
- **Enumerate before fixing:** list every entry point that reads or writes the thing, with the grep, and test each.
- **A regression test fails first**, on the unfixed code.
- **A feature's page changes with the feature:** `docs/features/` and `docs/features/registry.toml`.
- **Gate before pushing:** `make check`. Run one cargo job at a time.
- **Stage by path**, never `git add -A`: other sessions share this checkout. Do branch work in a worktree.
- **No direct clock, RNG or filesystem access** in `src/`, `mycelium-core/src/` or `mycelium-wasm-host/src/`:
  go through `mycelium_core::sim_seam`.
- **A new `Mutex` or `RwLock` field** in a library crate needs a row in
  [`docs/wiki/dev/concurrency/lock-order.md`](docs/wiki/dev/concurrency/lock-order.md).
- A refusal names what it needs. Fix that; do not work around it.
- Commit or open a pull request only when asked. Do not merge your own.
