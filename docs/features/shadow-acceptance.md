# Shadow acceptance: proposed, shadow, accept

## What it is

An artifact can be published as a proposal. A host loads a proposed entry only into the
[shadow lane](../guide/00-concepts.md#shadow) — as `{ns}/{name}.shadow`, callable by name and never resolved by a
requirer's filter, so it takes no demand — until a reviewer whose key that host lists co-signs it. Once accepted, the
host withdraws its shadow install, and the entry becomes eligible to load under the real name — which it does only
where demand for that name is unmet. Acceptance does not replace a live provider: while the
[incumbent](../guide/00-concepts.md#incumbent), the version already live under the real name, serves it, the demand
pass records `provider_present` and the floor `floor_met`, and the incumbent keeps serving. This is
how an agent-authored component, or a new version, runs beside real traffic before anything depends on it.

## When to use it, and when not

- **Use it when** a new component or version should be callable and observable on real hosts before a reviewer lets it
  load under the live name — especially one an agent wrote.
- **Not when** you need traffic split or compared automatically: the shadow lane is callable by name, and sending it
  work and judging the result is yours; and not when acceptance should roll a live v1 over to v2 — retiring the
  incumbent is a separate step.

## Turn it on

The publisher marks the description `proposed = true`; each host lists its reviewers.

- `[hosts].trusted_reviewers` ([unit-file § hosts](../reference/unit-file.md#hosts)): reviewer keys whose acceptance
  promotes a proposal; empty means a proposal never loads for real on that host. In code,
  [`require_reviewers`](../../mycelium-wasm-host/src/provisioner.rs).
- `mycelium-artifact accept <library> <ns/name> --key <seed-file>` (or `--key-env <VAR>`) co-signs a proposal, and
  `mycelium-artifact verify --reviewer` names a forged or unlisted acceptance, in
  [`mycelium-wasm-host/src/bin/mycelium-artifact.rs`](../../mycelium-wasm-host/src/bin/mycelium-artifact.rs).
- [`is_loadable`](../../mycelium-wasm-host/src/catalog.rs) and [`shadow_name`](../../mycelium-wasm-host/src/provisioner.rs)
  for code that decides or calls the lane itself.
- `a_proposed_entry_loads_only_into_the_shadow_lane_until_a_listed_reviewer_accepts_it`, in
  [`mycelium-wasm-host/src/provisioner.rs`](../../mycelium-wasm-host/src/provisioner.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`provisioning`](../../examples/README.md#the-capability-matrix), whose final wave publishes a proposed v2 beside a live
v1: the proposal runs only in the shadow lane while the incumbent serves the live name, until a reviewer accepts it.
CI runs it.

## What it guarantees, and what it does not

- **Rules:** [`prov.loadable`](../reference/rule-catalogue.md#provloadable) — `accepted` or `not_a_proposal`, refused
  `proposed_unaccepted`; [`prov.promotion`](../reference/rule-catalogue.md#provpromotion) — the shadow install of an entry
  accepted, or no longer in the catalogue, withdrawn (`accepted_withdraw_shadow`), or `still_proposed`.
- **Not built:** acceptance is per host — a host with a different reviewer list decides differently; reviewers are a
  separate key set from publishers, so the review is only as strong as who holds those keys; an accepted entry's fuel
  is still its signer's ([guide 16 § Agent-authored functions](../guide/16-guardrails.md#agent-authored-functions-the-legitimate-uses-of-dynamic-load)).
- **Evidence:** the ledger row
  [a looping entry is stopped at its fuel budget, and a proposed entry loads only into the shadow lane](../operations/what-is-proven.md#looping-entry-stopped-at-fuel-budget).

## Go deeper

- **Guide:** [16 § Agent-authored functions](../guide/16-guardrails.md#agent-authored-functions-the-legitimate-uses-of-dynamic-load).
- **Runbook:** [artifacts § Trust and provenance](../operations/artifacts.md#trust--provenance).
- **Design:** decision D20 in [`plans/design-time-tooling.md`](../plans/design-time-tooling.md).
- **Code:** `is_loadable` in [`mycelium-wasm-host/src/catalog.rs`](../../mycelium-wasm-host/src/catalog.rs) and the
  promotion in `provision_round`, [`mycelium-wasm-host/src/provisioner.rs`](../../mycelium-wasm-host/src/provisioner.rs).
