## [2026-09-30] ingest | the release commit's red job: a test race, not a defect

**What:** `v2.17.0`'s release commit (versions and docs only) turned `main`'s strict `Test` job red on
`test_identity_anchor_recorded_and_conflict_flagged` — *conflict tripwire never fired* — after eight
green runs of the same code that day. Local: one failure in eight runs. Fixed by rewriting the test
(`src/agent/http.rs`), ten of ten after.

**Durable knowledge:**
- **The sealed record masks a legacy poison.** `helpers::resolve_identity_record` reads
  `sys/identity-signed/{node}` first and the legacy `sys/identity/{node}` pair only when no sealed
  record is held. A poisoned legacy pair therefore trips the anchor conflict only while the sealed
  record has not arrived — the test's outcome was the arrival order, which nothing controlled. That
  masking is the *right* behaviour (the sealed record is authoritative); the test's premise dated from
  before 2.14.0 made the sealed record the primary one.
- **Poison what the watcher reads.** The realistic attack against a node holding the sealed record
  is a newer tombstone of it (any node can gossip one) plus a poisoned legacy pair; the test does
  that and the tripwire fires deterministically. It also asserts the masked case explicitly, so the
  behaviour is documented rather than relied on by accident.
- **A strict job on a docs commit is still the branch's verdict.** `RELEASING.md` step 2b says to look
  at the branch, not at what fed it; the release commit itself is the branch's newest state, and its
  run is the one that counts — a red there is investigated to root cause, never re-run to green. The
  release stands because the shipped code is unchanged and the cause is the test; the release notes
  say so.

**Pages touched:** CHANGELOG (Fixed), the v2.17.0 release notes (appended).
