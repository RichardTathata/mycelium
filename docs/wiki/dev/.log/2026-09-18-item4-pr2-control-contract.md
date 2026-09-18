## [2026-09-18] ingest | item 4 PR 2 — the contract types: classes, the predicate, profiles, spacing, settling

Up: [dev](../dev.md) · record `docs/design/adaptive-stability.md` §2, §3, §5, §7 · code `src/control.rs`.

### What landed

`mycelium::control`, ungated like `mandate`. Pure decisions only — no governor is changed, no actuator touched,
the rights ledger is PR 3. The decisive rule is one function (`holds_on_uncertainty`), the tests write the same
table by hand, and the two are pinned against each other — the partition-table shape from item 5.

### Two things worth carrying forward

1. **The predicate takes the real `ViewConfidence`.** It is public (`mycelium::ViewConfidence`), so there is no
   parallel "confidence" struct to drift from it, and WP5's `staleness_known()` becomes consequential: an
   isolated node's `max_staleness_ms: 0` is `Uncertainty::StalenessUnknown`, not "the freshest observer in the
   fleet". That was the WP5 fix's stated worry; the predicate is where it would have bitten.
2. **`Observe` is a distinct decision, not a flag.** `Decision::WouldHold(why)` proceeds and records; a caller
   that pattern-matches for `Proceed` cannot silently treat a would-hold as a proceed. Shadow-before-enforce is
   enforced by the type.

### Verified by planting

Inverting the rule for `ProtectiveShed` fails exactly the pinned pair (the rule/table agreement and the sweep);
making `Observe` enforce fails exactly the observe test. Nine tests.

### Pages touched

- [history.md](../history.md) — a PR 2 paragraph under the item 4 PR 1 section.
- The ADR's §9 table marks PR 2 landed.
