## [2026-11-15] ingest | the acceptor's model gives each acceptance its group (CR30, the follow-up to #784)

#787 fixed the carry between overlapping electorates behind `consensus_record_acceptance_group`. The model now
calls that fix: `model_internals` wraps `prepare_slot_tagged`, `claim_envelope_in` and `carry_hears` (the phase-1
wrapper applies it where `prepare_phase` does), the bridge reads where an acceptance was made back from the
engine, and `MigrationModel::recording` says which nodes record.

- With every node recording, `overlap` and `contended` (its CI bound) hold `ONE_VALUE_ACROSS`; `contended` at
  ballots to 3 held once by hand (110,929,684 states, 486 s). The reviewer's mixed case on #787 (R6c: every node
  but the stale member recording, the carrier a shared member holding the chosen value) holds.
- Pinned as counterexamples: nobody recording (the finding test, renamed `with_the_switch_off_…`), and the
  fallback's bound — the stale acceptance unrecorded and a carrier not holding the chosen value, where the carry
  hears every report as before 2.41.0 and the finding stands; `carry_hears` states that bound.
- The planted successor-early test asserts only that two different values are decided (#790's review).
- Pages touched: `dev/testing/acceptor-model.md` (the fix and its model, a table of recording × configuration,
  the rule's row in *What holds each rule*, what the group model does not reach). Outside the wiki:
  `docs/operations/what-is-proven.md` (the model row and the carry row), the critique plan's CR10 and CR30 State
  lines, `CHANGELOG.md`, the CI comment's state count.
