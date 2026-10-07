## [2026-10-07] ingest | A3: the history source (#543)

- `mandate::history_source`: an appointment stream pinned to `(issuer, stream)`, walked back from the reader's
  checkpoint along signed `prev` digests; gaps, repeated terms and forks break or void coverage; baselines bind
  by head digest. `HandoverJournal` gains its scope and derives its span from its entries.
- `eligible_strict`: a consecutive-terms run at the limit behind a stale head is `Unknown` (found by the
  `curator_handover` example).
- Review history worth keeping: round 1 found the first version reporting full coverage over a false history
  (no pinned issuer, chaining by position, an unbound baseline); round 3 narrowed two claims to what holds
  (forks since the reader opened; revocation only past the reader) and recorded the new resolver position in
  `docs/design/knowledge-validity.md`.
- Pages: `dev/security.md` (§ handover eligibility).
