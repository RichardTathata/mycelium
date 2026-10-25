# Fault classes against a declaration

## What it is

`mycelium test --faults [<class>,…] <units-dir> [--config <node.toml>]` builds a fleet of in-process nodes from a
project's [unit files](../guide/00-concepts.md#unit-file) and a configuration, and runs the
[fault classes](../guide/00-concepts.md#test-faults) the repository already proves against *that* fleet: `revoke`,
`restart`, `partition`, `replay`, `policy-diff`, `recovery` and `grant`. The report says, per class, which checks held, which guarantee and
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
- [`Class`](../../src/faults.rs) — the seven classes; `--faults` alone runs every one, a list runs those and fails the
  run if one cannot run here. `recovery` is [selective recovery](selective-recovery.md)'s page; `grant` (a
  [grant](../guide/00-concepts.md#grant) that is default-deny and holds across a restart) is on the
  [grants](grants.md) page — [`run`](../../src/faults/grant.rs) drives the declaration's first `[[rule]]` with
  `requires_grant = true` on the agent's node of the fleet: default-deny, the declared issuer's grant, the restart over
  the WAL, the binding, the record; it issues through the route's own decision
  ([`decide_grant`](../../src/agent/http/grants.rs)), the fleet having no gateway port.
- `replay` needs the binary built with the `sim` Cargo feature (`cargo build --bin mycelium --features cli,sim`); on
  any other it reads `not_in_build` naming `sim`. `revoke` needs `tls` and `compliance` in the build and a `[tls]`
  table; `restart` needs [`persistence`](../reference/configuration.md#the-fields); `policy-diff` needs `gateway` and
  `tls` and `--against <units-dir>`, the proposed revision; `recovery` needs `[persistence]` and a `gateway` + `tls`
  build; `grant` needs `[tls]`, `[persistence]`, a `compliance` build (the member keys a grant's checkpoint is verified
  against) and a declaration with a grant-required rule and a rule admitting someone to `grant.issue` on it — without
  the rules it reads `not_applicable`, naming which.
- `a_declaration_wire_check_refuses_is_refused_with_the_same_findings`, in [`src/faults.rs`](../../src/faults.rs):
  the plant is a unit nobody can wire; `test` refuses with `wire-check`'s own report, word for word.

## See it work

No runnable example exists: the [example index's capability matrix](../../examples/README.md#the-capability-matrix)
has no row for this feature. The CI job `Scaffold` runs it against the Rust scaffold's declaration with every class
the scaffold's build has (`revoke`, `restart`, `partition`, `recovery` and `grant` pass; `replay` reads not-in-build; a
planted widened `[[rule]]` is reported by `policy-diff`; a declaration `wire-check` refuses is refused with the same
text), the `one-hour` job runs `grant` and `recovery` on the published binary against the Python scaffold's declaration
(its steps 4 and 5, from the first release that carries both classes — v2.37.0 does not), and the job `embedded` builds the `sim` binary and runs `replay` against the co-op fixture. The
tests are in [`src/faults.rs`](../../src/faults.rs): `a_declaration_wire_check_refuses_is_refused_with_the_same_findings`,
`an_activation_runs_no_command`, `a_class_that_cannot_run_fails_only_when_asked_by_name`,
`the_restart_class_replays_a_synced_write_and_refuses_unreadable_state`,
`the_revoke_class_sees_the_three_verifiers_and_the_mandate_refuse`,
`the_partition_class_sees_a_dark_member_evicted_and_the_floor_refilled`,
`a_planted_policy_regression_is_reported_by_policy_diff`,
`the_grant_class_refuses_on_silence_issues_by_the_declared_rule_and_holds_across_a_restart` and
`the_grant_class_needs_a_grant_rule_tls_and_persistence_and_says_so`; the `sim` half is
`the_replay_class_replays_the_fleet_decision_for_decision` in [`tests/faults_replay.rs`](../../tests/faults_replay.rs).

## What it guarantees, and what it does not

- It adds no guarantee of its own; it exercises the ones in the guarantee catalogue —
  [`persist.unreadable_refused`](../reference/guarantee-catalogue.md) and
  [`persist.configured`](../reference/guarantee-catalogue.md) (`restart`), [`mesh.tls`](../reference/guarantee-catalogue.md)
  and [`mesh.signed_writes`](../reference/guarantee-catalogue.md) (`revoke`),
  [`id.revocation_authority`](../reference/guarantee-catalogue.md) (`revoke`; it reads `not_configured` under the
  development posture, the CA key being on the node), [`mesh.tls`](../reference/guarantee-catalogue.md) and
  [`persist.sync_mode`](../reference/guarantee-catalogue.md) (`grant`, `recovery`) — and the decision points in the rule catalogue:
  [`membership.governed`](../reference/rule-catalogue.md) (`partition`, `replay`), [`kv.expiry`](../reference/rule-catalogue.md)
  and [`cap.match`](../reference/rule-catalogue.md) (`partition`), [`ae.preflight`](../reference/rule-catalogue.md)
  (`policy-diff`, `grant` — who may issue, decided at `gateway:grants/issue`). Each is reported with its resolution on
  the fleet's node; an id that does not resolve `enforced` is listed as not exercised, with the reason the node gives.
- **It never runs an activation.** A unit's `[[activation]]` commands are counted and reported `not_applicable`;
  none is started, confined or not (D23).
- The fleet is the configuration's with stated rewrites, not the deployment: loopback, free ports, temporary
  directories, no gateway, every node bootstrapping to every other, a one-second health check, and **the declared
  profile not applied** — the fleet runs under `dev` and the report says which profile was declared: a profile's
  requirements include a gateway and code attachments the fleet has not, and `mycelium doctor` is what checks a
  profile. `--config` replaces the environment (`GOSSIP_*`), as `-c` does for the node. A `[[group]]` is governed
  to a band of one (floor one, ceiling one), settled before the cut, so a cut member is a deficit a survivor must
  fill; when the band moved between the settled read and the cut and a survivor already held the seat, the check
  says so and passes without exercising the re-fill. A cut is the node's runtime dropped: no departure is sent,
  its sockets close, the dark side is not observed. A class whose fleet cannot be built or started reads `refused`
  with the node's own words and fails the run, asked or not.
- `policy-diff` decides the declaration's call set — every `[[rule]]` as written and without the values it
  requires, every wired edge's two call shapes — at this command's own enforcement point under evaluators built from
  each revision's `[[rule]]`s, records each decision under one correlation id (the report names it; with `--keep`
  the journal stays and `mycelium explain --correlation <id> <journal>` renders the run as one chain), and
  re-decides the recorded calls under the proposed revision. An evidence record carries no argument values, so a
  journal written elsewhere (`--decisions <evidence.log>`) is re-decided from what its records carry — subject,
  operation, resource — and a record a proposed rule reads values for is reported *unattributable*, never as
  unchanged. The decisions are this command's, not a provider's: no gateway or provider door is driven. A rule's
  resource matches the call's as written, without a provider suffix.
- `replay` is the `sim` build's: each unit's node is recorded and replayed **alone** on a current-thread runtime
  under one kernel, as the node binary records; a divergence is reported where the seam raised it. The check holds
  when the kernel refused no request and the decisions reproduce; a recording may end a choice or two after the
  replay's run did (a choice the recorded run made while shutting down, on a wall-time race the paused clock does
  not have), which the report names with the first choice not reached rather than counting as a divergence. The schedule
  between nodes — frame arrival — is outside the seams (the inventory's §3.1.1), so a fleet recorded together
  diverges at its first cross-node channel choice; stated, not hidden.
- `grant` (adopter-surface X3) drives the state machine's door and the issuing door, in one process: the gateway's
  and the provider's doors on a declaration's fleet, and revocation with its drain bound, are the door tests' job
  (`src/agent/http/grant_tests.rs`), not this class's; the stop before the restart is graceful; a declared
  `tool:<name>` is also admitted at `tool:<name>@<the agent's node>` (the form the request record and the doors use),
  and the report says so; the route's scope rule (`grant:issue`) is outside its view, the evaluator's rule inside.
- `recovery` (adopter-surface X9) is its own page: [selective recovery](selective-recovery.md) — a lost
  acknowledgement probed before any retry, the closure of its record across the fleet's journals, each member
  recovered by its class as a new record, independent work untouched, explained from the records; its seamed form
  recorded into the corpus and replayed with the destination reached zero times.
- **Found by it, and fixed:** a governed member that went dark held the floor — its group entry is written once
  and withdrawn only by a graceful shutdown, so no survivor rolled to re-fill; the governor now counts a member only
  if it is this node or in its peer table ([`membership.governed`](../reference/rule-catalogue.md) revision 2), and
  with no peer at all reads the roster as written (a node restarting over persisted entries, or isolated, is not a
  fleet of one). The stale entry stays in the KV until the node returns: the fleet snapshot's group size, the
  `/gateway/mesh/group` members and the group-conflict diagnosis (`group_status`, the P-series detectors) still
  count it, so they can report an over-ceiling conflict for the state the governor is deliberately producing until
  then. **Found and stated:** a dark node's offers leave the view only after 3 × their `ttl_secs` — the class
  reports the time. **Found by `replay`, not fixed:** a node with `[persistence]` diverges on replay between the
  WAL write of its first entry and the governor's jitter draw (two seams whose order the schedule decides) — the
  class reports it as its first divergence; the single-node proof on the [proof page](../operations/what-is-proven.md)
  is over a node without persistence.
- `revoke` runs on three nodes of the fleet (the compromised node, the holder of its old key, the verifying peer),
  whatever the fleet's size; `restart` replays the fleet's own WAL, not the regression floor's golden fixtures under
  `tests/fixtures/persistence/`, which CI replays on their own; `partition`'s cut is a dropped runtime on wall time,
  not a seam.
- **Not built, stated:** a two-sided partition (the Docker suites', D6 — `--faults two-sided` says so); a load class (V1 on the
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
