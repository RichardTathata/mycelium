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
- [`persistence`](../reference/configuration.md#persistence): required beside the table — a grant's use markers are
  kept by the WAL and the execution authority's epochs under the persistence directory, so without it a restart
  would forget which grants were used.
- [`principal`](../reference/unit-file.md): the unit whose `principal` equals the table's `unit` is this node's.
- [`DeclaredAuthority`](../../src/agent/declared_authority.rs): the plan `start()` builds and checks before it binds a
  port — every refusal names its cause.
- `the_gateway_door_decides_by_this_units_rules_and_records_each_decision`, in
  [`src/agent/declared_authority_tests.rs`](../../src/agent/declared_authority_tests.rs): a node configured with the
  table and attached to in no code, over the [`governed-call`](../../templates/governed-call/units/purchaser.toml)
  scaffold's unit; `the_scaffolds_node_toml_with_an_authority_table_loads_and_decides_through_the_config_path` loads the
  scaffold's own `node.toml` through the binary's loader.

## See it work

No runnable example exists. `the_provider_door_refuses_a_call_under_a_revoked_grant`, in
[`src/agent/declared_authority_tests.rs`](../../src/agent/declared_authority_tests.rs), runs it end to end: the
treasurer issues a grant at the gateway and revokes it, and the provider door refuses the call that presents it.

## What it guarantees, and what it does not

- **Guarantees:** what the attachments make enforced —
  [`ae.authorised_at_seam`](../reference/guarantee-catalogue.md#ae.authorised_at_seam),
  [`ae.recorded_before_dispatch`](../reference/guarantee-catalogue.md#ae.recorded_before_dispatch),
  [`prov.enforcement`](../reference/guarantee-catalogue.md#prov.enforcement) and
  [`authz.execution_authority`](../reference/guarantee-catalogue.md#authz.execution_authority) and
  [`authz.durable_epochs`](../reference/guarantee-catalogue.md#authz.durable_epochs), so a node under
  `secure-single-domain` can meet the profile's authority requirements from the table. `mycelium doctor` reports them
  as `start()` would leave them: it checks that the journal and the epochs could be opened for writing, reads an existing epochs journal, and opens neither. For a path that does not exist yet the check creates and removes a probe file (`.mycelium-authority-probe-<pid>`) in its nearest existing directory; a probe left behind by a crash refuses a later check whose process has the same id, until it is removed.
- **One source of authority:** an evaluator, an execution authority or an evidence journal attached in code beside
  the table refuses the start.
- **Rule:** [`ae.preflight`](../reference/rule-catalogue.md#aepreflight) decides at every door this build serves; the
  table maps each door's operation, and an unmapped door refuses the start.
- **A rule's actor is matched exactly.** An issuer at this gateway is a gateway principal (`token:<issuer>/<name>`),
  and a rule names it so; the scaffold fixes its issuer (`gateway_identity_issuer`) so its unit can.
- **This node only:** a bare `tool:<name>`, `skill:<ns>/<name>` or `prompt:<ns>/<name>` (the last since 2.41) is
  qualified to this node, so the gateway admits only the tools, skills and prompt skills this node serves.
- **The clock:** the gate's guarantee assumes every issuing authority's clock is within `skew_ms` of this node's.
- **Epochs:** no route or setting installs a newer epoch on a table-only node, so its gate stays at the first epoch;
  an embedder installs one in code.
- **Not built:**
  - other units' rules;
  - the Cedar evaluator from configuration;
  - a gate spanning more than one resource authority;
  - exposure rules, which refuse the start;
  - mandate authorities' keys.

  All are listed under
  [identity and lifecycle § I0](../plans/identity-and-lifecycle.md#i0--this-nodes-evaluator-authority-and-journal-from-its-declaration).
- **Evidence:** no ledger row yet. The tests above are the evidence; the ledger's revocation row covers authority
  attached in code, not this table.

## Go deeper

- **Plan:** [identity and lifecycle § I0](../plans/identity-and-lifecycle.md#i0--this-nodes-evaluator-authority-and-journal-from-its-declaration).
- **Pages it attaches:** [authorising actions](action-authorisation.md), [provider enforcement](provider-enforcement.md),
  [mandates](mandates.md), [evidence journal](evidence-journal.md).
- **Code:** [`src/agent/declared_authority.rs`](../../src/agent/declared_authority.rs) (`plan`, `doors`,
  `check_doors`, `attach`), `preflight` and `start` in [`src/agent/lifecycle.rs`](../../src/agent/lifecycle.rs),
  `evaluated_doors` in [`src/agent/http/mod.rs`](../../src/agent/http/mod.rs).
