## [2026-09-25] ingest | closure plan C11: every frozen-clock read classified

Up: [dev](../dev.md) · page: [security](../security.md) · record `docs/design/authority-at-execution.md` §6 ·
code `scripts/check-hlc-current.sh`, `scripts/hlc-current-baseline.txt`, `mycelium-core/src/hlc.rs`.

#405 fixed the three decision sites the review found. This asked the general question: which other reads of
`Hlc::current()` decide something? Seven are the nonce seen-set's TTL, which is a decision, but one that fails in
the safe direction on a frozen clock: nonces age slower, so replays are remembered longer. They stay on the atomic
load because they run per message. The other two are stamps. One lease helper already had the right formula and now
names it. A per-file count, in CI like the sim-seam gate, keeps the next read from arriving unclassified.

The end-to-end test heeds #405's trap: it forces the gateway's HLC an hour into the past, because a freshly created
clock seeds from the wall clock and would pass against the frozen implementation too.
