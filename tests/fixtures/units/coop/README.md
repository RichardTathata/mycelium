# The co-op deployment, as unit files

The Food-Rescue Co-op demos' vocabulary (`examples/coop/`), written as one unit file per deployable
unit — the first fixture directory for `mycelium wire-check` (`docs/plans/design-time-tooling.md`
§3, §4; W2's exit gate). Each file's stem is the unit's name. The deployment is meant to check
**green** with the artifact library beside it (`../artifacts`), and to report exactly one
*would bind by provisioning* warning: nobody deploys `route/optimize`; a depot installs it from the
catalogue when the worker's demand appears — the `provisioning` demo's own story.

```sh
cargo run --features cli --bin mycelium -- wire-check tests/fixtures/units/coop --library tests/fixtures/units/artifacts
```

Without `--library` the same directory reports `unwired requirement` and `presence unhostable` for
`route/optimize`, because then nothing could provide it — which is also correct.

**Authority.** Who may call what a depot provides is declared by the depots, because a node with `[authority]`
builds its evaluator from its own unit's rules and a provider decides a member's call for the calling **node**:

- `route/optimize` is a hosted component, decided as `cap.invoke` on `cap:route/optimize@{node}`. `wire-check` reads
  this edge as that provider decides it, from the depots' rules, and passes it.
- `depot/intake` is a deployed capability, a skill, decided as `skill.invoke` on `skill:depot/intake@{node}` for a
  call that names it. `wire-check` does not read a deployed edge as its provider decides it (an open row on
  `docs/operations/what-is-proven.md`); the worker declares no vocabulary, so the check says `ungoverned edge` and
  claims nothing about it.

Both rules are for `actor = "*"` — every admitted member, the widest rule there is — because a declaration carries
no placement and so has no node address to name; a deployment that knows its callers names them,
`actor = "node:<ip>:<port>"`. Until 2026-11-18 both edges were authorised in `auditor.toml`, by `skill.invoke` rules
for the unit principal `worker` under a mandate `worker` held: rules no depot loads, for a principal no provider
sees, and the check passed them. What a provider built from this directory does with each call, under the rules as
they are and as they stood, is shown by `the_coop_fixtures_hosted_edge_is_decided_at_the_provider_as_wire_check_reads_it`
and `the_coop_fixtures_deployed_edge_runs_under_the_rule_its_depot_declares` (`src/agent/declared_authority_tests.rs`).
