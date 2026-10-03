# Developer tutorials — from discovery to evidence

↑ [Developer guide](../README.md) · [Example portfolio](../../../examples/README.md)

Start with `hello_mesh` and `hello_capability` in the guide's five-step path.
Then use this sequence to learn the newer surfaces one question at a time.
Keep [concepts and vocabulary](../00-concepts.md) and the
[unit-file reference](../../reference/unit-file.md) beside you as references.
Commands run from the repository root with the pinned Rust toolchain. The first
build of the WASM host is substantial; subsequent runs reuse it. No model or API
key is needed for these tutorials. Only tutorial 2's stem comparison needs Docker.

| Step | Question | Runnable evidence |
|---|---|---|
| [1 · Your first stem fleet](01-first-stem-fleet.md) | How do generic hosts acquire a missing capability? | Signed echo component, discovery, invocation, recovery |
| [2 · From code to declarations](02-code-to-declarations.md) | What moves out of application code? | The same provisioning scenario in Rust and stem units |
| [3 · A capability earns permission to run](03-shadow-and-acceptance.md) | When may proposed code become live? | Shadow execution and reviewer acceptance |
| [4 · Authority at every boundary](04-authority-boundaries.md) | Does a gateway permit authorise the final effect? | Provider admission, revocation, destination refusal |
| [5 · Replay a bundle](05-replay-a-decision.md) | What must be recorded for reproduction? | A replay bundle and a controlled divergence (the decision trace and `mycelium explain` are guide 19 § reading what a node decided) |
| [6 · Declared versus observed](06-declared-versus-observed.md) | Does a live snapshot match the design? | Schema validation, explicit identity mapping, missing providers |

**Keep four distinctions visible.** Bootstrap addresses introduce peers; discovery
finds providers. A declaration expresses intent; observation reports a node's view.
A check constrains only the boundary it covers. Replay reproduces choices captured
by its seams, not every action in an arbitrary application.

The [evidence ledger](../../operations/what-is-proven.md) remains the authority on
CI coverage and deployment limits. These tutorials point into existing mechanisms;
they do not create a second API reference.
