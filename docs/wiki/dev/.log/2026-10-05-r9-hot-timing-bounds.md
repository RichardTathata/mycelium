## [2026-10-05] ingest | realignment repairs R9 — the live timing setters keep validate()'s bounds

**What:** `src/agent/introspect.rs` — `set_health_check_interval_secs` / `set_reconnect_backoff_secs`
return `Result` and refuse a value above 3600 / 300; `hot_timing_setters_refuse_what_validate_refuses`
(new); the three existing callers in `lib_tests.rs` unwrap; the field table's rows and its R9 note; the
changelog; the plan row; this log.

**Durable knowledge:**

- **One bound, three doors.** A tunable that `validate()` bounds at start can also be set by an operator
  at runtime and by a fleet intent. The timing governor checked the bounds; the setters did not, so the
  operator's door — the one that also *pins* the node against the fleet — was the unguarded one.
- **A refused value changes nothing.** The pin is set only after the value is accepted, so a rejected
  call cannot leave the node ignoring the fleet on the strength of a value it never applied.
- **The live values start at the configured ones**, not at `0`; `0` is only the explicit *revert*.
  The witness's first draft assumed otherwise and was corrected before its fail-first run.
