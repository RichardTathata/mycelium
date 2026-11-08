# Intents

## What it is

An [intent](../guide/00-concepts.md#intent) is desired state published as soft state, not a command: a membership band
for a group, a tuning value for a parameter, a timing set. It is written under `sys/govern/`, stamped when written, and
gossiped like any key. Each node's governor reads it, decides locally how to reconcile, and applies it to itself; an
intent can name one node, and then only that node applies it. An intent evaporates: a governor stops acting on one
whose writer has not refreshed it within its window, so management that vanishes leaves the fleet holding its last
intent for a while and then its own defaults, never a stuck command. A node's local pin wins over a fleet intent.

## When to use it, and when not

- **Use it when** an operator or an agent wants to steer the fleet — size a pool, retune a parameter — without a
  control plane that the fleet would depend on.
- **Not when** the change must take effect on every node at once or be acknowledged: an intent is reconciled per node,
  as each one sees it and as its governor's spacing allows.

## Turn it on

This feature is turned on in code; the governor that reads each intent must run on the nodes that should apply it.

- [`publish_membership_intent`](../../src/agent/membership_governor.rs): a group's band
  ([governed membership](governed-membership.md)).
- [`publish_tuning_intent`](../../src/agent/tuning_governor.rs): a value for a parameter a node has not pinned.
- [`govern_timing`](../../src/agent/introspect.rs)`(health_check_interval_secs, reconnect_backoff_secs, target)`, which
  publishes a [`TimingIntent`](../../src/agent/timing_governor.rs): a node's timing set in one pass; it refuses an
  intent that would govern nothing.
- Over HTTP, `POST /gateway/govern/membership`, `/govern/tuning` and `/govern/timing`, under `govern:write` — a scope
  checked only in a build with `compliance` (the published node has it); without it any valid bearer writes an intent.
- [`FleetIntent`](../../src/agent/intent.rs): the shared transport — publish, freshness and evaporation, node
  targeting — that each governor's own policy sits on.
- `test_governor_node_targeted_intent`, in [`src/lib_tests.rs`](../../src/lib_tests.rs): a node-targeted intent is
  applied by the named node and not by the publisher's.

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`elastic_intent`](../../examples/README.md#the-capability-matrix): an operator publishes a band for `rush-pool`, goes
offline, and the band still holds. CI runs it. [`diagnostics`](../../examples/README.md#the-capability-matrix) shows a
node naming an intent-versus-reality mismatch from its own view.

## What it guarantees, and what it does not

- No guarantee or rule id of its own: what an intent leads to is its governor's decision, such as
  [`membership.governed`](../reference/rule-catalogue.md#membershipgoverned); under which control profile a governor
  enforces is [adaptive stability](adaptive-stability.md)'s.
- **Not built:** an intent is not authenticated beyond the KV write that carried it, and the governance routes are
  where authority is checked (`govern:write`, audited under `compliance`); nodes can disagree while it gossips; an
  intent whose writer disappears stops binding after its window, by design.
- **Evidence:** the [what-is-proven](../operations/what-is-proven.md) ledger has no row for intents on their own.

## Go deeper

- **Guide:** [22, stability and control](../guide/22-stability-and-control.md);
  [the management-as-intent wiki page](../wiki/domain/theory/management-as-intent.md).
- **Runbook:** [control profiles](../operations/control-profiles.md) and [diagnostics](../operations/diagnostics.md).
- **Design:** [`plans/elastic-sizing-intent-governed.md`](../plans/elastic-sizing-intent-governed.md).
- **Code:** [`src/agent/intent.rs`](../../src/agent/intent.rs) and the three governors beside it.
