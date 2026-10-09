# 2026-10-09 — doc-coverage run 22

Diff-gated over #555–#577 (record: `docs/analysis/doc-coverage.md`, run 22). 0 ✗ and 0 `~` after fixes; nine
instructions that failed as written fixed — among them `error-handling.md`'s `consistent_set` match (did not compile
against the `#[non_exhaustive]` enum), guide 04's pre-2.30 consensus model beside #575's new text, and the stem trace
that `docker stop` never writes. Three code gaps reported, documented as they behave: the checkpointer reads the blob
route's 403 `refused` as `unauthorized`; `mycelium-stem` ignores SIGTERM; a prepare-phase promise shortfall is
counted as `no_voters`.
