# Doctor

## What it is

`mycelium doctor` is the [readiness report](../guide/00-concepts.md#doctor) without a node: what `start()` would print
and whether it would refuse, from a configuration file and the environment alone. It runs the node's own preflight and
profile check and takes the same [guarantee report](../guide/00-concepts.md#guarantee-report) over a node that is built and
never started, so it binds no port, creates no directory and attaches nothing. It also prints the effective
configuration — every field as loaded, after defaults, file and environment, with its source named; a tuning
field at its auto value prints that value, the node derives the running one — and shows a secret as `set` or
`unset`, never its value. `--format json` is one document on stdout, schema `mycelium.doctor/1`, with the logs
on stderr.

## When to use it, and when not

- **Use it when** you are about to start a node from a configuration and want the refusal, if any, before the node
  runs, with the profile's unmet guarantees named in `start()`'s words; or when you want to see which of a field's
  values won, and where it came from.
- **Not when** the node attaches an evaluator, a journal or an audit sink in code: `doctor` sees what the `mycelium`
  binary sees, which attaches none, so an embedder reads its own `guarantee_report()` after attaching. Unreadable
  persisted state is found only by opening the directory, which `doctor` does not do; that refusal stays with
  `start()`.

## Turn it on

This feature is turned on in code.

- [`run`](../../src/doctor.rs) — loads the three layers (defaults, the file, the environment), validates, builds the
  node without starting it, and runs the same functions `start()` runs.
- [`preflight`](../../src/agent/lifecycle.rs) and [`profile_check`](../../src/agent/lifecycle.rs) — `start()`'s own
  refusal chain and profile check, extracted so `doctor` calls them rather than copies them.
- [`effective_fields`](../../src/doctor.rs) — the per-field source, by comparing the three layers.
- [`persistence`](../reference/configuration.md#the-fields) — the one field `doctor` inspects on disk, read-only.
- `doctor_names_the_refusal_start_would_give`, in [`src/doctor.rs`](../../src/doctor.rs): runs `doctor` and
  `start()` over the same configuration under `secure-single-domain` and compares the words.

## See it work

No runnable example exists: the [example index's capability matrix](../../examples/README.md#the-capability-matrix)
has no row for this feature. The end-to-end runs are tests in [`src/doctor.rs`](../../src/doctor.rs):
`doctor_names_the_refusal_start_would_give` (the same words as `start()`),
`a_table_this_build_cannot_enforce_is_a_refusal_naming_the_feature` (a token table in a build without `compliance`),
`a_missing_persistence_directory_is_reported_and_not_created`, `the_effective_configuration_names_each_fields_source`,
`a_secret_is_shown_as_set_and_never_its_value`, `a_credential_bindings_source_names_are_redacted` and
`every_top_level_field_is_in_the_configuration_reference_table`. The command itself is documented in
[diagnostics § Before you start](../operations/diagnostics.md#before-you-start-mycelium-doctor).

## What it guarantees, and what it does not

- `doctor` adds no guarantee of its own; it reports the ones in the [guarantee catalogue](../reference/guarantee-catalogue.md)
  as the node would resolve them, with `started` reading false.
- It says what this build and this configuration would do; it cannot say `enforced` for a guarantee the catalogue
  marks `not_verifiable_here`, and it cannot see an attachment made in code. The persistence directory's
  `read-only` is its mode bits, not an access check.
- **Changed with it:** `GossipConfig::validate()` no longer creates `persistence.base_path` as its early check; it
  reads the nearest existing ancestor instead, and `start()` creates the node's directory as before
  ([deprecations § 57](../guide/deprecations.md#57-validate-no-longer-creates-the-persistence-base-path-2370-adopter-surface-x1)).
- The plan row is [`adopter-surface.md`](../plans/adopter-surface.md) X1.

## Go deeper

- Runbook: [diagnostics § Before you start](../operations/diagnostics.md#before-you-start-mycelium-doctor) and the
  [production-readiness checklist](../operations/production-readiness.md), whose node-level items `doctor` answers.
- Reference: [the configuration reference](../reference/configuration.md) (the fields and their sources) and the
  [guarantee catalogue](../reference/guarantee-catalogue.md).
- No guide chapter and no design record; the plan row is the record.
- Code: [`src/doctor.rs`](../../src/doctor.rs), [`src/agent/lifecycle.rs`](../../src/agent/lifecycle.rs),
  [`src/main.rs`](../../src/main.rs).
