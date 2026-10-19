## [2026-10-19] ingest | K2's seat-lease margin: derived from the claim's bounds, not one phase timeout

A fleet-consensus seat's lease is stamped when the claim's decision envelope is built, before the first ballot
(`src/consensus.rs` `build_carry`). The call is admitted only if a whole window of the lease remains
(`seat_admissible`), so the lease margin is exactly the latency a claim is allowed.

The margin was 2 s, the length of one `CLAIM_PHASE_TIMEOUT`. A claim that lost one ballot to its prepare timeout was
always refused "decided too late". Under parallel test load that happened about 1 fleet-history run in 50 (3 of 160;
0 of 160 after the fix).

`SEAT_LEASE_MARGIN_MS` is now derived in `src/agent/fleet_history.rs`: ballots × (2 phases + jitter) + the
convergence poll + local work, 19 225 ms in all. The claim sets its own retry jitter so the bound holds.

The lesson generalises: a latency allowance that is checked against a stamp taken before a retry loop must cover the
whole loop, not one iteration. The margin is liveness only. Two admissions on one seat stay at least a window apart
whatever the margin, so a longer one only holds a used seat longer, which admits fewer.

Restamping at commit was rejected because the stamp is inside the agreed envelope, and a later ballot may adopt it.

Fail-first: `a_claim_that_loses_a_ballot_to_its_timeout_is_admitted`, with the hook `consensus::TEST_SWALLOW_PREPARES`.

Pages: none changed. Repo: plan §3.5 (K2) and §3.6, CHANGELOG, `deprecations.md` §53,
`docs/guide/20-authorising-actions.md`, `src/lib.rs`'s namespace table, the rule catalogue's K2 effect.
