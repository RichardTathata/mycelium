## [2026-11-14] ingest | a carry hears only acceptances made in the successor (CR30, step 1, #787)

CR10's migration model found a counterexample and two engine tests confirmed it: a successor electorate that
shares a member with its predecessor decided a value the predecessor had outvoted. The cause is that an acceptor
keeps one record per slot whatever group asks; the design record's argument (iii) said the successor "held
nothing", which is true of the group and false of its members. The maintainer's decision of the same day: record
the group an acceptance is made in, in two steps, and treat an acceptance of unknown group as today.

The first build filtered at the acceptor. The adversarial review (F1) ran a trace in which that is worse than
doing nothing: a shared member's recorded copy of the chosen value hidden, another member's unrecorded, outvoted
acceptance heard. The acceptor cannot see the other reports, so the decision moved to the carrier, which asks
(`CarryPrepareTagged` / `PrepareAckTagged`, appended to `ConsensusMsg`) and applies one rule over its whole
quorum: every report names its group — hear the successor's only; any unknown — hear everything.

- Pages touched: `dev/architecture/runtime-invariants.md` (the migration paragraph: the rule, who applies it, and
  the two shapes not to return to), `dev/owner-signed-records.md` (the acceptor record's row: the new field, the
  rewritten-group forgery, fresh line references), `dev/testing/acceptor-model.md` and its log (they name #787).
- Outside the wiki: `docs/design/consensus-electorate.md` (§8.3 carry bullet and (iii), §8.5, §8.6),
  `docs/guide/deprecations.md` §107, `docs/operations/what-is-proven.md` (the row main already had, restated),
  `docs/operations/diagnostics.md`, `docs/reference/configuration.md`, `docs/features/consensus-electorates.md`,
  guide 04, `CHANGELOG.md`, the critique plan (row CR30, and CR10's finding paragraph).
- Code: `src/consensus.rs` (`MadeIn`, `carry_hears`, `ConsensusEngine::made_in` / `asks_tagged` /
  `acceptor_record`, `prepare_slot_tagged`, `claim_envelope_in`/`claim_vote_in`, the `0x05` codec, the two
  variants), `src/agent/migration.rs` (module doc step 3, the conflict warning), `src/agent/acceptor_guard.rs`
  (`merge_records` keeps the group; `group_rewritten`), `src/model_internals.rs` (the new field),
  `mycelium-core/src/config.rs`.
- Learned on the way: a carry whose carrier holds a live record of another value never decides — its own
  acceptor refuses its own proposal at the proposal's ballot, which the proposer reads as contention and retries.
  And a slot's group tag says nothing about where its acceptance was made: `claim_gated` tags at a promise too.
