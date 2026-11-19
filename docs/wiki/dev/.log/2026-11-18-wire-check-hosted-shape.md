## [2026-11-18] ingest | `wire-check` reads an edge to a hosted component as its provider decides it

- **The gap** (`what-is-proven.md#wire-check-does-not-know-the-hosted-capability-shape`, open since 2026-11-14):
  the overlay tried `skill.invoke` and `tools/call` for the requirer's unit principal; a provider that hosts a
  component outside `tool` decides `cap.invoke` on `cap:{ns}/{name}@{node}` for `node:{sender}`. False red on the
  `cap.invoke` rule, false green on a `skill.invoke` one — the co-op fixture (`auditor.toml:11`) was green in CI
  and at the release gate.
- **What was built (the maintainer's option A).** `wire_check::hosted_call_shape`, used only where the edge's
  provider is a `wasm-component` artifact description; `call_shapes` and its two-shape return are unchanged, the
  JSON's `operations` carries a third item for such an edge, and one finding kind is new, `node-decided edge`
  (warning). A hosted edge is read on the hosted shape alone.
- **The actor is the node, in the rule and in the mandate.** `provider_enforcement::check` takes `node:{sender}`
  for an unframed call, and a member's own mandate travels under `node:{self}` (`rpc_call_with_mandate`). So the
  overlay passes `*` and `node:` actors, a `node:`-held mandate, and warns where only the unit principal would
  admit. It is a warning because the checker knows no placement and does not claim the refusal.
- **`[authority]` loads one unit's rules; the overlay reads every unit's.** Found moving the fixture's rule: left
  in `auditor.toml` it would pass the check and never be loaded by the depot that decides. The rule is now declared
  in `depot-a.toml` and `depot-b.toml`. The overlay did not check this placement at first; it does since #814's review
  (`2026-11-19-wire-check-hosted-review-814.md`).
- **Enumeration** (rule 1). Where a door derives a capability call: `provider_enforcement::derive_call` (five arms:
  `mcp.invoke`, `llm.invoke`, the `cap.invoke/` family, `skill.invoke`, any other protected kind) and its actor in
  `check`; the gateway's `/a2a` (`skill.invoke` on `skill:{id}@{target}`, `a2a.rs`) and `/mcp` doors; the gateway's
  raw routes refuse the family (`authz.rs`). Where the check builds a shape or an actor: `call_shapes`,
  `hosted_call_shape`, the overlay's loop, and `faults.rs`' policy-diff call set, which reads `Edge::operations`
  and so decides the third shape too.
- **Fail-first.** On the overlay as it was: the `cap.invoke` rule `unauthorisable edge`; the old fixture
  declaration exit 0; the provider-door test red at its first reading. Plants for the pin and the silent pass.
- **Not built:** the provider deciding on a unit principal the node can prove it speaks for
  (identity-and-lifecycle §9, O9, with the Promise-Theory framing); a component declared as a plain
  `[[capability]]` is still read as a skill or a tool; the `tool` namespace's actor on a direct call.

Pages: `docs/wiki/dev/operations.md` (the overlay's hosted shape), `docs/operations/what-is-proven.md` (the row
closed by a proven row), `docs/guide/deprecations.md` §108 and §122, `docs/guide/20-authorising-actions.md`,
`docs/features/unit-files-and-wire-check.md`, `docs/features/provider-enforcement.md`,
`docs/operations/capability-lifecycle.md`, `docs/reference/unit-file.md`, `docs/reference/declaration.schema.json`,
`docs/plans/identity-and-lifecycle.md` §9 (O9).
