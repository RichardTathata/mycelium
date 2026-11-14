## [2026-11-14] ingest | a carry hears only acceptances made in the successor (CR30, step 1)

CR10's migration model found a counterexample and two engine tests confirmed it: a successor electorate that
shares a member with its predecessor decided a value the predecessor had outvoted. The cause is that an acceptor
keeps one record per slot whatever group asks; the design record's argument (iii) said the successor "held
nothing", which is true of the group and false of its members. The maintainer's decision of the same day: record
the group in the acceptor record (tag `0x05`), read everywhere, written behind `consensus_record_acceptance_group`
(off by default for one MINOR), and report an acceptance with no group as before.

- Pages touched: `dev/architecture/runtime-invariants.md` (the migration paragraph: the rule, where it is read,
  and why a proposer-side rule is wrong).
- Outside the wiki: `docs/design/consensus-electorate.md` (§8.3 carry bullet and (iii), §8.5, §8.6),
  `docs/guide/deprecations.md` §107, `docs/operations/what-is-proven.md` (a new row: does not hold by default),
  `docs/reference/configuration.md`, `docs/features/consensus-electorates.md`, `CHANGELOG.md`, the critique plan
  (row CR30).
- Code: `src/consensus.rs` (`MadeIn`, `ConsensusEngine::made_in` and `acceptor_record`, `prepare_slot_for`,
  `claim_envelope_in`/`claim_vote_in`, the `0x05` codec), `src/agent/migration.rs` (module doc step 3, the conflict
  warning), `src/agent/acceptor_guard.rs` (`merge_records` keeps the group), `mycelium-core/src/config.rs`.
- Learned on the way: a carry whose carrier holds a live record of another value never decides — its own
  acceptor refuses its own proposal at the proposal's ballot, which the proposer reads as contention and retries.
  With the group unrecorded that leaves the adopted value accepted by a majority of the successor.
