# Presence bands: floors and ceilings

## What it is

A [presence band](../guide/00-concepts.md#presence) keeps a capability live across the fleet whether or not anyone is
asking for it: at least `min_providers` (the floor) and, if declared, at most `max_providers` (the ceiling). Each stem
that declares the band in its own unit file supervises it; the band is not published to the fleet, so a host that does
not declare it does not fill it. Below the floor, hosts self-elect to install the cheapest loadable entry that passes provenance and eligibility. Above the
ceiling, the hosts ranked beyond it by the band's rendezvous order withdraw — every host with the same view agrees
which — among the providers that advertise they will shed. The band is a target the fleet converges to, not a count it
guarantees at every moment.

## When to use it, and when not

- **Use it when** a capability must be there before the first request — a router, a safety check, a model the first
  caller should not wait for — and should not grow without bound.
- **Not when** you want providers only while there is demand: that is [demand-driven provisioning](demand-provisioning.md).
  Avoid overlapping bands with tight ceilings over one capability: they can trade a provider back and forth.

## Turn it on

Declare the band in each stem's unit file; a stem refuses `[[presence]]` without `[hosts]`. The catalog demo's
[`examples/units/catalog/late.toml`](../../examples/units/catalog/late.toml), which CI's wire-check reads, declares a
floor.

- `[[presence]]` ([unit-file § presence](../reference/unit-file.md#presence)): the filter inline (`ns`, `name`,
  `schema_id`, `attrs`, `ranking`), `min_providers` (`0` is refused) and an optional `max_providers` (refused below the
  floor).
- In code, [`supervise`](../../mycelium-wasm-host/src/provisioner.rs) and
  [`supervise_band`](../../mycelium-wasm-host/src/provisioner.rs) on the provisioner.
- A stem advertises `prov-shed/{ns}:{name}:{hash}` for each band it supervises with a ceiling; a provider that does not
  (registered in code, or a stem without this band) keeps its place once seen unmarked for two advertise intervals.
- `exactly_the_surplus_above_a_ceiling_withdraws`, in
  [`mycelium-wasm-host/src/provisioner.rs`](../../mycelium-wasm-host/src/provisioner.rs), checks the ceiling's ranking
  alone — the shed view, for every band size and ceiling, withdraws exactly the surplus — not a running band.

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`first_stem_fleet`](../../examples/README.md#the-capability-matrix): three hosts keep a floor of two providers of a
signed echo component and settle under the ceiling they declare; CI runs it. That a floor is filled and re-filled when a
provider goes is `the_stem_fleet_fills_a_presence_floor_and_reheals`, in
[`mycelium-wasm-host/src/stem.rs`](../../mycelium-wasm-host/src/stem.rs).

## What it guarantees, and what it does not

- **Rules:** [`prov.presence_floor`](../reference/rule-catalogue.md#provpresence_floor) — `below_floor` starts an install
  here, `floor_met`, deferred `self_election_declined`, or refused `already_hosted`, `provenance_rejected`, `ineligible`,
  `no_loadable_candidate` or `rights_refused`, rev 3 (earlier revisions chose the cheapest line before checking it, so a
  cheaper line any member gossiped stalled the floor); [`prov.shed`](../reference/rule-catalogue.md#provshed) — `above_ceiling` withdraws the
  hosts ranked beyond it, rev 2 (rev 1 drew per host, and every host could withdraw at once).
- **Not built:** a cardinality guarantee — a count sits outside the band while installs and withdrawals settle, and a
  wrong view of the providers costs at most its own wrong entries; a fleet-wide band — each host fills only the bands
  it declares; placement — which host fills a gap is the self-election's roll; feeding an install's failure back into
  the choice — a trusted line whose install always fails is chosen again every round, and the floor is not filled here.
  Two lines still hold a floor at `already_hosted`: a line signed by a trusted publisher that names an artifact this host
  already hosts for something else, and — on a host with `accept_unsigned = true`, where every line passes provenance —
  any member's line that does the same.
- **Evidence:** the ledger row
  [every artifact-shaped co-op demo reaches the same outcome as stem nodes](../operations/what-is-proven.md#coop-demos-same-outcome-as-stems).

## Go deeper

- **Guide:** [tutorial 1, your first stem fleet](../guide/tutorials/01-first-stem-fleet.md).
- **Runbook:** [capability lifecycle](../operations/capability-lifecycle.md).
- **Design:** [`design/artifact-library.md`](../design/artifact-library.md) and plan D17 in
  [`plans/design-time-tooling.md`](../plans/design-time-tooling.md).
- **Code:** `provision_round` and the supervision policy in
  [`mycelium-wasm-host/src/provisioner.rs`](../../mycelium-wasm-host/src/provisioner.rs).
