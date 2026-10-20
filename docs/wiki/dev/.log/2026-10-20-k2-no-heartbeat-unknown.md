## [2026-10-20] ingest | K2: a member's entries without its heartbeat are unknown coverage

- #656's adversarial review (finding 1, predating it): `tally_state_counted`'s non-peer loop skipped a node with
  entries and no readable heartbeat — neither counted nor unknown — so a reader that received a crashed member's entries
  before its heartbeat under-counted (fails open).
- Now a verified member's entry for the counter, inside the window + staleness + drift or unreadable, is unknown
  coverage naming the node; a fabricated id is ignored (`verified_member`); without `[tls]` any such node is unknown.
  The bound is the collector's, so the node is consulted exactly until its orphaned entry is collected.
- Fail-first: `a_members_entries_without_its_heartbeat_are_unknown_coverage` (both halves: the reader unfixed, the
  membership check removed) and the extended `without_tls_a_non_peers_absence_is_not_established`.

Lesson: the same "skip the node when its dating record is missing" shape sat in both the collector and the reader; a
fix to one is a prompt to read the other.

Pages: `dev/dev.md` (K2 paragraph). Repo: `docs/guide/20-authorising-actions.md`, `docs/guide/deprecations.md` §56,
`CHANGELOG.md`, `docs/plans/post-360-hardening.md` §3.6.
