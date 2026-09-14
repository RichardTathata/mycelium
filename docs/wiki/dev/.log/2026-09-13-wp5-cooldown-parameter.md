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

**External review before merge (2026-09-14), two corrections.** (1) *"Additive" was wrong for both structs.*
`ViewConfidence` and `GossipConfig` are publicly constructible and not `#[non_exhaustive]`, so a new public
field breaks exhaustive literals and destructures — the reviewer compiled a consumer against the base and
watched it fail on `staleness_known`. For `ViewConfidence` the field is **gone**: `staleness_known()` is a
derived accessor (`peers_heard > 0`) and a hand-written `Serialize` emits the JSON key, so the Rust shape is
untouched and JSON consumers still gain the key. For `GossipConfig` the field stays — a config struct's
fields are its API — but the CHANGELOG now says plainly that it is a break for exhaustive literals (not
for the documented `Default` + assignment pattern), and a §6.6 ledger entry schedules `#[non_exhaustive]`
at `3.0.0` to end the series. The same break rode in with item 7's three config fields, already on `main`;
the ledger entry covers those too. (2) The accessor had been inserted between `apply_env_overrides`'s doc
block and its signature, so rustdoc attributed the override documentation to the cooldown accessor and left
the real method undocumented — moved above the block.

**Decided here.** *Live timing intents do not alter the cooldown.* The timing governor (M10.2) reconfigures
intervals live; the cooldown is a stated parameter of the membership governor, read at start, and a new value
takes effect on restart — the doc says so. Carrying it as an intent would make the oscillation bound itself a
gossiped soft state; item 4's `ControlSpec` (WP12 PR1) is the place to revisit that, with the harness to show it
is safe.
