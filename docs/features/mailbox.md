# The mailbox

## What it is

A [mailbox](../guide/00-concepts.md#mailbox) delivers events to one node in order, through the gossip KV. Any node that
knows a target's id writes an event under `mailbox/{target}/{kind}/{hlc}`; it spreads like any other key, so a target
that was down receives its pending events when it reconnects. The target's watcher hands events over in HLC order and
then deletes each one, gossiping the tombstone so it is not delivered again after a restart. Where the delete happens
differs by consumer: the in-process mailbox deletes an event when it enters the receiver's channel, the gateway's
stream only once it has handed the event to the HTTP connection.

## When to use it, and when not

- **Use it when** a request or an event addressed to one node must not be lost because that node was briefly away,
  and must arrive in causal order.
- **Not when** many consumers should share work: that is the [tuple space](../guide/07-pipelines.md); and not when
  missing an event is fine, where a [signal](signal-mesh.md) is cheaper.

## Turn it on

This feature is turned on in code: every node has it, through [`service`](../../src/agent/mod.rs).

- [`deliver_event`](../../src/agent/service_handle.rs): write an event for a target and a kind; over HTTP,
  `POST /gateway/mailbox/deliver`.
- [`open_mailbox`](../../src/agent/service_handle.rs): receive this node's events of a kind, in HLC order; dropping the
  handle stops the watcher. Over HTTP, `GET /gateway/mailbox/{kind}` streams them.
- `mailbox_hlc_ordering`, in [`src/agent/mailbox.rs`](../../src/agent/mailbox.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`mailbox_llm`](../../examples/README.md#the-capability-matrix): a request delivered to a provider as a mailbox event and
answered, run in CI. That a dropped gateway stream loses only what it had handed on is
`a_dropped_mailbox_stream_loses_only_what_it_handed_on`, in
[`src/agent/http/remainder_tests.rs`](../../src/agent/http/remainder_tests.rs).

## What it guarantees, and what it does not

- No guarantee or rule id: the mailbox registers nothing in the
  [guarantee catalogue](../reference/guarantee-catalogue.md) or the rule catalogue.
- **Not built:** exactly-once delivery — an in-process receiver dropped with events queued loses them, because the
  event was deleted when it entered the channel; events carry no expiry of their own, so an event for a node that never returns stays in every replica until it is
  deleted; delivery is to a node, not to an identity that moves.
- **Evidence:** the [what-is-proven](../operations/what-is-proven.md) ledger has no row for the mailbox; the row E
  fixes are in [post-360 hardening](../plans/post-360-hardening.md).

## Go deeper

- **Guide:** [03, signals and boundaries](../guide/03-signals.md), whose example delivers through a mailbox.
- **Runbook:** no runbook of its own; the gateway routes and their scopes (`mesh:read`, `mesh:write`, and a companion's
  family scope for its kinds) are in [rbac § 2](../operations/rbac.md#2-scope-vocabulary-gateway-acls).
- **Design:** no design record; row E of the [post-360 plan](../plans/post-360-hardening.md) is the record of the
  gateway stream's delete-after-hand-off.
- **Code:** [`src/agent/mailbox.rs`](../../src/agent/mailbox.rs) and the mailbox routes in
  [`src/agent/http/mesh.rs`](../../src/agent/http/mesh.rs).
