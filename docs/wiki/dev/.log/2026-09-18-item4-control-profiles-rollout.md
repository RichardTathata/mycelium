## [2026-09-18] ingest | item 4 — the profile ladder wired through every governor, and the rollout runbook

Up: [dev](../dev.md) · record `docs/design/adaptive-stability.md` §7 · runbook `docs/operations/control-profiles.md`
· code `src/agent/{tuning_governor,opacity,control_profile}.rs`, `src/control/scenario_c.rs`,
`mycelium-wasm-host/src/provisioner.rs`.

### What landed

The Phase E item "shadow-mode rollout documented" — and the wiring the document needed to be true. §7 says
`Legacy` leaves today's governors untouched and `Observe` changes nothing but counts. After 4b and 4c only the
membership governor read the node's profile: the tuning gate's spacing and settling, the opacity boundary's
release spacing and the provisioner's rights refusal acted whatever the profile said. Now the tuning governor
holds its own copy of the profile (fanned out by `set_control_profile`), `Legacy` skips its contract entirely,
`Observe` counts and proceeds, the enforcing profiles hold; the opacity spacing answers *spaced or not* and the
profile says whether that is a hold; and the provisioner refuses only under `EnforceAllocated` — the Tier C
profile, a rights-backed bound being Tier C — counting a would-refuse otherwise and still recording the rejection.

### Three things worth carrying forward

1. **A runbook that cannot be followed is a finding about the code.** Writing "under `Observe` nothing is
   held" against 4b's code showed it was false for two governors and 4c's. The document was the test.
2. **`Legacy` is the ladder's own witness.** Scenario C's "settles" claim now runs under the enforcing
   profiles, where the breakers act; under `Legacy` the same schedules with the same breakers configured flap
   and chatter, and under `Observe` they do the same while counting every hold not made. The sweep pins the
   ladder as behaviour, not as a table.
3. **The counters keep their names across the ladder.** `held_by_spacing` under `Observe` counts would-holds,
   under `EnforceLocal` holds; `GovernorSnapshot.profile` says which. A dashboard does not jump when the
   profile does — the alternative, separate would-hold counters, doubles every graph for one bit of state.

### Defaults that moved back

4b's release spacing (1 s) and tuning spacing (two ticks) were on by default; under this change they act only
from `EnforceLocal` up. `Legacy`'s behaviour is again exactly pre-4b, which is what "today's governors,
untouched" was always supposed to mean.

### The gateway surface

`GET /gateway/govern` gains a `control` block — the profile, `would_hold`, `opacity_releases_spaced`, the tuning
counters with the profile they were taken under — and `POST /gateway/govern/profile` steps the ladder by name
under `govern:write`. `Profile::{name, parse}` are the wire vocabulary, pinned; an unknown name is refused rather
than read as `legacy`, because a typo must not step the ladder down. `TaskCtx::set_control_profile` is the one
write path, so the agent's API and the route cannot drift.

### Not done

A per-governor profile (one per node is the record's choice).

### Pages touched

- [history.md](../history.md) — the section; the ADR's §7 carries a dated note.
- `docs/operations/control-profiles.md` (new), indexed from the operations README; the testing page's scenario C
  paragraph; CHANGELOG.
