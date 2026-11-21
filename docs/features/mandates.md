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
- **Not for authority, a `call:` mandate:** a gateway's cancellation handle for one call it dispatches. No rule can
  require one, and it is verified apart from every other mandate
  ([I4](../plans/identity-and-lifecycle.md#3--rows), row I4, D-1 and D-6).

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
- [`assess_call`](../../src/agent/gateway_authority.rs): verifies a gateway's `call:` mandate
  ([`PresentedCallMandate`](../../src/agent/gateway_authority.rs)) from the checkpoint carried beside it, and holds
  its scope until the mandate's window ends. Work registered under it stops when its term is revoked. This one needs
  a build with `compliance` as well: the mandate is verified against the gateway's member key, which a node holds
  only there, and without it every `call:` mandate reads unknown.
- `an_installed_epoch_survives_a_restart_as_a_floor`, in
  [`src/mandate/authority.rs`](../../src/mandate/authority.rs).
- `a_revocation_written_during_admission_is_read_or_fed_never_missed`, in
  [`src/agent/call_mandate_tests.rs`](../../src/agent/call_mandate_tests.rs), for the `call:` mandate.

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
- **The `call:` mandate's gateway half** ([§110](../guide/deprecations.md)): with
  [`gateway_mint_call_mandates`](../reference/configuration.md#gateway_mint_call_mandates) on, a gateway mints one
  for each protected call it dispatches ([`mint`](../../src/agent/call_issuer.rs)), writes its checkpoint and
  a signed `window` record durably before sending the call, re-asserts its checkpoint when another value lands at
  its key, and collects the `call/` and `grant/` families past D-5's floor (`max_drain_secs`,
  `grant_retention_secs`). Tests: [`src/agent/call_mint_tests.rs`](../../src/agent/call_mint_tests.rs).
- **Cancelling a call** ([§114](../guide/deprecations.md)): A2A `tasks/cancel`, MCP `notifications/cancelled` at
  `/mcp`, and `GossipAgent::cancel_call` ([`call_cancel.rs`](../../src/agent/call_cancel.rs)) revoke a minted call's term,
  **forward the revocation to the provider the call went to at once** (the cancel-forward, `sys.call-cancel.v1`,
  accepted only from the issuing gateway — `cancel_not_from_issuer` otherwise) and record it on disk beside the
  forward; the caller's acknowledgement waits for the disk, the provider's stop does not
  ([§124](../guide/deprecations.md); identity-and-lifecycle §9 O10). A cancel the disk refuses has still been
  forwarded: `cancel_call` answers `NotDurableForwarded` with the provider's class (A2A `-32603`, the task kept — it reads as working in `tasks/get` until `A2A_TASK_TTL`, 300 s), the stop held in memory only. At `/a2a` a send that answers while its cancel is recorded is decided by the provider's answer to the forward: `Stopped` — the stop's, the task `canceled`; `NotRunning` — the call completed, its result kept, the cancel `-32002`; anything else (a lost answer among them) — outcome unknown, the answer kept as the task's result and the cancel `-32603`, never *canceled* or *completed*. `cancel_call` returns a
  `CancelReceipt` whose class is the provider's own answer — `Stopped` only where the agent doing the work promised
  to stop and it ended, `Unbounded` where it made no such promise, `DeliveryUnknown` for no answer (never a
  negative). Tests: [`src/agent/call_cancel_tests.rs`](../../src/agent/call_cancel_tests.rs).
- **Not built, for the `call:` mandate:** a receipt on the A2A or MCP wire (by decision, Q2: the doors answer what
  the gateway did); after a gateway restart, the gateway re-asserts nothing and forwards nothing for the calls it
  minted before (`NotForwarded`: the KV carries the cancel). The authority's half and the provider's admission are
  built ([provider enforcement](provider-enforcement.md))
  ([I4](../plans/identity-and-lifecycle.md#3--rows), row I4, *The build*; [§100](../guide/deprecations.md#100-the-execution-authority-verifies-a-call-mandate-a-call-mandate-in-the-first-slot-is-never-established-the-call-key-family-is-owned-2410-identity-and-lifecycle-i4-pr-1a)).
- **A `call:` checkpoint in the KV is verified at read:** a copy its gateway did not sign, or that names another
  call, is treated as absent and counted in `GET /stats` `call_checkpoints_refused`; an older copy written back and
  a tombstone are counted in `call_checkpoints_stale` and `call_checkpoints_missing`. A member that overwrites the
  key can no longer deny a cancel the gateway forwards (PR 3: the forward does not go through the KV); it still can,
  until the mandate's window ends, where nothing is forwarded — after a gateway restart — unless the minting gateway
  writes its checkpoint back first (PR 2's re-assertion)
  ([ledger](../operations/what-is-proven.md#call-checkpoint-replay))
  ([§100](../guide/deprecations.md#100-the-execution-authority-verifies-a-call-mandate-a-call-mandate-in-the-first-slot-is-never-established-the-call-key-family-is-owned-2410-identity-and-lifecycle-i4-pr-1a)).
- **Evidence:** the ledger rows [running work stops](../operations/what-is-proven.md#running-work-stops) and
  [after a revocation every door runs nothing](../operations/what-is-proven.md#revocation-every-door-runs-nothing);
  for the `call:` mandate, [the soak](../operations/what-is-proven.md#call-soak) (10⁵ cancellable calls over two
  nodes, half cancelled while running, run once over a gateway with no `[persistence]`; 10³ on every merge,
  [`src/agent/call_soak_tests.rs`](../../src/agent/call_soak_tests.rs)), [its costs](../operations/what-is-proven.md#call-mandate-carried-checkpoint-cost)
  and [the memory a held window takes](../operations/what-is-proven.md#call-window-memory).
- **What a cancel's forward-first order costs — the crash bound** (§9 O10, stated and tested): the forwarded
  revocation is in no KV until its sync returns, so a gateway that crashes between the forward and the sync answers
  its caller **nothing** — no acknowledgement, not a "not durable" one — and has no record of the revocation after its
  restart; the provider holds it in memory only (D-4), and after a provider restart the call, presented again inside
  its window, is admitted. Such a cancel lasts as long as the provider process that holds it, at most to the call's
  window. A cancel asked again writes it durably and the KV then stops the call
  (`a_gateway_crash_between_the_forward_and_the_sync_leaves_no_record_and_a_restarted_provider_admits_the_call_again`)
  ([ledger](../operations/what-is-proven.md#call-cancel-behind-collector)).
- **Still open, for the `call:` mandate** (§9 O11): the revocation itself waits its turn at the gateway's WAL — one
  sync per synced append, in arrival order — so the caller's acknowledgement, a mint, and (under `sync_mode =
  "flush"`, where the gateway's connection reader waits on the WAL for each inbound record) the provider's answer to
  the forward wait behind any burst; the receipt is then `DeliveryUnknown`, never a negative
  ([ledger](../operations/what-is-proven.md#call-cancel-behind-collector)).

## Go deeper

- **Guide:** [21, mandates and resource fences](../guide/21-mandates.md), including
  [§ Eligibility](../guide/21-mandates.md#eligibility-unknown-history-is-never-eligible-2240).
- **Runbook:** [audit § 9, Mandate lifecycle](../operations/audit.md#9-mandate-lifecycle--three-endings-recorded-apart).
- **Design:** [`scoped-mandates.md`](../design/scoped-mandates.md) and
  [`authority-at-execution.md`](../design/authority-at-execution.md).
- **Code:** [`src/mandate.rs`](../../src/mandate.rs) and [`src/mandate/`](../../src/mandate/authority.rs),
  [`src/agent/gateway_authority.rs`](../../src/agent/gateway_authority.rs).
