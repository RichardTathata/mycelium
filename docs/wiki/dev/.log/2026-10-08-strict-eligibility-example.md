## [2026-10-08] ingest | strict_eligibility calls the history source

`examples/strict_eligibility.rs` rewritten to call `mandate::history_source::history_from_appointment_stream`
end to end (doc-coverage run 21's HOW gap: nothing outside the source's tests called it). A food co-op's
rotating coordinator role, consecutive-terms limit 2 and cumulative limit 3 terms (cooling-off left to the
eligibility tests — it would contradict the co-op's two-in-a-row): signed records verified through `KnowledgeStore::put_signed`,
heads through `HeadCheckpoints::offer`, and each verdict asserted — eligible, ineligible, `Unknown` for a missing
record and for a reader with no checkpoint, the history surviving the old key's revocation (a hand-built key view, not the identity path) for a current-key
checkpoint and not for a revoked-key one, `ready` refusing an `Unknown`. Now `required-features = ["tls"]` and run
in CI's examples job. No public-API gap: everything the example needs was reachable.

Pages touched: `dev/examples.md` (contracts-axis bullet). Outside the wiki: guide 21 § Eligibility,
`examples/README.md` matrix row (CI ✓), CHANGELOG `[Unreleased]`.
