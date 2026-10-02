## [2026-09-30] ingest | X1: the examples' declaration directories

**What:** `docs/plans/design-time-tooling.md` §16 X1. Sixteen `examples/units/<example>/` directories
(nine top-level and co-op examples with capabilities, the lane-only `llm_council`, the provisioning
ones with `artifacts/`), `scripts/wire-check-examples.sh` in CI, the *Declared* matrix column, and the
gate `tests/wire_check_examples.rs`.

**Durable knowledge:**
- **A deletion gate needs a requirer — and a count.** *Deleting one capability block turns the row
  red* only holds when something in the directory requires what was deleted; an advertise-only
  directory stays green, and a redundant provider (`elastic_intent`'s five rush workers) is deletable
  by design. The gate deletes the **last** provider of each advertised `ns/name`.
  So each directory writes the example's own caller down as a `[[requirement]]` — which is the more
  faithful declaration anyway. Seen failing first on the first run: `catalog`'s `artifact/librarian`
  had no requirer (the installer resolves it through `MeshArtifactSource`, and now says so); and the
  clean check caught `provisioning`'s `done` lane with no consumer — the demo's own depth read.
- **The directory sits under `examples/units/`,** not `examples/<name>/units/`: a single-file example
  has no directory, and one flat loop is what a CI script and a test want. The plan's §16 wording is
  amended (rev 0.9).
- **A tool is not a capability.** The MCP-tool-only demos (`mcp_tool_authority`, `authority_drain`,
  `composed_commit`, `procurement_authority`) have no row in the unit format; they are listed as
  not-here with the reason rather than given an empty directory.

**Pages touched:** `examples/README.md` (matrix, legend, tutorial contract), `examples/units/README.md`,
`dev/examples.md`, plan §16 + row X1, CHANGELOG, `ci.yml`.
