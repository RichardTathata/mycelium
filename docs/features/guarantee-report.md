# The guarantee report and profiles

## What it is

The [guarantee report](../guide/00-concepts.md#guarantee-report) is what a node says it enforces, and why not the
rest. Every core guarantee, and every one a companion registers, resolves against the node as built and configured to
one of five states: `enforced`, `not_configured` (naming the setting), `not_in_build` (naming the feature),
`not_applicable` (naming the role fact) or `not_verifiable_here` (an external prerequisite, listed and never counted).
The node logs the report once at `start()` and serves it on demand. A profile names a set of guarantees that must be
enforced: `dev` requires nothing and says so, and under `secure-single-domain` or `secure-multi-domain` the start is
refused, naming each unmet guarantee.

## When to use it, and when not

- **Use it when** you need to know, or show, what a node actually enforces before it serves traffic — and to make a
  missing setting stop the start rather than run without it.
- **Not when** you want a security certification: a profile validates its stated conditions on one node, and what a
  node cannot see (network confinement, clock agreement) stays yours to evidence.

## Turn it on

The report is always on. A profile is a setting; the two-domain scaffold's
[`kitchen-gw.toml`](../../templates/two-domains/nodes/kitchen-gw.toml), which CI's two-domains job starts, sets
`secure-multi-domain` ([the multi-domain profile](multi-domain-profile.md)).

- [`profile`](../reference/configuration.md#profile): `dev` (unset), `secure-single-domain` or `secure-multi-domain`,
  or `GOSSIP_PROFILE`; an unknown name is refused by `validate()`, and an unmet required guarantee refuses `start()`.
- [`guarantee_report`](../../src/agent/mod.rs) in code, and `GET /gateway/guarantees` under `fleet:read` — a scope
  checked only in a build with `compliance` (the published node has it; without it any valid bearer reads the report).
- [`register_guarantee`](../../src/agent/mod.rs): a companion's own guarantee, before `start()`; a duplicate, a core id
  or a late registration is refused by name.
- `mycelium doctor` prints the same report without starting a node ([doctor](doctor.md)).
- `a_node_providing_every_requirement_starts_under_the_secure_profile`, in [`src/lib_tests.rs`](../../src/lib_tests.rs):
  under `secure-single-domain` (revision 5 today), the node starts, its report says so, and its gateway refuses an
  unauthenticated request.

## See it work

No runnable example exists: the [example index](../../examples/README.md#the-capability-matrix) has no row for the
report; every node logs it at `start()`. The refusal is a test in
[`src/agent/guarantee.rs`](../../src/agent/guarantee.rs): `the_secure_profile_refuses_an_open_node_by_name` — under
`secure-single-domain` an open node refuses to start, naming each unmet guarantee and what is missing.

## What it guarantees, and what it does not

- No guarantee id of its own: it reports the [guarantee catalogue](../reference/guarantee-catalogue.md), whose state
  matrix shows each guarantee under reference configurations.
- **Not built:** `not_applicable` is a role fact, never a waiver; the strongest sentence the report makes is *node
  requirements satisfied*; the report is not gossiped, so another node does not read it; a guarantee a companion
  registers resolves by the companion's own closure. The snippet in
  [guide 20 § The guarantee report](../guide/20-authorising-actions.md#the-guarantee-report--what-this-node-enforces-and-why-not-the-rest)
  is not run by a test.
- **Evidence:** the ledger row
  [a node says which guarantees it enforces, and why not the rest](../operations/what-is-proven.md#node-says-which-guarantees-enforces-why).

## Go deeper

- **Guide:** [20 § The guarantee report](../guide/20-authorising-actions.md#the-guarantee-report--what-this-node-enforces-and-why-not-the-rest);
  the profiles' meanings in the [concepts glossary](../guide/00-concepts.md#profile-meanings).
- **Runbook:** [production readiness](../operations/production-readiness.md) and [diagnostics § Before you start](../operations/diagnostics.md#before-you-start-mycelium-doctor).
- **Design:** [`plans/guarantees-and-rule-catalogue.md`](../plans/guarantees-and-rule-catalogue.md).
- **Code:** [`src/agent/guarantee.rs`](../../src/agent/guarantee.rs) (the descriptors, the resolutions, the profiles,
  `check`).
