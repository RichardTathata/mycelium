# Fault classes against a declaration

## What it is

`mycelium test --faults [<class>,…] <units-dir> [--config <node.toml>]` builds a fleet of in-process nodes from a
project's [unit files](../guide/00-concepts.md#unit-file) and a configuration, and runs the
[fault classes](../guide/00-concepts.md#test-faults) the repository already proves against *that* fleet: `revoke`,
`restart`, `partition`, `replay` and `policy-diff`. The report says, per class, which checks held, which guarantee and
rule ids it exercised with each one's resolution on the fleet's node, and which it could not exercise and why — in the
same vocabulary as the [readiness report](../guide/00-concepts.md#doctor): `not_configured`, `not_in_build`,
`not_applicable`, `not_verifiable_here`. A finding is the fleet's: a revoked key still verifying at one door, a synced
write gone after a restart, a dark member still counted.

## When to use it, and when not

- **Use it when** a declaration and a configuration are about to carry real work and you want to know what this
  fleet does under the faults the substrate claims to handle — before it does, with the ids named.
- **Not when** the question is a two-sided partition, load, or a real provider: the fleet is in-process and loopback
  (D6), a cut is a node gone dark, and nothing here leaves the machine. An attachment made in code — an evaluator, a
  journal, an authority — is not in its view, as it is not in `doctor`'s.

## Turn it on

This feature is turned on in code.

- [`run`](../../src/faults.rs) — `wire-check` over the declaration first (an error refuses the run with the same
  findings), then one fleet per class, built from the configuration with loopback addresses, free ports, persistence
  and TLS material under a temporary directory, no gateway port, a one-second health check and governor cooldown.
- [`Class`](../../src/faults.rs) — the five classes; `--faults` alone runs every one, a list runs those and fails the
  run if one cannot run here.
- `replay` needs the binary built with the `sim` Cargo feature (`cargo build --bin mycelium --features cli,sim`); on
  any other it reads `not_in_build` naming `sim`. `revoke` needs `tls` and `compliance` in the build and a `[tls]`
  table; `restart` needs [`persistence`](../reference/configuration.md#the-fields); `policy-diff` needs `gateway` and
  `tls` and `--against <units-dir>`, the proposed revision.
- `a_declaration_wire_check_refuses_is_refused_with_the_same_findings`, in [`src/faults.rs`](../../src/faults.rs):
  the plant is a unit nobody can wire; `test` refuses with `wire-check`'s own report, word for word.

## See it work

No runnable example exists: the [example index's capability matrix](../../examples/README.md#the-capability-matrix)
has no row for this feature. The CI job `Scaffold` runs it against the Rust scaffold's declaration with every class
the scaffold's build has (`revoke`, `restart`, `partition` pass; `replay` reads not-in-build; a planted widened
`[[rule]]` is reported by `policy-diff`; a declaration `wire-check` refuses is refused with the same text), and the
job `embedded` builds the `sim` binary and runs `replay` against the co-op fixture. The tests are in
[`src/faults.rs`](../../src/faults.rs): `a_declaration_wire_check_refuses_is_refused_with_the_same_findings`,
`an_activation_runs_no_command`, `a_class_that_cannot_run_fails_only_when_asked_by_name`,
`the_restart_class_replays_a_synced_write_and_refuses_unreadable_state`,
`the_revoke_class_sees_the_three_verifiers_and_the_mandate_refuse`,
`the_partition_class_sees_a_dark_member_evicted_and_the_floor_refilled` and
`a_planted_policy_regression_is_reported_by_policy_diff`; the `sim` half is
`the_replay_class_replays_the_fleet_decision_for_decision` in [`tests/faults_replay.rs`](../../tests/faults_replay.rs).

## What it guarantees, and what it does not

- It adds no guarantee of its own; it exercises the ones in the guarantee catalogue —
  [`persist.unreadable_refused`](../reference/guarantee-catalogue.md) and
  [`persist.configured`](../reference/guarantee-catalogue.md) (`restart`), [`mesh.tls`](../reference/guarantee-catalogue.md)
  and [`mesh.signed_writes`](../reference/guarantee-catalogue.md) (`revoke`),
  [`id.revocation_authority`](../reference/guarantee-catalogue.md) (`revoke`; it reads `not_configured` under the
  development posture, the CA key being on the node) — and the decision points in the rule catalogue:
  [`membership.governed`](../reference/rule-catalogue.md) (`partition`, `replay`), [`kv.expiry`](../reference/rule-catalogue.md)
  and [`cap.match`](../reference/rule-catalogue.md) (`partition`), [`ae.preflight`](../reference/rule-catalogue.md)
  (`policy-diff`). Each is reported with its resolution on the fleet's node; an id that does not resolve `enforced`
  is listed as not exercised, with the reason the node gives.
- **It never runs an activation.** A unit's `[[activation]]` commands are counted and reported `not_applicable`;
  none is started, confined or not (D23).
- The fleet is the configuration's with stated rewrites, not the deployment: loopback, free ports, temporary
  directories, no gateway, every node bootstrapping to every other, a one-second health check. A `[[group]]` is
  governed to a band of one (floor one, ceiling one), so a cut member is a deficit a survivor must fill. A cut is the node's runtime dropped: no departure is sent, its sockets close, the dark
  side is not observed.
- `policy-diff` decides the declaration's call set — every `[[rule]]` as written and without the values it
  requires, every wired edge's two call shapes — at this command's own enforcement point under evaluators built from
  each revision's `[[rule]]`s, records each decision, and re-decides the recorded calls under the proposed revision.
  A record it cannot join to a call with its inputs is reported *unattributable*, never as unchanged. A rule's
  resource matches the call's as written, without a provider suffix.
- `replay` is the `sim` build's: each unit's node is recorded and replayed **alone** on a current-thread runtime
  under one kernel, as the node binary records; a divergence is reported where the seam raised it. The schedule
  between nodes — frame arrival — is outside the seams (the inventory's §3.1.1), so a fleet recorded together
  diverges at its first cross-node channel choice; stated, not hidden.
- **Found by it, and fixed:** a governed member that went dark held the floor — its group entry is written once
  and withdrawn only by a graceful shutdown, so no survivor rolled to re-fill; the governor now counts a member only
  if it is this node or in its peer table ([`membership.governed`](../reference/rule-catalogue.md) revision 2). The
  stale entry stays in the KV until the node returns, so a fleet snapshot's group size still counts it. **Found and
  stated:** a dark node's offers leave the view only after 3 × their `ttl_secs` — the class reports the time.
- **Not built, stated:** a two-sided partition (the Docker suites', D6); a load class (V1 on the
  [proof page](../operations/what-is-proven.md)); the verifiers an in-process fleet cannot reach from outside the
  node (consensus, caller envelopes, knowledge records — the suite's); a provider-side run of the call set. The plan
  row is [`adopter-surface.md`](../plans/adopter-surface.md) X6.

## Go deeper

- Runbook: [diagnostics § Before the fleet carries anything](../operations/diagnostics.md#before-the-fleet-carries-anything-mycelium-test---faults).
- Reference: [the unit file](../reference/unit-file.md), [the configuration reference](../reference/configuration.md),
  the [guarantee catalogue](../reference/guarantee-catalogue.md) and the [rule catalogue](../reference/rule-catalogue.md).
- Guide: [replay and simulation](../guide/19-replay-and-simulation.md) for what `replay` records and what a divergence
  means.
- No design record; the plan row is the record.
- Code: [`src/faults.rs`](../../src/faults.rs), [`src/agent/fault_probes.rs`](../../src/agent/fault_probes.rs),
  [`src/main.rs`](../../src/main.rs).
