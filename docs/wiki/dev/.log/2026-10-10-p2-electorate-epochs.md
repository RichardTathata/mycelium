## [2026-10-10] ingest | P2 revised — the electorate pinned by identity and epoch (#601's review)

- #601's adversarial review found P2's first design pinned the electorate by member *count*, checked only by the
  proposer: a swap (D in, C out) or two chained steps (3→4→5) let two proposers decide on disjoint majorities. The
  user's decision: **order the electorate's steps**.
- Built: `ElectorateDecl { group, epoch, members, exclusive_default }`, each epoch a consensus decision on
  `electorate/{group}/{epoch}` (genesis unanimous among the named members; a one-member step by a majority of the
  epoch before), certified under `consensus/electorate-cert/` and adopted only when the certificate verifies
  (`ConsensusEngine::electorate_view`). `PrepareIn`/`ProposeIn`/`StaleElectorate` appended to `ConsensusMsg`; the
  acceptor's door `electorate_admits` answers only its own epoch and fences the epoch before a step it has accepted.
  Roster ≠ members by identity → `ElectorateMismatch` + `electorate_roster_mismatches`. The exclusive verbs decide in
  the fleet's `exclusive_default` group (`exclusive_propose`), the commitment award and capauthz included.
- Safety statement (decision record §8.3–8.4): per slot, within an epoch and across one step; **two or more steps
  after a decision it rests on the commit record** — closing that is state transfer at the step, recorded as the
  versioned-electorates plan, not built.
- Pages touched: `dev/architecture/runtime-invariants.md`, `dev/operations.md`. Outside the wiki: the decision record
  §3 and §8 (rewritten), threat model §7, guide 04, the FAQ, error-handling, diagnostics, production-readiness,
  configuration, `what-is-proven.md`, CHANGELOG.
