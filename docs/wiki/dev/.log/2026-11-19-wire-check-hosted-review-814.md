## [2026-11-19] ingest | #814's review: the hosted reading takes the rule set, the matcher and the fields from the provider too

- **What the review found** (ten findings, the first two reproduced at the provider's door): the first cut read the
  hosted shape as the provider derives it and left three things the overlay's own — rules gathered from every unit,
  `starts_with("node:")` for an actor, and no rule field but `requires_mandate`. Each was a false green: a rule in a
  unit no provider loads; `node:*`, `node:`, `…@`; `cancellable`, `requires_scopes`, `signed_with`.
- **What the code adds to the review.** (1) The reference evaluator decides by the **first** allowance that matches
  (`evaluate_with_history`), so "some rule admits" is wrong whenever an earlier matching rule does not permit — shown
  at the door with a cancellable rule before a plain one. (2) `[authority]` entitles grants only
  (`declared_authority::plan`: `entitle_grants`, no `entitle(scope, …)`), so a `requires_mandate` rule never admits
  at a provider built from the declaration: `mandate-dependent edge`. (3) `requires_facts` is *indeterminate* there.
  (4) A hosted edge has to be read whenever **any** unit declares vocabulary: gating on the requirer, or on the
  hosts, left a rule in a third unit reported as `ungoverned edge` only.
- **The `tool` namespace is the same family** (F3): a member's direct `cap.invoke/tool/x` is decided as `tools/call`
  on `tool:x@{node}` for `node:{sender}` — established at the door, then read the same way.
- **One derivation** (F8): `hosted_capability_call` in `src/agent/mod.rs`, called by `derive_call` and by the check.
- **The fixture** (F4, F6): both of the worker's edges are authorised by the depots' own rules, for `*`; the worker
  and the auditor declare no vocabulary. `*` is the widest rule and the fixture says so; it cannot name a node.
- **Left open, as dated ledger rows:** a deployed edge (unit principal, any unit's rule, any-rule-admits); a component
  declared as a `[[capability]]` and a `blob`; `policy-diff` deciding the hosted entry for the unit principal.
- **Method.** Every reading has a test with two halves in one function: `wire_check::check` over a declaration, and a
  node built from `[authority]` over the same directory with a receiver counting what it is handed
  (`door_decides`, `src/agent/declared_authority_tests.rs`). Those tests build only with `compliance`.
- **The confirm round (2026-11-20).** Two false reds from the round's own additions. (C1) A rule naming a value
  (`requires_values`) was counted as a first-match blocker; `Rule::matches` skips it when the call carries another
  value, so it blocks nothing. (C2) "Any unit's vocabulary turns the hosted reading on" failed deployments that
  enforce only at a gateway: `[authority]` is node configuration, so where neither the requirer nor a host declares
  vocabulary the edge is `ungoverned edge` again, reworded to name the hosts and a rule they do not read. Stated,
  not read: a `requires_grant` or `requires_values` rule before a plain one shadows it at the provider; a host unit
  with an exposure rule or no `principal`. `mandate-dependent edge` has a door half that presents a mandate.

Pages: `docs/wiki/dev/operations.md` (the overlay's hosted reading), `docs/operations/what-is-proven.md` (the proven
row rewritten; three open rows), `docs/guide/deprecations.md` §122 (retitled), `docs/guide/20-authorising-actions.md`,
`docs/features/unit-files-and-wire-check.md`, `docs/reference/unit-file.md`, `docs/reference/cli-json.md`,
`docs/operations/capability-lifecycle.md`, the fixture's README.
