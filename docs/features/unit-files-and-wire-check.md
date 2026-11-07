# Unit files and wire-check

## What it is

A [unit file](../guide/00-concepts.md#unit-file) is one TOML file per deployable unit, declaring what it offers, needs,
groups and hosts, its [lanes](../guide/00-concepts.md#lane), the authority it expects and the rules it carries. One loader
reads it for three consumers: `mycelium wire-check`, the stem, and an SDK agent's `POST /gateway/units/declare`.
[`wire-check`](../guide/00-concepts.md#wire-check) is a pure function over a directory of unit files that applies the
mesh's own match rule and names, by kind, everything that could not bind — so a deployment is checked before anything
runs. Its answer is *would bind*, never *is bound*; the runtime keeps the last word, and a
[wiring report](../guide/00-concepts.md#wiring-report) set beside an observation shows the difference.

## When to use it, and when not

- **Use it when** a fleet's units are written down and you want an unwired requirement, a schema mismatch or an
  unauthorisable edge found in CI rather than in production.
- **Not when** you need to know what is bound now: the check never observes a fleet. `[[lane]]`, `[[mandate]]` and
  `[[rule]]` are read only by the check's overlay — at runtime a stem logs them and an SDK agent reports them in
  `not_enforced` ([unit-file § Who reads which section](../reference/unit-file.md#who-reads-which-section)).

## Turn it on

This feature is turned on in code: the check is in the `mycelium` binary, and a file loads with
[`NodeCapabilityConfig::load_from_file`](../../src/capability_config.rs). Every example's declaration directory under `examples/units/` —
[`provisioning/provider-a.toml`](../../examples/units/provisioning/provider-a.toml) among them — is checked by CI with
`scripts/wire-check-examples.sh`.

- `mycelium wire-check <units-dir> [--library <dir>] [--schemas <dir>] [--format text|json|dot] [--strict-deployed]
  [--no-authority]` — a file that does not load exits 2 naming the field
  ([capability lifecycle § Check](../operations/capability-lifecycle.md#2--check--before-anything-runs)).
- Sections: `[[lane]]` ([unit-file § lane](../reference/unit-file.md#lane)), `[[mandate]]`
  ([unit-file § mandate](../reference/unit-file.md#mandate)) and `[[rule]]` ([unit-file § rule](../reference/unit-file.md#rule)),
  beside the offers, needs and hosting sections other pages own.
- The JSON output's schema is [`declaration.schema.json`](../reference/declaration.schema.json).
- In code, [`check`](../../src/wire_check.rs).
- `every_error_class_is_reported_by_name`, in [`src/wire_check.rs`](../../src/wire_check.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`compare_stem_observations`](../../examples/README.md#the-capability-matrix): [`first_stem_fleet`](../../examples/README.md#the-capability-matrix)'s exported
wire-check report beside one observer's snapshot of who provides `demo/echo` — what would bind against what did. CI
runs it, and the [sixth tutorial](../guide/tutorials/06-declared-versus-observed.md) walks it.

## What it guarantees, and what it does not

- No guarantee or rule id of its own: the check applies [`cap.match`](../reference/rule-catalogue.md#capmatch), the same
  rule the resolver uses.
- **Not built:** placement — `[hosts]` describes eligibility and the check never says which host installs; an observed
  binding — the answer is *would bind*; enforcement of `[[mandate]]` and `[[rule]]` at runtime from the file (an
  evaluator does that, built in code).
- **Evidence:** the ledger row
  [the declared fleet checks offline and runs as stem nodes](../operations/what-is-proven.md#declared-fleet-checks-offline-runs-stem).

## Go deeper

- **Guide:** [tutorial 2, code to declarations](../guide/tutorials/02-code-to-declarations.md) and
  [20 § Checking authority before deployment](../guide/20-authorising-actions.md#checking-authority-before-deployment-the-wire-check-overlay).
- **Runbook:** [capability lifecycle](../operations/capability-lifecycle.md) (declare, check, deploy).
- **Design:** [`plans/design-time-tooling.md`](../plans/design-time-tooling.md).
- **Code:** [`src/wire_check.rs`](../../src/wire_check.rs) and [`src/capability_config.rs`](../../src/capability_config.rs);
  the reference is [`unit-file.md`](../reference/unit-file.md).
