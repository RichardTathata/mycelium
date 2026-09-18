## [2026-09-18] ingest | item 4 PR 3a — the journal split, and a blind spot in the forbidden-call check

Up: [dev](../dev.md) · record `docs/design/adaptive-stability.md` §4 · code `src/agent/journal.rs` (the
mechanism), `src/agent/evidence_journal.rs` (the AE profile) · gate `scripts/check-sim-seams.sh`.

### What landed

The AE evidence journal's mechanism — length-prefixed file, bounded queue, fsyncing writer, cursor reader —
moved to `agent::journal`; `EvidenceJournal` is a thin profile over it with every public name unchanged and
the replay stream still `ae/journal`. The rights ledger is the second user. Pure move; no behaviour change.

### Two things worth carrying forward

1. **The feature-gated dead-code trap, live.** The first cut declared the journal ungated with no ungated
   user, and `--no-default-features` clippy failed on every item in it. The gate CLAUDE.md describes did its
   job. The rule that fell out: **ungate a mechanism in the same change as its first ungated user**, never
   ahead of it. PR 3 ungates the journal with the ledger.
2. **The forbidden-call check has a blind spot for `#[cfg(test)]` on inner items.** It skips from the
   attribute to the next `}` at column 0, which is the end of the *enclosing* block when the item is inside an
   `impl` or `mod`. Test-only methods written inline hid `append`'s `tokio::time::timeout`; the baseline
   dropped from 5 to 4 for a file whose sites had not changed. The tell was the number moving when a diff of
   the sites showed none had. Rule: test-only methods go in a separate top-level `#[cfg(test)] impl`; recorded
   in the script's "what it cannot see". A baseline that moves when no site moved is the signal.

### Pages touched

- [history.md](../history.md) — a PR 3a paragraph under the item 4 section.
- [testing/testing.md](../testing/testing.md) is where the seams-check blind spot would belong as lore; the
  script's own header is canon and carries it. Not duplicated here.
