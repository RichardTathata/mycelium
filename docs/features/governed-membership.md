# Groups and governed membership

## What it is

A [group](../guide/00-concepts.md#group) is a named set of nodes that join by a capability filter, declared in code or
as a unit file's `[[group]]`. A group can be governed: an operator publishes an evaporating membership intent — keep
between `min` and `max` members, drain these nodes — and each candidate node runs a membership governor that decides
for itself whether to join or leave, spacing and settling its moves, so the group holds the band without a controller
choosing who. The intent lives in gossip for a window; if the operator goes away the band still holds until it
evaporates. Over HTTP, a governed group's membership moves only through `/gateway/govern/group`: the plain
`/gateway/mesh/group` routes refuse it (403 `governed_group`). In code, `mesh().join_group` and `leave_group` do not
check, and the governor never rolls an electorate group.

## When to use it, and when not

- **Use it when** a pool should hold a size — enough workers for a rush, no more — and members may fail, with nobody
  assigning them.
- **Not when** you need membership fixed by identity, as a consensus electorate is: a governed group is elected from
  whoever matches the filter ([consensus and electorates](consensus-electorates.md)).

## Turn it on

This feature is turned on in code. The co-op operator's unit file
[`examples/units/elastic_intent/coop-operator.toml`](../../examples/units/elastic_intent/coop-operator.toml), which
CI's wire-check reads, declares the group.

- `[[group]]` ([unit-file § group](../reference/unit-file.md#group)): `name`, `filter`, and optionally `topology_policy`, `provides` and
  `requires`; in code, [`define_capability_group`](../../src/agent/capability_handle.rs).
- [`publish_membership_intent`](../../src/agent/membership_governor.rs) with a
  [`MembershipIntent`](../../src/agent/membership_governor.rs): the band and the drain list; refresh it within
  `MEMBERSHIP_INTENT_TTL_MS` (five minutes) or it evaporates.
- [`start_membership_governor`](../../src/agent/membership_governor.rs): on every candidate node.
- [`set_control_profile`](../../src/agent/control_profile.rs): under `EnforceLocal` or above, a move waits for a
  confident view (enough peers heard, fresh enough); under `Legacy`, the default, the confidence bound is not
  consulted ([adaptive stability](adaptive-stability.md)).
- `test_membership_governor_converges_to_min`, in [`src/lib_tests.rs`](../../src/lib_tests.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`elastic_intent`](../../examples/README.md#the-capability-matrix): an operator keeps `rush-pool` in `[2, 3]` depots,
goes offline, and a member is killed; the governors self-elect and heal the count. CI runs it.

## What it guarantees, and what it does not

- **Rule:** [`membership.governed`](../reference/rule-catalogue.md#membershipgoverned) — `join` or `leave`; deferred as
  `view_not_confident` (under an enforcing control profile), `settling` or `spacing`; otherwise `hold`.
- **Held at the gateway:** a governed group's filter cannot be redefined through `units/declare`, nor its membership
  changed through `/gateway/mesh/group` (both 403 `governed_group`).
- **Not built:** the band is a target the governors reach, not an invariant at every instant — a count can sit
  outside it while moves settle; a group is self-joined by filter, so a node that matches and misbehaves is not kept
  out; in-process `join_group` and `leave_group`, and in-process or gossiped `grp/` writes (the HTTP KV routes refuse `grp/`, 403 `protected_key`), are not checked against governance; the
  governor's eligibility reads `cap/` without freshness, so a dead node can stay eligible — a known gap fixed by a
  separate PR.
- **Evidence:** the [what-is-proven](../operations/what-is-proven.md) ledger's
  [hysteresis row](../operations/what-is-proven.md#hysteresis-control-loops-load-bearing), within scenario C's bounded
  sense.

## Go deeper

- **Guide:** [02, capabilities](../guide/02-capabilities.md) for groups;
  [22, stability and control](../guide/22-stability-and-control.md) for the governor.
- **Runbook:** [control profiles](../operations/control-profiles.md);
  [deployment § Rolling upgrades](../operations/deployment.md#rolling-upgrades) on `/gateway/govern/group`.
- **Design:** [`adaptive-stability.md`](../design/adaptive-stability.md).
- **Code:** [`src/agent/membership_governor.rs`](../../src/agent/membership_governor.rs).
