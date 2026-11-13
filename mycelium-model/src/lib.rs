//! **A model of Mycelium's single-decree acceptor**, checked with [Stateright](https://docs.rs/stateright)
//! (critique plan row CR10, `docs/plans/external-critique-2026-11-12.md`).
//!
//! It checks the **design at a bound**; it does not prove the implementation. Three areas:
//!
//! - **slot reuse** ([`slot`]) — a slot is decided, its decision ends (a release, or a lease lapsing), and it is
//!   decided again. *At most one value is decided per slot and life.*
//! - **the lease's wall-clock end** ([`slot`]) — every node reads a lease on its own clock, and clocks differ by
//!   up to δ. *No holder is granted while another is inside its own deadline less δ.* The holder stops δ before
//!   its deadline: that margin is **row CR11's, as designed, and is not built** — this area checks the design, and
//!   the engine as it stands is the model's planted fault (no margin).
//! - **the electorate migration** ([`migration`]) — declare the successor, retire the predecessor, carry,
//!   complete. *Nothing is decided by a successor before the predecessor's retirement is visible.*
//!
//! # What the model shares with the engine
//!
//! The model's acceptors and proposers do not re-implement these: each call goes through
//! `mycelium::model_internals` (a feature only this crate enables) to the function the engine runs.
//!
//! | Decision | Engine function |
//! |---|---|
//! | promise a ballot, or refuse | `prepare_slot` (`src/consensus.rs:4662`) |
//! | accept a value at a ballot, or refuse | `claim_envelope` → `claim` → `may_accept` → `may_cast_vote_digest` (`src/consensus.rs:4830`, `4842`, `4876`, `4891`) |
//! | what a proposer carries after its promise quorum | `Carry::report`, `set_aside_finished`, `phase1_choice` → `choose_after_prepare` (`src/consensus.rs:4128`, `4183`, `4159`, `4730`) |
//! | whether a decision is over — released, or its lease past on the reader's clock — and which record is the top | `consensus_life::read_slot`, with `Envelope::identity`, `marker_key`, `encode_marker`, `decode_marker` (`src/consensus_life.rs:245`; the lapse is line 275) |
//! | the value Paxos decides | `consensus_life::Envelope` (`new`, `encode`, `decode`), `raw_of` (`src/consensus.rs:4174`) |
//! | whose votes count | `electorate_vote_filter` (`src/consensus.rs:534`) |
//! | an electorate's quorum; who is a member | `ElectorateDecl::quorum`, `ElectorateDecl::contains` (`src/agent/electorate.rs:134`, `139`) |
//!
//! # What the model re-implements — the gap between model and code
//!
//! Each of these is the engine's logic written again, because the engine's version is a method over a
//! `TaskCtx` (the store, tokio, the network), not a pure function. A change to one of them in the engine does
//! not change the model.
//!
//! | In the model | The engine's version |
//! |---|---|
//! | the order of a proposer's steps — draw, prepare, choose, claim, propose, count, commit ([`slot`]'s `start_ballot`, `finish_phase1`, `commit`) | `propose_inner`, `prepare_phase`, `collect_one_ballot`, `try_commit_if_ready`, `commit_decision` (`src/consensus.rs:1825`, `3650`, `3997`, `3806`, `3855`) |
//! | counting a vote or a promise only from the filter (`shared::counted`) | the inline `trust_set.contains(&voter.id_hash())` in `prepare_phase` and `collect_one_ballot` |
//! | a member's refusal moving a proposal (`shared::counted` again) | `ElectorateTag::sent_by_member` (`src/consensus.rs:453`) |
//! | an acceptor's answer to a prepare and to a proposal, with "a live decision of another value refuses" | `answer_prepare`, `run_consensus_listener` (`src/consensus.rs:5006`, `5205`) |
//! | the floor and the live decision, from one read of the slot (`slot`'s `view`) | `ConsensusEngine::floor`, `ended`, `live_digest` (`src/consensus.rs:1419`, `1377`, `1262`) |
//! | a call that decided another's value is not the caller's; the guard is issued on a live top record of the caller's value | `own_or_superseded` (`src/consensus.rs:4231`), `lock_guard_after_commit` (`src/agent/consensus_handle.rs:796`) |
//! | **the holder's margin** — it stops δ before its deadline (`slot::holding`) | **nothing: CR11 is not built.** Today's deadline is `started + ttl` (`src/agent/consensus_handle.rs:822`) |
//! | where a predecessor stands, and what a successor may then decide (`migration`'s `admits`) | `predecessor_state`, `successor_ready`, `incomplete_carry`, `migration_complete`, `carry_admissible`, `completion_acceptable` (`src/consensus.rs:2400`–`2590`), reached through `electorate_door` and `electorate_admits` (`2920`, `3129`) |
//! | closing a group at its retirement's promise and before a manifest (`migration`'s `RetireVote`, `Manifest`) | `claim_gated` (`GateKind::Retire`), `close_for_carry` (`src/consensus.rs:2699`, `2762`) |
//! | the carry set and the value carried (`migration`'s `Deliver(Manifest)`, `Begin(Carry)`) | `agent::migration` (`src/agent/migration.rs`) |
//!
//! # What is not modelled
//!
//! The decided floor (`consensus/decided/`: the model's floor is the ended record's ballot alone, so acceptors
//! refuse less than the engine's do); the collector and a shrunk acceptor (`collect_finished`,
//! `shrink_acceptor`); a restart and the digest-only acceptance it leaves; the acceptor guard; renewals; legacy
//! (pre-envelope) proposers and the legacy reading; the genesis and its supersession, drops, merges, the fleet's
//! exclusive default; cross-group proposals; trust slices, opacity and the topology gate; signatures and
//! certificates (a certificate is the votes it holds); more than one slot (slots share nothing but the node).
//!
//! # The bound
//!
//! Each configuration is finite and explored **in full**: every interleaving of its steps, not a sample and not
//! a depth cut. `tests/model.rs` pins the number of distinct states of each, so a change to a bound is made
//! in the open; `docs/operations/what-is-proven.md` states them.

#![deny(unsafe_code)]

pub mod migration;
pub mod shared;
pub mod slot;
