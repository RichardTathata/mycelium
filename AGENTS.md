# AGENTS.md

For a coding agent that reads this file: Codex, Cursor, a worker Orca starts. Claude Code reads
[`CLAUDE.md`](CLAUDE.md), which holds the full rules and points here. The wiki's schema,
[`docs/wiki/AGENTS.md`](docs/wiki/AGENTS.md), is a different file.

Mycelium is an embedded Rust library for agent fleets with no coordinator; `mycelium` is its node binary.

## Using a fleet

The binary prints the guide for its own version: six steps to a governed fleet, every subcommand, and how to read
`--format json`. With the published binary on `PATH`:

    mycelium skills get fleet

In a checkout, with the release's features:

    cargo run --features cli,tls,compliance,a2a,llm,metrics --bin mycelium -- skills get fleet

Its source is [`docs/reference/agent-skills/fleet.md`](docs/reference/agent-skills/fleet.md); tests hold it to the code.

## Changing this repository

Read `CLAUDE.md` first; it applies to every agent. Some of its rules, each with its home:

- **Query the wiki first:** [`docs/wiki/wiki.md`](docs/wiki/wiki.md) (`CLAUDE.md`, the wiki workflow).
- **Enumerate before fixing:** every entry point, with the grep, each tested (`CLAUDE.md`, verification rule 1).
- **A feature's page changes with the feature:** `docs/features/` and its registry (`CLAUDE.md`, rule 5).
- **Fail-first:** the test that failed before the fix, and its output (`.github/pull_request_template.md`).
- **Gate before pushing:** `make check` (`CLAUDE.md`, build and test gates).
- **No direct clock, RNG or filesystem access** in the scanned crates: use `mycelium_core::sim_seam` (`CLAUDE.md`,
  hot invariants).
- **A new `Mutex` or `RwLock` field** in a library crate needs a lock-order row (`CLAUDE.md`, hot invariants).
