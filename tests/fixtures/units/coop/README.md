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
