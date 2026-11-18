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

**Authority.** The worker's two edges are read as their providers decide them. `depot/intake` is a deployed
capability: a skill, admitted by the auditor's rule for the worker under its `routing` mandate. `route/optimize` is a
hosted component: an enforcing provider decides a member's direct call of it as `cap.invoke` on
`cap:route/optimize@{node}` for the **calling node**, so its rule — any node, `cap.invoke`, `cap:route/optimize` — is
declared in each depot that could host it (`[authority]` loads a node's own unit's rules). Until 2026-11-18 the
optimizer was authorised by a `skill.invoke` rule for the worker, which that provider never reads; the check passed
it, and now names it (`unauthorisable edge`). The provider's decision on both rules is shown by
`the_coop_fixtures_hosted_edge_is_decided_at_the_provider_as_wire_check_reads_it`.
