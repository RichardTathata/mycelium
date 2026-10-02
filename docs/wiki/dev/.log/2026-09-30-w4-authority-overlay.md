## [2026-09-30] ingest | W4: the authority overlay in the checker

**What:** `wire_check::check` gains an overlay (`CheckOptions::authority`, default on): a governed
requirer's edge must be admissible by some declared `[[rule]]`, and a rule that requires a mandate
scope needs a declared `[[mandate]]` held by the requirer's principal enumerating the operation.
Findings `unauthorisable edge` (error, with the nearest reason) and `ungoverned edge` (warning).

**Durable knowledge:**
- **Which call shapes an edge could take** is the provider enforcement point's own derivation
  (`provider_enforcement.rs` § Deriving what the call is): a skill is `skill.invoke` on
  `skill:{ns}/{name}`, an MCP tool `tools/call` on `tool:{name}`. The overlay admits an edge if
  either shape is authorised; a deployment that wants one shape only says so in its rules.
- **The operation string is the gateway's rule restated.** `mandate_operation_form` is
  `{operation}:{resource_key}`; `mycelium::mandate_operation` is behind the attesting build, so the
  checker carries its own copy and a test under `gateway+tls` pins them equal. A `[[mandate]]`'s
  `operations` must be written in that form — the co-op fixture had `route.optimize` and was wrong
  the moment the overlay existed, which is the kind of drift the overlay is for.
- **Governed** means the unit declared any authority vocabulary. An ungoverned unit beside governed
  ones is a warning, not an error: absence of a rule is not a decision, and a deployment may govern
  only its sensitive edges.
- `CheckOptions` now has a manual `Default` (authority true); a literal breaks, `..Default::default()`
  does not.

**Pages touched:** the plan's W4 row; `CHANGELOG.md`; the module doc's table.
