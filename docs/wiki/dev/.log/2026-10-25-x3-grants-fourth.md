# 2026-10-25 — X3's fourth PR: the `grant` fault class, the one-hour job's step 4

- `Class::Grant` in `src/faults/grant.rs`: the declaration's first `requires_grant = true` rule driven on the agent's node
  of the fleet — default-deny, the declared issuer's grant with a stranger refused, the restart over the WAL, the binding,
  the record. Gated on `compliance` (member keys). Issues through `decide_grant`, factored out of the route.
- `[[rule]]` gained `requires_grant`; `RuleDecl` and `MandateDecl` refuse unknown keys (§65); `wire-check` warns
  `unissuable grant`.
- Both scaffolds declare `tool:purchase.approve` under the treasurer's grant; the `Scaffold` job and the one-hour job's
  step 4 run the class — the job measures all six steps from the first release carrying it.
- Pages: `docs/features/grants.md`, `docs/features/test-faults.md`, `docs/reference/unit-file.md`, `00-concepts.md`,
  `deprecations.md` §65, the plan's X3 row and §1.1, `dev/operations.md`.
