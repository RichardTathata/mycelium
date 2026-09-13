## [2026-09-13] ingest | WP5 — the governor cooldown as a parameter; staleness that admits it is unknown

Up: [dev](../dev.md) · plan §10.12.5 (item 4's standalone fix), private WP5 · code
`mycelium-core/src/config.rs` (`membership_cooldown_secs`, `membership_cooldown()`),
`src/agent/membership_governor.rs`, `src/agent/emergent.rs` (`ViewConfidence::staleness_known`).

**Finding.** The membership governor's cooldown was an unexported constant, `3 × health_check_interval_secs`
— an oscillation bound nobody could state, tune or pin, coupled to the ping cadence. `ViewConfidence` reported
`max_staleness_ms: 0` for a node that had heard from *no* peer: the isolated node looked like the freshest
observer in the fleet, the exact inversion RT1/RT2 exist to prevent.

**Change.** `membership_cooldown_secs: Option<u64>` (env `GOSSIP_MEMBERSHIP_COOLDOWN_SECS`), default `None` =
the historical value, explicit = that value bounded below at 1 s; the governor reads it once at start.
`staleness_known: bool` beside `max_staleness_ms` — additive, no shape change; `/stats` and the fleet snapshot
carry it. Two pins.

**Decided here.** *Live timing intents do not alter the cooldown.* The timing governor (M10.2) reconfigures
intervals live; the cooldown is a stated parameter of the membership governor, read at start, and a new value
takes effect on restart — the doc says so. Carrying it as an intent would make the oscillation bound itself a
gossiped soft state; item 4's `ControlSpec` (WP12 PR1) is the place to revisit that, with the harness to show it
is safe.
