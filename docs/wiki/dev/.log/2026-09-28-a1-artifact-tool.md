# 2026-09-28 — A1: the artifact tool

**What:** `mycelium-wasm-host/src/tools.rs` (`publish`, `list`, `verify`, `render_entry`,
`signing_key_from_hex`, `publisher_from_str`, `kind_from_name`/`kind_name`) and the `mycelium-artifact`
binary (feature `stem`). D10 built: the signed line-hex manifest stays the truth; the description
(`mycelium::wire_check::ArtifactDescription`, the same TOML the checker reads) is the input and the tool
derives the one from the other.

**Durable knowledge:**
- The tool is a **wasm-host binary**, not `mycelium artifact …`: the manifest and entry types live in
  the wasm-host crate and `mycelium` cannot depend on it (the same reason D18's runtime half moved).
- A publisher key is a **32-byte seed as 64 hex characters** — the form the federation fixtures already
  use — from a file (`--key`) or an environment variable (`--key-env`), never a unit file.
- `verify --descriptions` is the drift check: a description's bytes must hash to the address the
  manifest carries for its capability (matched by `ns/name`); a changed file under an unchanged manifest
  is named with both hashes. `publish` is idempotent (same bytes → one line, one blob).
- `list` output **parses back** as a description (`ArtifactDescription::from_toml_str`), which is
  the round-trip the gate asserts; the comment header carries what a description cannot (address,
  size, signer).

**Pages touched:** `docs/operations/artifacts.md` §2 (the command before the code); the lifecycle
page's publish step; the plan's A1 row; `CHANGELOG.md`.
