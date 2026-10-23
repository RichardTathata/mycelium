# Templates — what `mycelium init` writes

Each directory here is a project of its own: `mycelium init <dir> --template <name>` writes a copy of it, with
`{{MYCELIUM_TAG}}` replaced by the tag of the release the binary came from. The directory is the source of truth
and CI builds and runs it as a project; the binary embeds it at compile time (`src/init.rs`), so the two cannot
differ — a template that drifts from the API fails CI before it fails a user (`docs/plans/adopter-surface.md`, X2).

| Template | Language | What it shows |
|---|---|---|
| `governed-call` | Rust | two in-process nodes; a member's direct call to a provider's tool, permitted and refused by a policy enforced at the provider and recorded in an evidence journal; a prompt skill whose text is a file |
| `governed-call-py` | Python | the published `mycelium` node under a named-token table; an SDK agent declares a unit, writes a key, and a second agent is refused by the gateway's own scope rule |

The layout every template follows: `node.toml` (the node's configuration, which `mycelium doctor` reads),
`units/*.toml` (unit files `wire-check` reads and the node declares), `skills/*.md` (prompt-skill text, one file
per skill, where a template has one), and the program (`src/main.rs` or `main.py`).
