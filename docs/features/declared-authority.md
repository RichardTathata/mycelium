# Authority from the declaration

## What it is

A node can take its [evaluator](../guide/00-concepts.md#evaluator), its execution authority, provider enforcement and
its [evidence journal](../guide/00-concepts.md#evidence-journal) from configuration instead of from code. `start()`
reads this node's unit from the declaration, builds the reference evaluator from that unit's `[[rule]]`s only, and
attaches all four before the node serves a request. A node built from a file, such as the published `mycelium`
binary, then decides every door it serves by the rules it declares.

## When to use it, and when not

- **Use it when** a node is started from a configuration file and its unit's `[[rule]]`s are the policy it should
  enforce at its gateway and provider doors.
- **Not when** the policy is another engine (the Cedar adapter is attached in code) or spans units: only this unit's
  rules are loaded ([identity and lifecycle § I0](../plans/identity-and-lifecycle.md#i0--this-nodes-evaluator-authority-and-journal-from-its-declaration)).

## Turn it on

- [`authority`](../reference/configuration.md#authority): the `[authority]` table, in a build with `compliance`;
  without it `start()` refuses the table by name. An older binary ignores the table silently.
- [`principal`](../reference/unit-file.md): the unit whose `principal` equals the table's `unit` is this node's.
- [`DeclaredAuthority`](../../src/agent/declared_authority.rs): the plan `start()` builds and checks before it binds a
  port — every refusal names its cause.
- `the_gateway_door_decides_by_this_units_rules_and_records_each_decision`, in
  [`src/agent/declared_authority_tests.rs`](../../src/agent/declared_authority_tests.rs): a node configured with the
  table and attached to in no code, over the [`governed-call`](../../templates/governed-call/units/purchaser.toml)
  scaffold's unit.

## See it work

No runnable example exists. `the_provider_door_refuses_a_call_under_a_revoked_grant`, in
[`src/agent/declared_authority_tests.rs`](../../src/agent/declared_authority_tests.rs), runs it end to end: the
treasurer issues a grant at the gateway and revokes it, and the provider door refuses the call that presents it.

## What it guarantees, and what it does not

- **Guarantees:** what the attachments make enforced —
  [`ae.authorised_at_seam`](../reference/guarantee-catalogue.md#ae.authorised_at_seam),
  [`ae.recorded_before_dispatch`](../reference/guarantee-catalogue.md#ae.recorded_before_dispatch),
  [`prov.enforcement`](../reference/guarantee-catalogue.md#prov.enforcement) and
  [`authz.execution_authority`](../reference/guarantee-catalogue.md#authz.execution_authority). `mycelium doctor`
  reports them as `start()` would leave them, without opening the journal.
- **Rule:** [`ae.preflight`](../reference/rule-catalogue.md#aepreflight) decides at every door this build serves; the
  table maps each door's operation, and an unmapped door refuses the start.
- **A rule's actor is matched exactly.** An issuer at this gateway is a gateway principal (`token:<issuer>/<name>`),
  and a rule names it so. The `governed-call` scaffold's bare `treasurer`, and its missing `grant.revoke` rule, are
  why the revoked-grant test writes its own copy of the unit.
- **Not built:**
  - other units' rules;
  - the Cedar evaluator from configuration;
  - a gate spanning more than one resource authority;
  - exposure rules, which refuse the start;
  - mandate authorities' keys.

  All are listed under
  [identity and lifecycle § I0](../plans/identity-and-lifecycle.md#i0--this-nodes-evaluator-authority-and-journal-from-its-declaration).
- **Evidence:** the ledger row
  [after a revocation every door runs nothing](../operations/what-is-proven.md#revocation-every-door-runs-nothing).

## Go deeper

- **Plan:** [identity and lifecycle § I0](../plans/identity-and-lifecycle.md#i0--this-nodes-evaluator-authority-and-journal-from-its-declaration).
- **Pages it attaches:** [authorising actions](action-authorisation.md), [provider enforcement](provider-enforcement.md),
  [mandates](mandates.md), [evidence journal](evidence-journal.md).
- **Code:** [`src/agent/declared_authority.rs`](../../src/agent/declared_authority.rs) (`plan`, `doors`,
  `check_doors`, `attach`), `preflight` and `start` in [`src/agent/lifecycle.rs`](../../src/agent/lifecycle.rs),
  `evaluated_doors` in [`src/agent/http/mod.rs`](../../src/agent/http/mod.rs).
