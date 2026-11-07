# Mandates and authority drain

## What it is

A [mandate](../guide/00-concepts.md#mandate) is one appointment: enumerated operations, no wildcard, under a term and
an epoch. The protected resource checks it inside its own atomic boundary, and once it has installed a newer epoch
nothing authorised only under the old one commits there, whether its holder retries, reconnects or restarts. A
stale mandate is refused as superseded, never as a conflict a retry loop would consume. Work already running keeps
its authority only by its [continuation](../guide/00-concepts.md#continuation) — re-checked on an interval, or run
to a declared limit — and stops when that lapses. A successor's eligibility is decided from verified history, and
unknown history is never eligible ([strict eligibility](../guide/00-concepts.md#strict-eligibility)).

## When to use it, and when not

- **Use it when** one holder at a time may act on a resource (a curator, a scheduler, an operator role), and a
  revocation must stop that holder's writes and running work, not only new calls.
- **Not when** you need an approval for one call: that is a [grant](grants.md), a mandate for one call.

## Turn it on

This feature is turned on in code, in a build with `gateway` and `tls`.

- [`ExecutionAuthority`](../../src/agent/gateway_authority.rs): verifies a presented mandate; attach it with
  [`with_execution_authority`](../../src/agent/mod.rs), and a call that presents a mandate gets a finding —
  established, refused or unknown — in its action envelope.
- [`with_durable_epochs`](../../src/agent/gateway_authority.rs), over
  [`DurableEpochs::open`](../../src/mandate/authority.rs): installed epochs journalled first, so a restart does not
  restore revoked authority.
- [`ResourceAuthority::check`](../../src/mandate.rs): the check a protected resource runs inside its own boundary.
- [`Continuation`](../../src/mandate/authority.rs): how admitted long-running work continues; a declared limit the
  resource does not enforce is not a bound.
- [`eligible_strict`](../../src/mandate/eligibility.rs): a successor's verdict per incumbency rule, from a verified
  term history.
- [`with_provider_enforcement`](../../src/agent/mod.rs): so the mandate is checked at the provider as well
  ([provider enforcement](provider-enforcement.md)).
- `an_installed_epoch_survives_a_restart_as_a_floor`, in
  [`src/mandate/authority.rs`](../../src/mandate/authority.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`curator_handover`](../../examples/README.md#the-capability-matrix): a wiki curator's appointment changes while a
write is in flight, and the single transaction that checks the appointment and moves the content has one outcome.
CI runs it. [`strict_eligibility`](../../examples/README.md#the-capability-matrix) shows a successor refused on
unknown history; [`authority_drain`](../../examples/README.md#the-capability-matrix) measures the stop of running
work.

## What it guarantees, and what it does not

- **Guarantees:** [`authz.execution_authority`](../reference/guarantee-catalogue.md#authz.execution_authority) —
  mandates established and verified at this node; [`authz.durable_epochs`](../reference/guarantee-catalogue.md#authz.durable_epochs)
  — a restart does not restore revoked authority.
- **Not built:** [guide 21 § What this does not establish](../guide/21-mandates.md#what-this-does-not-establish).
  The check-then-act window is narrowed and stated, not eliminated, and the wiki store counts its late writes
  rather than promising none ([what-is-proven § the check-then-act window](../operations/what-is-proven.md#check-then-act-window)).
- **Evidence:** the ledger rows [running work stops](../operations/what-is-proven.md#running-work-stops) and
  [after a revocation every door runs nothing](../operations/what-is-proven.md#revocation-every-door-runs-nothing).

## Go deeper

- **Guide:** [21, mandates and resource fences](../guide/21-mandates.md), including
  [§ Eligibility](../guide/21-mandates.md#eligibility-unknown-history-is-never-eligible-2240).
- **Runbook:** [audit § 9, Mandate lifecycle](../operations/audit.md#9-mandate-lifecycle--three-endings-recorded-apart).
- **Design:** [`scoped-mandates.md`](../design/scoped-mandates.md) and
  [`authority-at-execution.md`](../design/authority-at-execution.md).
- **Code:** [`src/mandate.rs`](../../src/mandate.rs) and [`src/mandate/`](../../src/mandate/authority.rs),
  [`src/agent/gateway_authority.rs`](../../src/agent/gateway_authority.rs).
