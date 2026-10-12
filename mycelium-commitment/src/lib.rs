//! # mycelium-commitment — the contract net, as five records
//!
//! The v3 contracts axis (`docs/plans/v3-contracts-axis.md` §6.9, §13.2) names the third of the
//! epoch's three coordination models — beside the tuple space's competitive `take` and the
//! blackboard's shared facts — and insists it is **a composition, not a subsystem**:
//!
//! > A commitment links a declared requirement, an authorized participant's acceptance, the relevant
//! > mandate and resource allocation, and the receipts and assessments that accumulate afterwards
//! > establishing what happened.
//!
//! This crate is that composition with a mechanism per record, and nothing the substrate does not
//! already have:
//!
//! | Record | Mechanism | Where it lives |
//! |---|---|---|
//! | **announce** — a declared requirement with terms, deadline, criteria | a KV head, declarer-owned | `cn/{requirement}` |
//! | **offer** — a participant's willingness; *not yet an obligation*; signed when the participant holds a key | `append` on a log stream | `log/cn/{requirement}/offers` |
//! | **award** — acceptance: one participant, one requirement, once | the deterministic lowest-participant rule (the tuple-space primary election's rule), written with **`set_with_receipt`** | `cn/{requirement}/award` |
//! | **report** — the awardee's receipt of the work: outcome, or *unknown* | `append` | `log/cn/{requirement}/reports` |
//! | **assess** — whether the requirement was satisfied; signed | `append`, Ed25519 over the record | `log/cn/{requirement}/assessments` |
//!
//! ## The rules, in the plan's words
//!
//! - **No component assigns another participant's obligation.** An award only records an *offer* the
//!   participant made; a requirement with no offers is a visible state ([`CommitmentRefusal::NoOffers`]),
//!   not a retry loop.
//!
//!   **And the rule now has a mechanism under it.** It used to be a convention: an offer names its
//!   participant in a field, so any member able to append to the stream could post an offer naming
//!   somebody else, and the deterministic rule would award that participant work they never offered
//!   — with every reader checking the award against the offers agreeing it was correct. An offer is
//!   signed when the participant holds a key ([`ContractNet::offer_signed`]), and a declarer that
//!   cares awards from [`offers_verified`](ContractNet::offers_verified), where a forgery is not a
//!   candidate. An award is signed by its declarer the same way ([`Award::signed`]).
//!
//!   **Unsigned stays legal and keeps meaning *unproven*.** A single-tenant mesh whose members are
//!   trusted equally has nothing to prove to itself, and [`plan_award`](ContractNet::plan_award)
//!   behaves exactly as before. What is not legal is reading an unsigned record as proof:
//!   [`verify_offer`] and [`verify_award`] answer `false` for one, never true-by-absence.
//!
//!   **What a verifying signature does and does not establish** — the same care item 1 takes with a
//!   receipt. It proves the holder of that key made the record. Whether the key belongs to the
//!   participant the record names is `sys/identity/{node}`'s question, and that is only as strong as
//!   `require_identity_proofs`, which is **default-off**: without proofs an admitted node can append
//!   its own key to another node's identity entry (`src/consensus.rs`, and why identity-auth Phase 2
//!   exists). So this is `SelfImposedPrevention` in the guardrails tier vocabulary under the default
//!   configuration, and it rises no higher on its own. Where the participant is not a node at all —
//!   a principal, a driver, a service — the key directory is the caller's, which is why
//!   `offers_verified` takes a resolver rather than reaching into the mesh.
//! - **One award per requirement.** A second award is **refused** ([`CommitmentRefusal::AlreadyAwarded`]),
//!   never written over the first. Two declarers racing to the same key is the companion's defining
//!   failure and gets a replay witness in CN2; here the refusal is the local half of that rule.
//! - **The award is a receipt-bearing operation, never a KV write alone.** [`Awarded`] carries the
//!   `WriteReceipt` item 1 returns, under `operation_id = cn/{requirement}/award`, so a retry of the
//!   award is recognisable and its durability is what the receipt says, no more.
//! - **An awarded participant that vanishes leaves an award with no report** — reported as such by
//!   whoever reads the streams; re-announcement is the declarer's decision under its own policy, never
//!   automatic reassignment by this crate.
//! - **The declarer is not the only permitted assessor.** Anyone may assess; an assessment names its
//!   assessor and is signed when the assessor holds a key — **unsigned means unproven**, and
//!   [`verify_assessment`] says `false` for it rather than true-by-absence. Offers and awards now
//!   follow the same rule; reports do not, and that is deliberate: a report is the *awardee's* claim
//!   about its own work, and an award already names the awardee, so a forged report is a claim about
//!   somebody else's obligation that the assessment layer exists to contradict.
//!
//! Heads and terms are in the gossip KV; offers, reports and assessments are the companion's streams
//! under one prefix (`cn/`, registered in `mycelium`'s namespace table). Time enters through the
//! caller (`now_ms`), so every decision here is pure in time and a harness can drive it.
//!
//! ## Which offers were in time: the declarer's close (post-360 row H)
//!
//! **No timestamp an offerer can write decides whether its offer was late.** `offered_at_ms` is a
//! field the offerer writes; so is the HLC segment of an offer's log key (`log/cn/{r}/offers/{hlc}/…`
//! is a KV key any member can `set` with any number in it — #609's review backdated one to
//! `0000000000010000` and won an award a minute after the deadline). So the deadline is not judged
//! on either. The **declarer closes** the requirement ([`ContractNet::close`]) — at or after
//! `deadline_ms` on its own clock (the `now_ms` it passes) — appending a [`Close`] to the
//! requirement's close stream `cn/{requirement}/closes` that names every offer the declarer's node holds
//! at that moment, each by its log position and the SHA-256 of its encoding, and binds the announcement
//! it closes (its deadline and digest). An award counts **only offers the close names**, and is refused
//! ([`CommitmentRefusal::NotClosed`]) before a close exists. An offer that reaches the stream after the
//! close is not a candidate whatever its key or stamp says.
//!
//! **Which close counts.** A stream rather than one key, so a forged close cannot pre-empt the
//! declarer's and an overwrite cannot erase it for good (#609's review, round 2). The checked path
//! ([`ContractNet::close_of_verified`], [`ContractNet::plan_award_verified`]) takes the **earliest close
//! that verifies under the declarer the caller pins** — earliest by the close's own signed
//! `closed_at_ms`, tie-broken by its digest, never by stream key (round 3: any member writes keys, and a
//! genuine close copied to a backdated key changes nothing). Forged entries are passed over. **Recovery
//! re-asserts; it never re-decides** (round 3): if the declarer's close is overwritten, `close` re-appends
//! the close that handle made, byte for byte, and a restarted declarer re-asserts the close it kept with
//! [`ContractNet::reassert_close`] — a re-close never admits an offer the first close did not. A
//! declarer that genuinely signs two different closes has equivocated: the earlier-dated counts, and both
//! sit on the stream. The unverified path ([`ContractNet::close_of`]) takes the earliest close (the same
//! order) that names the announcement's declarer, deadline and digest and is dated at or after the
//! deadline — **a forged unsigned close dated at or after the deadline still steers it**; any member can
//! append one, so that path is steerable, as its docs say. A close is refused by name before it would
//! exceed one KV write ([`MAX_CLOSE_WRITE_BYTES`]) and read back after its append.
//!
//! **The residual, stated:** the declarer decides which offers arrived in time. A signed close establishes **which offers the declarer counted** and that it bound this announcement; it does **not** establish when the declarer closed — `closed_at_ms` is the declarer's own claim, checked against nothing but the deadline it names. A declarer that closes late admits late offers; one
//! that omits an offer excludes it; a signed close makes either attributable to the declarer, not
//! detectable from the record alone. Unsigned, the close is like the announcement head: declarer-owned by
//! convention, writable by any member through the Rust API or a peer (the gateway's KV doors refuse
//! `cn/`). The HLC-contagion question the review raised (one fast member pulling every node's HLC
//! forward) does not apply: no HLC decides lateness.
//!
//! An award read back is checked too: [`ContractNet::award_of`] reports only a record that names its
//! requirement and operation, and [`ContractNet::award_of_verified`] only one the declarer **the caller
//! names** signed. Offers, reports and assessments are read only when they name the requirement whose
//! stream holds them.

#![deny(unsafe_code)]

use bytes::Bytes;
use mycelium::mandate::{Mandate, MandateRefusal, ResourceAuthority};
use mycelium::{CommitError, CommitReceipt, ConsensusConfig, GossipAgent, OperationId, ReceiptError, WriteReceipt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The companion's KV prefix — the same string as `mycelium::signal::kv_ns::CN`.
pub const CN_PREFIX: &str = "cn/";

fn head_key(requirement: &str) -> String {
    format!("{CN_PREFIX}{requirement}")
}
fn award_key(requirement: &str) -> String {
    format!("{CN_PREFIX}{requirement}/award")
}
fn closes_stream(requirement: &str) -> String {
    format!("cn/{requirement}/closes")
}

/// The most a single KV write may carry, key and value together: `mycelium_core::framing::MAX_KV_WRITE_BYTES`
/// (10 MiB less 64 KiB), which the public API does not re-export. A close is refused by name before it
/// would pass this, and every close is read back after its append — the read-back, not this copy, is what
/// proves the write, so a drift between the two is caught as `CloseNotWritten` rather than reported as `Ok`.
pub const MAX_CLOSE_WRITE_BYTES: usize = 10 * 1024 * 1024 - 64 * 1024;

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}
fn offers_stream(requirement: &str) -> String {
    format!("cn/{requirement}/offers")
}
fn reports_stream(requirement: &str) -> String {
    format!("cn/{requirement}/reports")
}
fn assessments_stream(requirement: &str) -> String {
    format!("cn/{requirement}/assessments")
}

/// A declared requirement: desired work, its terms, when offers close, what satisfies it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Announcement {
    pub requirement: String,
    pub declarer: String,
    pub terms: String,
    pub criteria: String,
    /// When offers close, on the declarer's clock: the declarer may [`close`](ContractNet::close) the
    /// requirement at or after it, and an award counts only the offers the close names. No stamp an
    /// offerer writes — `offered_at_ms` or its log key's HLC — is compared against it.
    pub deadline_ms: u64,
    pub announced_at_ms: u64,
}

impl Announcement {
    /// Lowercase hex SHA-256 of the announcement's encoding — what a [`Close`] binds.
    pub fn sha256(&self) -> String {
        sha256_hex(&encode(self))
    }
}

/// A participant's willingness to undertake a requirement. Not yet an obligation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    pub requirement: String,
    pub participant: String,
    pub bid: String,
    /// The offerer's own stamp — part of the signed record, **not** what decides lateness (the
    /// declarer's [`Close`] does; see the crate doc).
    pub offered_at_ms: u64,
    /// Ed25519 over [`Offer::canonical_bytes`]; empty means unsigned.
    ///
    /// **Why an offer is the record that most needs one.** The crate's first rule is that *no
    /// component assigns another participant's obligation* — and an unsigned offer defeats exactly
    /// that, because any member who can append to the stream can post an offer naming somebody
    /// else as `participant`, and [`AwardRule::LowestParticipant`] will duly award them work they
    /// never offered to do. A signature is what turns that rule from a convention into something a
    /// declarer can check ([`ContractNet::offers_verified`]).
    #[serde(default)]
    pub signature: Vec<u8>,
}

impl Offer {
    /// The bytes a signature covers: the record with its signature emptied, as canonical JSON.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let unsigned = Offer { signature: Vec::new(), ..self.clone() };
        serde_json::to_vec(&unsigned).unwrap_or_default()
    }

    /// Whether the participant signed at all. `false` means *unproven*, not *forged*.
    pub fn is_signed(&self) -> bool {
        !self.signature.is_empty()
    }
}

/// Verify an offer against the **participant's** Ed25519 public key. `false` for an unsigned one,
/// and `false` for one signed by anybody else — which is the whole point: the key belongs to the
/// participant the offer names, so a forgery fails even when it is validly signed by its forger.
pub fn verify_offer(offer: &Offer, key: &[u8; 32]) -> bool {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let Ok(vk) = VerifyingKey::from_bytes(key) else { return false };
    let Ok(sig) = Signature::from_slice(&offer.signature) else { return false };
    vk.verify(&offer.canonical_bytes(), &sig).is_ok()
}

/// One offer as a [`Close`] names it: its log position and the SHA-256 of its encoding, so a copy of
/// the record at another position, or another record at this position, is not the offer named.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosedOffer {
    pub hlc: u64,
    /// Lowercase hex SHA-256 of the offer's encoding (`serde_json`, signature included).
    pub sha256: String,
}

impl ClosedOffer {
    /// How a close names the offer at `hlc`.
    pub fn of(hlc: u64, offer: &Offer) -> Self {
        Self { hlc, sha256: sha256_hex(&encode(offer)) }
    }
}

/// The declarer's close: **which offers arrived in time**, in the declarer's own words (post-360 row H).
/// Appended to the requirement's close stream `cn/{requirement}/closes` at or after the deadline on the
/// declarer's clock; an award counts only the offers it names. It binds the announcement it closes —
/// its deadline and its digest — so a close made for one announcement is not a close of another under
/// the same name. See the crate doc for what this does and does not establish.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Close {
    pub requirement: String,
    pub declarer: String,
    /// The deadline of the announcement this closes.
    pub deadline_ms: u64,
    /// [`Announcement::sha256`] of the announcement this closes.
    #[serde(default)]
    pub announcement_sha256: String,
    /// When the declarer says it closed, on its own clock. **The declarer's claim, not a measurement**:
    /// signed with the close, so a declarer that closes "at the deadline" while naming offers that came
    /// later has signed the pair — but nothing here checks `closed_at_ms` against any clock but the
    /// declarer's own.
    pub closed_at_ms: u64,
    /// Every offer the declarer's node held when it closed.
    pub offers: Vec<ClosedOffer>,
    /// Ed25519 over [`Close::canonical_bytes`], by the declarer; empty means unsigned.
    #[serde(default)]
    pub signature: Vec<u8>,
}

impl Close {
    /// The bytes a signature covers: the record with its signature emptied, as canonical JSON.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let unsigned = Close { signature: Vec::new(), ..self.clone() };
        serde_json::to_vec(&unsigned).unwrap_or_default()
    }

    /// Whether the declarer signed at all. `false` means *unproven*, not *forged*.
    pub fn is_signed(&self) -> bool {
        !self.signature.is_empty()
    }

    /// Whether this close names the offer at `hlc`.
    pub fn names(&self, hlc: u64, offer: &Offer) -> bool {
        let named = ClosedOffer::of(hlc, offer);
        self.offers.contains(&named)
    }

    /// Lowercase hex SHA-256 of the close's encoding, signature included — what an [`Award`] records as
    /// the close it was chosen from.
    pub fn sha256(&self) -> String {
        sha256_hex(&encode(self))
    }

    /// Whether this is a close of `announcement` by its declarer, at or after its deadline — the checks
    /// both paths make before any signature.
    pub fn closes(&self, announcement: &Announcement) -> bool {
        self.requirement == announcement.requirement
            && self.declarer == announcement.declarer
            && self.deadline_ms == announcement.deadline_ms
            && self.announcement_sha256 == announcement.sha256()
            && self.closed_at_ms >= announcement.deadline_ms
    }
}

/// Verify a close against the **declarer's** Ed25519 public key. `false` for an unsigned one.
pub fn verify_close(close: &Close, key: &[u8; 32]) -> bool {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let Ok(vk) = VerifyingKey::from_bytes(key) else { return false };
    let Ok(sig) = Signature::from_slice(&close.signature) else { return false };
    vk.verify(&close.canonical_bytes(), &sig).is_ok()
}

/// The acceptance: one participant's offer, taken, once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Award {
    pub requirement: String,
    pub participant: String,
    pub declarer: String,
    /// The log position of the offer awarded — the award records an offer that was made.
    pub offer_hlc: u64,
    pub awarded_at_ms: u64,
    /// The operation the award was written under: `cn/{requirement}/award`.
    pub operation_id: String,
    /// [`Close::sha256`] of the close the award was chosen from, so an award can be checked against the
    /// offers that close named. Empty on an award planned before closes existed.
    #[serde(default)]
    pub close_sha256: String,
    /// Ed25519 over [`Award::canonical_bytes`], by the **declarer**; empty means unsigned.
    ///
    /// An award names a `declarer`, and without a signature that name is a claim rather than a
    /// fact: any member can write `cn/{requirement}/award` and say the announcement's declarer made
    /// it. The signature is what lets a participant check that the acceptance came from the party
    /// that announced the requirement, rather than trusting the key it was written under.
    #[serde(default)]
    pub signature: Vec<u8>,
}

impl Award {
    /// The bytes a signature covers: the record with its signature emptied, as canonical JSON.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let unsigned = Award { signature: Vec::new(), ..self.clone() };
        serde_json::to_vec(&unsigned).unwrap_or_default()
    }

    /// Sign this award as the declarer. Consuming, so a signed award is built once and not
    /// mutated afterwards — a mutated one would carry a signature over bytes it no longer has.
    pub fn signed(mut self, key: &ed25519_dalek::SigningKey) -> Self {
        use ed25519_dalek::Signer;
        self.signature = key.sign(&self.canonical_bytes()).to_bytes().to_vec();
        self
    }

    /// Whether the declarer signed at all. `false` means *unproven*, not *forged*.
    pub fn is_signed(&self) -> bool {
        !self.signature.is_empty()
    }
}

/// Verify an award against the **declarer's** Ed25519 public key. `false` for an unsigned one.
pub fn verify_award(award: &Award, key: &[u8; 32]) -> bool {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let Ok(vk) = VerifyingKey::from_bytes(key) else { return false };
    let Ok(sig) = Signature::from_slice(&award.signature) else { return false };
    vk.verify(&award.canonical_bytes(), &sig).is_ok()
}

/// What became of the work, in the receipts vocabulary: an outcome, or *unknown* — never
/// "nothing happened".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    Fulfilled,
    Failed(String),
    /// The participant cannot say — a timeout, a lost acknowledgement. The honest report.
    Unknown,
}

/// The awardee's report against the award's operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub requirement: String,
    pub participant: String,
    pub outcome: Outcome,
    /// The award's operation id, so a report is tied to *this* acceptance and no other.
    pub operation_id: String,
    pub reported_at_ms: u64,
}

/// Whether the requirement was satisfied, by someone with standing to say — not necessarily the
/// declarer. `signature` is Ed25519 over [`Assessment::canonical_bytes`]; empty means unsigned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assessment {
    pub requirement: String,
    pub assessor: String,
    pub satisfied: bool,
    pub note: String,
    pub assessed_at_ms: u64,
    #[serde(default)]
    pub signature: Vec<u8>,
}

impl Assessment {
    /// The bytes a signature covers: the record with its signature emptied, as canonical JSON.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let unsigned = Assessment { signature: Vec::new(), ..self.clone() };
        serde_json::to_vec(&unsigned).unwrap_or_default()
    }

    /// Whether the assessor signed at all. `false` means *unproven*, not *forged*.
    pub fn is_signed(&self) -> bool {
        !self.signature.is_empty()
    }
}

/// Verify an assessment against the assessor's Ed25519 public key. `false` for an unsigned one.
pub fn verify_assessment(assessment: &Assessment, key: &[u8; 32]) -> bool {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let Ok(vk) = VerifyingKey::from_bytes(key) else { return false };
    let Ok(sig) = Signature::from_slice(&assessment.signature) else { return false };
    vk.verify(&assessment.canonical_bytes(), &sig).is_ok()
}

/// An award that was written, with the receipt that says what the write established.
#[derive(Clone, Debug)]
pub struct Awarded {
    pub award: Award,
    pub receipt: WriteReceipt,
}

/// An award committed through a consensus round (CN2), with item 1's commit receipt: the cluster
/// agreed, and `commit.local_durability` says whether *this* node has it on disk — two statements.
#[derive(Clone, Debug)]
pub struct AwardedLinearizable {
    pub award: Award,
    pub commit: CommitReceipt,
}

/// Why an award was not made. **Each is a visible state, not a retry.**
///
/// `#[non_exhaustive]` since the release that boxed [`CommitmentRefusal::AlreadyAwarded`]: the
/// vocabulary of *why an award did not happen* has grown twice already (the mandate refusals at
/// CN3), and one announced break is better than a series of unannounced ones. A `_` arm is required
/// downstream and **must fail closed** — an unrecognised refusal is *no award was made*, never *an
/// award was made*.
#[derive(Debug)]
#[non_exhaustive]
pub enum CommitmentRefusal {
    /// No announcement head exists for the requirement.
    NotAnnounced,
    /// The requirement was announced and nobody offered before its deadline.
    NoOffers,
    /// The requirement already has an award; it is returned, never overwritten.
    ///
    /// **Boxed** since the signature field: an `Award` carries six owned fields and a signature,
    /// and an unboxed one in the error variant makes every `Result` in this crate as large as its
    /// rarest outcome. The box costs an allocation on a path that has already lost a race.
    AlreadyAwarded(Box<Award>),
    /// The award's write did not yield a receipt (item 1's vocabulary: the fate may be unknown).
    Receipt(ReceiptError),
    /// A linearizable award's round produced no commit before its deadline: the award **may or may
    /// not** have committed elsewhere. Not "no award" — a retry resolves it as `AlreadyAwarded` or
    /// as a fresh commit.
    AwardUnknown { ballots_tried: u32 },
    /// A linearizable award's round was refused for a reason other than an existing award.
    NotCommitted(String),
    /// The acceptor's mandate did not authorize the acceptance (CN3): a stale holder's award is
    /// `Superseded { installed, presented }` — the resource has moved to a later epoch than the
    /// one this mandate was minted under. Refused **before** any write.
    Mandate(MandateRefusal),
    /// The mandate presented is not the acceptor's own: it names another holder.
    MandateNotTheAcceptors { holder: String, acceptor: String },
    /// A record in the medium did not decode.
    Encoding(String),
    /// The requirement has no [`Close`]: the declarer has not said which offers arrived in time, so
    /// nothing can be awarded yet (post-360 row H).
    NotClosed,
    /// The verified path found a close that the declarer it was told to trust did not sign.
    CloseUnproven,
    /// Only the announcement's declarer closes a requirement.
    NotTheDeclarer { declarer: String, me: String },
    /// A close before the deadline, on the declarer's clock.
    BeforeDeadline { deadline_ms: u64, now_ms: u64 },
    /// The requirement is already closed — a close this declarer would itself select exists **in this
    /// node's view** (a local read: two of the declarer's nodes closing at once both write, and every
    /// reader selects the earlier). The close is returned.
    AlreadyClosed(Box<Close>),
    /// The close would not fit one KV write ([`MAX_CLOSE_WRITE_BYTES`]): refused before anything is
    /// written. A requirement with that many offers needs compacting or a narrower market.
    CloseTooLarge { bytes: usize, max: usize },
    /// The close was appended but is not in this node's store when read back: not written.
    CloseNotWritten,
    /// [`ContractNet::reassert_close`] was given a close that does not close the current announcement
    /// (another requirement, deadline or announcement digest, or dated before the deadline).
    CloseNotForThisAnnouncement,
    /// The award slot holds a record that decodes as an award **for another requirement or
    /// operation** — somebody else's, not this requirement's award.
    SlotHoldsAnotherRecord { requirement: String, operation_id: String },
    /// The award slot holds an award the declarer the caller trusts did not sign: the slot is taken
    /// and nothing proves by whom. A linearizable slot stays taken — visible, not repairable here.
    UnprovenAwardHolds(Box<Award>),
}

impl std::fmt::Display for CommitmentRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAnnounced => f.write_str("the requirement was never announced"),
            Self::NoOffers => f.write_str("nobody offered before the deadline — a visible state, not a retry"),
            Self::AlreadyAwarded(a) => write!(f, "already awarded to {} (offer at hlc {})", a.participant, a.offer_hlc),
            Self::Receipt(e) => write!(f, "the award's write returned no receipt: {e:?}"),
            Self::AwardUnknown { ballots_tried } => write!(
                f,
                "the award's round reached no commit after {ballots_tried} ballot(s) — it may or may not have committed; retry"
            ),
            Self::NotCommitted(e) => write!(f, "the award's round was refused: {e}"),
            Self::Mandate(e) => write!(f, "the acceptor's mandate does not authorize the acceptance: {e}"),
            Self::MandateNotTheAcceptors { holder, acceptor } => {
                write!(f, "the mandate names {holder} as holder, but the acceptor is {acceptor}")
            }
            Self::Encoding(e) => write!(f, "a record did not decode: {e}"),
            Self::NotClosed => f.write_str("the declarer has not closed the requirement — no award before the close"),
            Self::CloseUnproven => f.write_str("the close is not signed by the declarer the caller trusts"),
            Self::NotTheDeclarer { declarer, me } => write!(f, "only the declarer {declarer} closes the requirement, not {me}"),
            Self::BeforeDeadline { deadline_ms, now_ms } => {
                write!(f, "the deadline is {deadline_ms} ms and the declarer's clock reads {now_ms} ms — too early to close")
            }
            Self::AlreadyClosed(c) => write!(f, "already closed at {} ms naming {} offer(s)", c.closed_at_ms, c.offers.len()),
            Self::CloseTooLarge { bytes, max } => write!(f, "the close is too large: {bytes} bytes, one KV write carries at most {max}"),
            Self::CloseNotWritten => f.write_str("the close was not in the store when read back — not written"),
            Self::CloseNotForThisAnnouncement => f.write_str("that close does not close the current announcement"),
            Self::SlotHoldsAnotherRecord { requirement, operation_id } => write!(
                f,
                "the award slot holds a record for requirement {requirement:?} (operation {operation_id:?}), not this one's award"
            ),
            Self::UnprovenAwardHolds(a) => write!(
                f,
                "the award slot holds an award to {} naming declarer {} that the trusted declarer did not sign",
                a.participant, a.declarer
            ),
        }
    }
}
impl std::error::Error for CommitmentRefusal {}

/// How an award is chosen among the offers made before the deadline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AwardRule {
    /// The tuple-space primary election's rule: the lowest participant id wins, ties on the
    /// earlier offer. Deterministic from the offers alone — every reader of the streams reaches
    /// the same conclusion, which is what lets the award be checked rather than trusted.
    LowestParticipant,
}

impl AwardRule {
    /// **Pure** — the offer this rule awards, or `None` when there is nothing to award.
    pub fn choose<'a>(&self, offers: &'a [(u64, Offer)]) -> Option<&'a (u64, Offer)> {
        match self {
            AwardRule::LowestParticipant => {
                offers.iter().min_by(|(ha, a), (hb, b)| a.participant.cmp(&b.participant).then(ha.cmp(hb)))
            }
        }
    }
}

/// The contract net, from one participant's point of view: a handle over the public KV API.
///
/// `me` is this participant's name in the records — a node, a principal, a driver; the companion
/// does not mint identity and does not check it (item 7's caller context and item 5's mandates are
/// where authority lives; CN3 checks the award against a mandate epoch where one exists).
#[derive(Clone)]
pub struct ContractNet {
    agent: Arc<GossipAgent>,
    me: String,
    /// The closes this handle minted, by requirement (shared by its clones; lock-order row 59): what
    /// [`close`](Self::close) re-asserts when its own close is no longer on the stream. In memory only — a
    /// declarer that restarts keeps the close `close` returned and calls
    /// [`reassert_close`](Self::reassert_close).
    minted: Arc<std::sync::Mutex<std::collections::HashMap<String, Close>>>,
}

impl ContractNet {
    pub fn new(agent: Arc<GossipAgent>, me: impl Into<String>) -> Self {
        Self { agent, me: me.into(), minted: Arc::default() }
    }

    fn remembered_close(&self, requirement: &str) -> Option<Close> {
        self.minted.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(requirement).cloned()
    }

    fn remember_close(&self, close: &Close) {
        self.minted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(close.requirement.clone(), close.clone());
    }

    /// Who this handle acts as.
    pub fn me(&self) -> &str {
        &self.me
    }

    /// **Announce** a requirement: the head `cn/{requirement}`, declarer-owned.
    pub fn announce(
        &self,
        requirement: impl Into<String>,
        terms: impl Into<String>,
        criteria: impl Into<String>,
        deadline_ms: u64,
        now_ms: u64,
    ) -> Announcement {
        let a = Announcement {
            requirement: requirement.into(),
            declarer: self.me.clone(),
            terms: terms.into(),
            criteria: criteria.into(),
            deadline_ms,
            announced_at_ms: now_ms,
        };
        let _ = self.agent.kv().set(head_key(&a.requirement).as_str(), encode(&a));
        a
    }

    /// The announcement head, if any.
    pub fn announcement(&self, requirement: &str) -> Option<Announcement> {
        self.agent.kv().get(&head_key(requirement)).and_then(|b| decode(&b))
    }

    /// **Offer** against an announced requirement. Returns the offer's log position, or `None`
    /// when the requirement was never announced — an offer needs something to offer against.
    pub fn offer(&self, requirement: &str, bid: impl Into<String>, now_ms: u64) -> Option<u64> {
        self.offer_signed(requirement, bid, None, now_ms)
    }

    /// **Offer, signed** — the same verb with the participant's key, so a declarer can check that
    /// the offer came from the participant it names ([`offers_verified`](Self::offers_verified)).
    ///
    /// `None` for the key is [`offer`](Self::offer) and stays legal: a single-tenant mesh where
    /// every member is trusted equally has nothing to prove to itself. What is *not* legal is
    /// reading an unsigned offer as proof — [`verify_offer`] answers `false` for it rather than
    /// true-by-absence.
    ///
    /// **What this does and does not establish.** A verifying signature proves the holder of that
    /// key made the offer. Whether the key belongs to the participant it names is
    /// `sys/identity/{node}`'s question, and *that* is only as strong as
    /// `require_identity_proofs` — **default-off** — because without proofs an admitted node can
    /// append its own key to another's identity entry. In the guardrails tier vocabulary this is
    /// `SelfImposedPrevention` under the default configuration and rises no higher on its own.
    pub fn offer_signed(
        &self,
        requirement: &str,
        bid: impl Into<String>,
        key: Option<&ed25519_dalek::SigningKey>,
        now_ms: u64,
    ) -> Option<u64> {
        use ed25519_dalek::Signer;
        self.announcement(requirement)?;
        let mut o = Offer {
            requirement: requirement.to_string(),
            participant: self.me.clone(),
            bid: bid.into(),
            offered_at_ms: now_ms,
            signature: Vec::new(),
        };
        if let Some(k) = key {
            o.signature = k.sign(&o.canonical_bytes()).to_bytes().to_vec();
        }
        Some(self.agent.kv().append(&offers_stream(requirement), encode(&o)))
    }

    /// Every offer on the requirement, with its log position, in log order — only records that name
    /// **this** requirement: a signed offer copied from another requirement's stream is that
    /// requirement's offer, not this one's (#609's review, finding 3).
    pub fn offers(&self, requirement: &str) -> Vec<(u64, Offer)> {
        self.agent
            .kv()
            .scan_log(&offers_stream(requirement), 0, u64::MAX)
            .into_iter()
            .filter_map(|e| decode::<Offer>(&e.value).map(|o| (e.hlc, o)))
            .filter(|(_, o)| o.requirement == requirement)
            .collect()
    }

    /// **Close** the requirement: the declarer's statement of which offers arrived in time — every offer
    /// this node holds now, named by log position and digest — appended to `cn/{requirement}/closes`,
    /// binding the current announcement, signed with `key` when one is given. Only the announcement's
    /// declarer closes, and only at or after `deadline_ms` on its own clock (`now_ms`).
    ///
    /// **Once — as this declarer reads it.** With a key, it is [`CommitmentRefusal::AlreadyClosed`] only
    /// when a close of the current announcement verifying under that key is visible; without one, only
    /// when the unverified path would already select a close. A forged close written first does not stop
    /// the declarer's first close. `AlreadyClosed` is a local read.
    ///
    /// **Recovery re-asserts; it never re-decides.** If this declarer's close is no longer visible (it was
    /// overwritten), calling `close` again re-appends **the close this handle made, byte for byte** — the
    /// same offers, the same `closed_at_ms` — so a re-close never admits an offer the first did not (#609's
    /// review, round 3). The handle remembers its closes in memory only: **keep the `Close` this returns**,
    /// and after a restart re-assert it with [`reassert_close`](Self::reassert_close). A handle with no
    /// memory of a close (a new one, or after a restart) cannot tell an overwritten close from none, and
    /// `close` then mints a fresh one from what the node holds now — the residual, stated: a declarer that
    /// restarts, does not keep its close, and has it overwritten everywhere, re-decides.
    ///
    /// The close is refused before writing when it would not fit one KV write
    /// ([`CommitmentRefusal::CloseTooLarge`], [`MAX_CLOSE_WRITE_BYTES`]), and read back after the append
    /// ([`CommitmentRefusal::CloseNotWritten`]) — `Ok` means it is in this node's store.
    pub fn close(
        &self,
        requirement: &str,
        key: Option<&ed25519_dalek::SigningKey>,
        now_ms: u64,
    ) -> Result<Close, CommitmentRefusal> {
        use ed25519_dalek::Signer;
        let announcement = self.announcement(requirement).ok_or(CommitmentRefusal::NotAnnounced)?;
        if announcement.declarer != self.me {
            return Err(CommitmentRefusal::NotTheDeclarer { declarer: announcement.declarer, me: self.me.clone() });
        }
        if now_ms < announcement.deadline_ms {
            return Err(CommitmentRefusal::BeforeDeadline { deadline_ms: announcement.deadline_ms, now_ms });
        }
        let own_key = key.map(|k| k.verifying_key().to_bytes());
        if let Some(existing) = self.select_close(&announcement, own_key.as_ref()) {
            return Err(CommitmentRefusal::AlreadyClosed(Box::new(existing)));
        }
        // Not visible — overwritten, or never made. If this handle made it, re-assert **that** decision, byte
        // for byte: a re-close never names an offer the first close did not (#609's review, round 3).
        if let Some(original) = self.remembered_close(requirement)
            && original.closes(&announcement)
            && own_key.is_none_or(|k| verify_close(&original, &k))
        {
            return self.append_close(original);
        }
        let mut close = Close {
            requirement: requirement.to_string(),
            declarer: self.me.clone(),
            deadline_ms: announcement.deadline_ms,
            announcement_sha256: announcement.sha256(),
            closed_at_ms: now_ms,
            offers: self.offers(requirement).iter().map(|(hlc, o)| ClosedOffer::of(*hlc, o)).collect(),
            signature: Vec::new(),
        };
        if let Some(k) = key {
            close.signature = k.sign(&close.canonical_bytes()).to_bytes().to_vec();
        }
        let close = self.append_close(close)?;
        self.remember_close(&close);
        Ok(close)
    }

    /// **Re-assert a close this declarer made**, byte for byte — the recovery for a declarer whose close
    /// was overwritten on the stream and which no longer holds it in memory (it restarted): keep the
    /// close [`close`](Self::close) returned, and pass it here. Refused unless it is this declarer's close
    /// of the current announcement ([`CommitmentRefusal::NotTheDeclarer`],
    /// [`CommitmentRefusal::CloseNotForThisAnnouncement`]); size-checked and read back like a close.
    pub fn reassert_close(&self, close: &Close) -> Result<Close, CommitmentRefusal> {
        let announcement = self.announcement(&close.requirement).ok_or(CommitmentRefusal::NotAnnounced)?;
        if close.declarer != self.me {
            return Err(CommitmentRefusal::NotTheDeclarer { declarer: close.declarer.clone(), me: self.me.clone() });
        }
        if !close.closes(&announcement) {
            return Err(CommitmentRefusal::CloseNotForThisAnnouncement);
        }
        let close = self.append_close(close.clone())?;
        self.remember_close(&close);
        Ok(close)
    }

    /// Append a close's exact encoding to its stream: refused by name when it would not fit one KV write,
    /// read back afterwards.
    fn append_close(&self, close: Close) -> Result<Close, CommitmentRefusal> {
        let requirement = close.requirement.as_str();
        let encoded = encode(&close);
        // The log key the append writes: `log/{stream}/{hlc:016x}/{node}`.
        let key_len = format!("log/{}/", closes_stream(requirement)).len() + 17 + self.agent.node_id().to_string().len();
        if key_len + encoded.len() > MAX_CLOSE_WRITE_BYTES {
            return Err(CommitmentRefusal::CloseTooLarge { bytes: key_len + encoded.len(), max: MAX_CLOSE_WRITE_BYTES });
        }
        let stream = closes_stream(requirement);
        let hlc = self.agent.kv().append(&stream, encoded);
        let written = self
            .agent
            .kv()
            .scan_log(&stream, hlc, hlc.saturating_add(1))
            .iter()
            .any(|e| decode::<Close>(&e.value).as_ref() == Some(&close));
        if !written {
            return Err(CommitmentRefusal::CloseNotWritten);
        }
        Ok(close)
    }

    /// Every close on the requirement's close stream, with its log position, in log order — unchecked.
    pub fn closes(&self, requirement: &str) -> Vec<(u64, Close)> {
        self.agent
            .kv()
            .scan_log(&closes_stream(requirement), 0, u64::MAX)
            .into_iter()
            .filter_map(|e| decode::<Close>(&e.value).map(|c| (e.hlc, c)))
            .filter(|(_, c)| c.requirement == requirement)
            .collect()
    }

    /// The earliest close of `announcement` (by its declarer, at or after its deadline, binding it) —
    /// and, with `key`, verifying under it. **Earliest by the close's own `closed_at_ms`**, tie-broken by
    /// its digest — never by stream key, which any member writes (#609's review, round 3): copying a
    /// genuine close to a backdated key changes nothing. A declarer that genuinely signs two different
    /// closes has equivocated; the earlier-dated one counts, and both are on the stream for anyone to see.
    fn select_close(&self, announcement: &Announcement, key: Option<&[u8; 32]>) -> Option<Close> {
        self.closes(&announcement.requirement)
            .into_iter()
            .map(|(_, c)| c)
            .filter(|c| c.closes(announcement) && key.is_none_or(|k| verify_close(c, k)))
            .min_by(|a, b| a.closed_at_ms.cmp(&b.closed_at_ms).then_with(|| a.sha256().cmp(&b.sha256())))
    }

    /// The close the **unverified** path counts: the earliest (by `closed_at_ms`, then digest) that closes
    /// the current announcement — names its declarer, its deadline and its digest, and is dated at or
    /// after the deadline. Any member can append an unsigned one that passes this read and steers the
    /// award (see the crate doc).
    pub fn close_of(&self, requirement: &str) -> Option<Close> {
        let announcement = self.announcement(requirement)?;
        self.select_close(&announcement, None)
    }

    /// The close the **checked** path counts: the earliest that closes the current announcement and
    /// verifies under the key `resolve` gives for `declarer`, the declarer the caller pins. A forged or
    /// overwritten entry is passed over; it cannot pre-empt the declarer's own signed close.
    pub fn close_of_verified(
        &self,
        requirement: &str,
        declarer: &str,
        resolve: impl Fn(&str) -> Option<[u8; 32]>,
    ) -> Option<Close> {
        let announcement = self.announcement(requirement)?;
        if announcement.declarer != declarer {
            return None;
        }
        let key = resolve(declarer)?;
        self.select_close(&announcement, Some(&key))
    }

    /// The offers whose signature verifies under the key `resolve` gives for the participant they
    /// name — **the candidate set a declarer that cares about provenance should award from**.
    ///
    /// An unsigned offer is not a candidate here, and neither is one signed by anybody but the
    /// participant it names. `resolve` returning `None` for a participant drops that participant's
    /// offers: a key we cannot find is not a key we may assume.
    ///
    /// Where the key comes from is deliberately the caller's business, not this crate's — the
    /// substrate publishes node keys at `sys/identity/{node}`, but a participant here may be a
    /// principal, a driver or a service that is not a node at all, and minting identity is not
    /// something a coordination companion should start doing.
    pub fn offers_verified(
        &self,
        requirement: &str,
        resolve: impl Fn(&str) -> Option<[u8; 32]>,
    ) -> Vec<(u64, Offer)> {
        self.offers(requirement)
            .into_iter()
            .filter(|(_, o)| resolve(&o.participant).is_some_and(|k| verify_offer(o, &k)))
            .collect()
    }

    /// **Plan** an award: read the head, any existing award and the offers made by the deadline,
    /// and choose by `rule` — **no write**. [`award`](Self::award) is this followed by
    /// [`commit_award`](Self::commit_award); they are separate so a harness can interleave two
    /// declarers' steps and show what the plain path cannot promise (CN2's witness).
    pub fn plan_award(&self, requirement: &str, rule: AwardRule, now_ms: u64) -> Result<Award, CommitmentRefusal> {
        let offers = self.offers(requirement);
        self.plan_award_from(requirement, rule, now_ms, offers)
    }

    /// **Plan an award from a candidate set you chose** — the same rule, over offers the caller has
    /// already filtered. This is the seam that makes provenance the declarer's decision rather than
    /// this crate's policy: pass [`offers_verified`](Self::offers_verified) and a forged offer is
    /// not a candidate; pass [`offers`](Self::offers) and you get today's behaviour, unchanged.
    ///
    /// The close and the already-awarded check still apply — they are the requirement's, not the
    /// candidate set's: only offers the declarer's [`Close`] names count, and there is no award before a
    /// close ([`CommitmentRefusal::NotClosed`]).
    ///
    /// **Unverified, stated plainly.** This path reads the close ([`close_of`](Self::close_of): the
    /// earliest naming the announcement's declarer, deadline and digest, dated at or after the deadline)
    /// and any existing award without a signature check, so **any member able to write
    /// `cn/{requirement}/award` or append to `cn/{requirement}/closes` can block or steer an award here** —
    /// a forged award is `AlreadyAwarded` for good. A declarer that holds keys uses [`plan_award_verified`](Self::plan_award_verified).
    pub fn plan_award_from(
        &self,
        requirement: &str,
        rule: AwardRule,
        now_ms: u64,
        candidates: Vec<(u64, Offer)>,
    ) -> Result<Award, CommitmentRefusal> {
        self.announcement(requirement).ok_or(CommitmentRefusal::NotAnnounced)?;
        if let Some(existing) = self.award_of(requirement) {
            return Err(CommitmentRefusal::AlreadyAwarded(Box::new(existing)));
        }
        let close = self.close_of(requirement).ok_or(CommitmentRefusal::NotClosed)?;
        self.choose_award(requirement, rule, now_ms, candidates, &close)
    }

    /// **Plan an award on the checked path**: the close must be signed by `declarer` — the declarer
    /// *the caller* pins, not the one the writable head names — the candidates are the offers whose
    /// signature verifies ([`offers_verified`](Self::offers_verified)) and that the close names, and
    /// only an award `declarer` signed ([`award_of_verified`](Self::award_of_verified)) counts as
    /// already made. An unsigned or foreign record at the award slot does not block it here (the plain
    /// KV commit then overwrites it; a linearizable slot a forgery holds stays taken and
    /// [`commit_award_linearizable_verified`](Self::commit_award_linearizable_verified) says so).
    pub fn plan_award_verified(
        &self,
        requirement: &str,
        rule: AwardRule,
        now_ms: u64,
        declarer: &str,
        resolve: impl Fn(&str) -> Option<[u8; 32]>,
    ) -> Result<Award, CommitmentRefusal> {
        self.announcement(requirement).ok_or(CommitmentRefusal::NotAnnounced)?;
        if let Some(existing) = self.award_of_verified(requirement, declarer, &resolve) {
            return Err(CommitmentRefusal::AlreadyAwarded(Box::new(existing)));
        }
        let Some(close) = self.close_of_verified(requirement, declarer, &resolve) else {
            // Some close exists that the checked path cannot accept, or none does.
            return Err(if self.closes(requirement).is_empty() { CommitmentRefusal::NotClosed } else { CommitmentRefusal::CloseUnproven });
        };
        let candidates = self.offers_verified(requirement, &resolve);
        self.choose_award(requirement, rule, now_ms, candidates, &close)
    }

    /// The rule over the candidates the close names.
    fn choose_award(
        &self,
        requirement: &str,
        rule: AwardRule,
        now_ms: u64,
        candidates: Vec<(u64, Offer)>,
        close: &Close,
    ) -> Result<Award, CommitmentRefusal> {
        let eligible: Vec<(u64, Offer)> = candidates
            .into_iter()
            .filter(|(hlc, o)| o.requirement == requirement && close.names(*hlc, o))
            .collect();
        let (offer_hlc, offer) = rule.choose(&eligible).cloned().ok_or(CommitmentRefusal::NoOffers)?;
        Ok(Award {
            requirement: requirement.to_string(),
            participant: offer.participant,
            declarer: self.me.clone(),
            offer_hlc,
            awarded_at_ms: now_ms,
            operation_id: award_key(requirement),
            close_sha256: close.sha256(),
            signature: Vec::new(),
        })
    }

    /// **Commit** a planned award with a receipt under `operation_id = cn/{requirement}/award` —
    /// the plain KV path, for **one declarer per requirement**. Between a plan and its commit
    /// another declarer may have committed; two racing declarers both commit and LWW keeps one
    /// (CN2's sweep shows it). Where two may race, use
    /// [`commit_award_linearizable`](Self::commit_award_linearizable).
    pub async fn commit_award(&self, award: Award) -> Result<Awarded, CommitmentRefusal> {
        let receipt = self
            .agent
            .kv()
            .set_with_receipt(&OperationId::new(award.operation_id.clone()), award_key(&award.requirement).as_str(), encode(&award))
            .await
            .map_err(CommitmentRefusal::Receipt)?;
        Ok(Awarded { award, receipt })
    }

    /// **Award** the requirement by `rule` over the offers made by its deadline, writing the award
    /// **with a receipt** under `operation_id = cn/{requirement}/award`. Refused — not overwritten —
    /// when an award already exists; refused visibly when nobody offered. Plan, then commit.
    pub async fn award(&self, requirement: &str, rule: AwardRule, now_ms: u64) -> Result<Awarded, CommitmentRefusal> {
        let award = self.plan_award(requirement, rule, now_ms)?;
        self.commit_award(award).await
    }

    /// **Commit a planned award linearizably** (CN2): through a consensus round on the slot
    /// `cn/{requirement}/award`, so that of two declarers racing exactly one commits and the other
    /// is `AlreadyAwarded` **with the committed award** — the plan's *"a `group_propose` round where
    /// the award must be linearizable"*. A round with no commit before its deadline is
    /// [`CommitmentRefusal::AwardUnknown`]: the award may have committed elsewhere, and a retry
    /// resolves it. The commit receipt says what the cluster agreed and, separately, whether this
    /// node has it on disk.
    ///
    /// The committed record is reported **as what it is**: this requirement's award is `AlreadyAwarded`
    /// (unverified — any member that commits a forgery first holds the slot; see
    /// [`commit_award_linearizable_verified`](Self::commit_award_linearizable_verified)), a record for
    /// another requirement or operation is [`CommitmentRefusal::SlotHoldsAnotherRecord`], and only bytes
    /// that are not an award at all are `Encoding`.
    pub async fn commit_award_linearizable(&self, award: Award) -> Result<AwardedLinearizable, CommitmentRefusal> {
        self.commit_linearizable_inner(award, None).await
    }

    /// [`commit_award_linearizable`](Self::commit_award_linearizable) on the checked path: a committed
    /// award counts as `AlreadyAwarded` only when `declarer` signed it; one nobody trusted signed is
    /// [`CommitmentRefusal::UnprovenAwardHolds`] — the slot is taken, and this says so rather than
    /// reporting it as the award.
    pub async fn commit_award_linearizable_verified(
        &self,
        award: Award,
        declarer: &str,
        resolve: impl Fn(&str) -> Option<[u8; 32]>,
    ) -> Result<AwardedLinearizable, CommitmentRefusal> {
        // The declarer is pinned, so its key is resolved once, before the round — nothing to hold across it.
        let key = resolve(declarer);
        self.commit_linearizable_inner(award, Some((declarer, key))).await
    }

    async fn commit_linearizable_inner(
        &self,
        award: Award,
        trusted: Option<(&str, Option<[u8; 32]>)>,
    ) -> Result<AwardedLinearizable, CommitmentRefusal> {
        let slot = award_key(&award.requirement);
        // An award is an exclusive outcome (P2, #601's review): marked safety-sensitive and decided where the
        // fleet decides those — its exclusive-default electorate group, or the whole cluster when none is declared.
        match self.agent.consensus().exclusive_propose_receipt(&slot, encode(&award), ConsensusConfig::default()).await {
            Ok(commit) => Ok(AwardedLinearizable { award, commit }),
            Err(CommitError::Superseded { .. }) => {
                let committed: Award = self
                    .agent
                    .consensus()
                    .consensus_get(&slot)
                    .and_then(|b| decode(&b))
                    .ok_or_else(|| CommitmentRefusal::Encoding("superseded, and the committed record is not an award".into()))?;
                if committed.requirement != award.requirement || committed.operation_id != slot {
                    return Err(CommitmentRefusal::SlotHoldsAnotherRecord {
                        requirement: committed.requirement,
                        operation_id: committed.operation_id,
                    });
                }
                match trusted {
                    Some((declarer, key))
                        if committed.declarer != declarer || !key.is_some_and(|k| verify_award(&committed, &k)) =>
                    {
                        Err(CommitmentRefusal::UnprovenAwardHolds(Box::new(committed)))
                    }
                    _ => Err(CommitmentRefusal::AlreadyAwarded(Box::new(committed))),
                }
            }
            Err(CommitError::DeliveryUnknown { ballots_tried, .. }) => Err(CommitmentRefusal::AwardUnknown { ballots_tried }),
            Err(other) => Err(CommitmentRefusal::NotCommitted(other.to_string())),
        }
    }

    /// [`plan_award`](Self::plan_award) then [`commit_award_linearizable`](Self::commit_award_linearizable).
    pub async fn award_linearizable(
        &self,
        requirement: &str,
        rule: AwardRule,
        now_ms: u64,
    ) -> Result<AwardedLinearizable, CommitmentRefusal> {
        let award = self.plan_award(requirement, rule, now_ms)?;
        self.commit_award_linearizable(award).await
    }

    /// **Commit a planned award under the acceptor's mandate** (CN3): the mandate must be the
    /// acceptor's own, and `authority` — the requirement's scope, at its installed epoch — must
    /// authorize the operation `accept` for it *now*. A stale holder's award, minted under an epoch
    /// the resource has moved past, is [`CommitmentRefusal::Mandate`] with
    /// `MandateRefusal::Superseded { installed, presented }`. The check runs **before any write**;
    /// a passing check commits linearizably. Where a deployment has no mandates, use
    /// [`commit_award_linearizable`](Self::commit_award_linearizable) — this method is not a
    /// mandate check that can be skipped, it is one that is either made or not called.
    pub async fn commit_award_under_mandate(
        &self,
        award: Award,
        acceptor: &Mandate,
        authority: &ResourceAuthority,
        now_ms: u64,
    ) -> Result<AwardedLinearizable, CommitmentRefusal> {
        if acceptor.holder.as_str() != award.participant {
            return Err(CommitmentRefusal::MandateNotTheAcceptors {
                holder: acceptor.holder.as_str().to_string(),
                acceptor: award.participant,
            });
        }
        authority.check(acceptor, "accept", now_ms).map_err(CommitmentRefusal::Mandate)?;
        self.commit_award_linearizable(award).await
    }

    /// The award, if the requirement has one: a linearizable award (the consensus slot) first, then
    /// a plain one (the KV head).
    ///
    /// **What it checks, and what it cannot.** A record is reported only when it names *this*
    /// requirement and the operation `cn/{requirement}/award` — a record copied from another
    /// requirement's slot (any member can write the head) is not this requirement's award and is not
    /// reported, and does not block an award as `AlreadyAwarded`. Whether the declarer it names made
    /// it needs the declarer's key, which this crate does not hold: that is
    /// [`award_of_verified`](Self::award_of_verified), and an award read here is **unproven** in the
    /// crate's usual sense — a claim, until a signature under the declarer the caller trusts says
    /// otherwise. Any member can write a record that passes this read.
    pub fn award_of(&self, requirement: &str) -> Option<Award> {
        let slot = award_key(requirement);
        let award: Award = self
            .agent
            .consensus()
            .consensus_get(&slot)
            .and_then(|b| decode(&b))
            .or_else(|| self.agent.kv().get(&slot).and_then(|b| decode(&b)))?;
        (award.requirement == requirement && award.operation_id == slot).then_some(award)
    }

    /// The award, if the requirement has one **and `declarer` signed it** — the read a participant
    /// that cares about provenance should make before acting on an acceptance.
    ///
    /// `declarer` is **pinned by the caller** — whom the caller knows to have announced the
    /// requirement — never read from the announcement head, which is unsigned and writable: a check
    /// against the head fell in two writes (rewrite the head's declarer, then sign the award with that
    /// key; #609's review, finding 4). Reported only when [`award_of`](Self::award_of)'s checks pass,
    /// the award names `declarer`, and its signature verifies under the key `resolve` gives for
    /// `declarer`. `resolve` returning `None` drops the award: a key we cannot find is not a key we may
    /// assume. The `require_identity_proofs` caveat every signature in this crate carries applies.
    pub fn award_of_verified(
        &self,
        requirement: &str,
        declarer: &str,
        resolve: impl Fn(&str) -> Option<[u8; 32]>,
    ) -> Option<Award> {
        let award = self.award_of(requirement)?;
        (award.declarer == declarer && resolve(declarer).is_some_and(|k| verify_award(&award, &k))).then_some(award)
    }

    /// **Report** against an award's operation: what became of the work, or that it is unknown.
    pub fn report(&self, requirement: &str, outcome: Outcome, operation_id: impl Into<String>, now_ms: u64) -> u64 {
        let r = Report {
            requirement: requirement.to_string(),
            participant: self.me.clone(),
            outcome,
            operation_id: operation_id.into(),
            reported_at_ms: now_ms,
        };
        self.agent.kv().append(&reports_stream(requirement), encode(&r))
    }

    /// Every report on the requirement, in log order — only records that name this requirement.
    pub fn reports(&self, requirement: &str) -> Vec<Report> {
        self.agent
            .kv()
            .scan_log(&reports_stream(requirement), 0, u64::MAX)
            .into_iter()
            .filter_map(|e| decode::<Report>(&e.value))
            .filter(|r| r.requirement == requirement)
            .collect()
    }

    /// **Assess** whether the requirement was satisfied — by this participant, whoever it is.
    /// Signed with `key` when one is given; unsigned means unproven.
    pub fn assess(
        &self,
        requirement: &str,
        satisfied: bool,
        note: impl Into<String>,
        key: Option<&ed25519_dalek::SigningKey>,
        now_ms: u64,
    ) -> u64 {
        use ed25519_dalek::Signer;
        let mut a = Assessment {
            requirement: requirement.to_string(),
            assessor: self.me.clone(),
            satisfied,
            note: note.into(),
            assessed_at_ms: now_ms,
            signature: Vec::new(),
        };
        if let Some(k) = key {
            a.signature = k.sign(&a.canonical_bytes()).to_bytes().to_vec();
        }
        self.agent.kv().append(&assessments_stream(requirement), encode(&a))
    }

    /// Every assessment on the requirement, in log order — only records that name this requirement.
    pub fn assessments(&self, requirement: &str) -> Vec<Assessment> {
        self.agent
            .kv()
            .scan_log(&assessments_stream(requirement), 0, u64::MAX)
            .into_iter()
            .filter_map(|e| decode::<Assessment>(&e.value))
            .filter(|a| a.requirement == requirement)
            .collect()
    }
}

fn encode<T: Serialize>(t: &T) -> Bytes {
    Bytes::from(serde_json::to_vec(t).unwrap_or_default())
}
fn decode<T: for<'de> Deserialize<'de>>(b: &[u8]) -> Option<T> {
    serde_json::from_slice(b).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycelium::{GossipConfig, NodeId};

    /// A fixed deadline (2096 in ms) the tests close at — a constant, so the sim-replay test's recording
    /// and replay announce and close with the same bytes.
    const FAR_DEADLINE: u64 = 4_000_000_000_000;

    fn offer(participant: &str) -> Offer {
        Offer { requirement: "r".into(), participant: participant.into(), bid: "".into(), offered_at_ms: 0, signature: Vec::new() }
    }

    #[test]
    fn the_rule_awards_the_lowest_participant_and_the_earlier_offer_on_a_tie() {
        let offers = vec![(30, offer("driver-c")), (10, offer("driver-a")), (20, offer("driver-a"))];
        let (hlc, o) = AwardRule::LowestParticipant.choose(&offers).unwrap();
        assert_eq!((*hlc, o.participant.as_str()), (10, "driver-a"));
        assert!(AwardRule::LowestParticipant.choose(&[]).is_none(), "nothing to award is None, not a panic");
    }

    async fn live() -> Arc<GossipAgent> {
        for _ in 0..16 {
            let port = mycelium::test_util::alloc_port();
            let agent = Arc::new(GossipAgent::new(
                NodeId::new("127.0.0.1", port).unwrap(),
                GossipConfig { bind_port: port, ..Default::default() },
            ));
            if agent.start().await.is_ok() {
                return agent;
            }
        }
        panic!("could not bind a gossip port");
    }

    #[tokio::test]
    async fn one_award_per_requirement_with_a_receipt_and_a_second_is_refused_not_overwritten() {
        let agent = live().await;
        let hub = ContractNet::new(Arc::clone(&agent), "hub");
        let a = ContractNet::new(Arc::clone(&agent), "driver-b");
        let b = ContractNet::new(Arc::clone(&agent), "driver-a");

        assert!(a.offer("pickup-1", "5 km", 1).is_none(), "an offer needs an announcement to offer against");
        hub.announce("pickup-1", "2 crates, 14:00", "delivered before 15:00", FAR_DEADLINE, 0);
        assert!(a.offer("pickup-1", "5 km", 1).is_some());
        assert!(b.offer("pickup-1", "9 km", 2).is_some());
        hub.close("pickup-1", None, FAR_DEADLINE).expect("the declarer closes");

        let awarded = hub.award("pickup-1", AwardRule::LowestParticipant, 50).await.expect("awarded");
        assert_eq!(awarded.award.participant, "driver-a", "the lowest participant, not the first offer");
        assert_eq!(awarded.receipt.operation_id.as_str(), "cn/pickup-1/award");
        assert_eq!(awarded.receipt.application, mycelium::LocalApplication::Applied);
        assert_eq!(hub.award_of("pickup-1").as_ref().map(|x| x.participant.as_str()), Some("driver-a"));

        match hub.award("pickup-1", AwardRule::LowestParticipant, 60).await {
            Err(CommitmentRefusal::AlreadyAwarded(existing)) => assert_eq!(existing.participant, "driver-a"),
            other => panic!("a second award must be refused, got {other:?}"),
        }
        assert_eq!(hub.award_of("pickup-1").unwrap().awarded_at_ms, 50, "the first award stands untouched");
        agent.shutdown().await;
    }

    #[tokio::test]
    async fn no_offers_and_late_offers_are_visible_states_not_awards() {
        let agent = live().await;
        let hub = ContractNet::new(Arc::clone(&agent), "hub");
        let d = ContractNet::new(Arc::clone(&agent), "driver-a");
        assert!(matches!(hub.award("nothing", AwardRule::LowestParticipant, 0).await, Err(CommitmentRefusal::NotAnnounced)));
        hub.announce("pickup-2", "1 crate", "any time today", 100, 0);
        assert!(matches!(hub.award("pickup-2", AwardRule::LowestParticipant, 10).await, Err(CommitmentRefusal::NotClosed)));
        assert!(matches!(hub.close("pickup-2", None, 10), Err(CommitmentRefusal::BeforeDeadline { .. })), "not before the deadline");
        assert!(matches!(d.close("pickup-2", None, 100), Err(CommitmentRefusal::NotTheDeclarer { .. })), "only the declarer closes");
        hub.close("pickup-2", None, 100).expect("closed at the deadline, naming nothing");
        assert!(matches!(hub.award("pickup-2", AwardRule::LowestParticipant, 100).await, Err(CommitmentRefusal::NoOffers)));
        d.offer("pickup-2", "late", 50); // appended after the close, with an early stamp
        assert!(matches!(hub.award("pickup-2", AwardRule::LowestParticipant, 200).await, Err(CommitmentRefusal::NoOffers)),
            "an offer after the close is not considered, whatever its stamp");
        assert!(matches!(hub.close("pickup-2", None, 300), Err(CommitmentRefusal::AlreadyClosed(_))), "closed once");
        assert_eq!(hub.offers("pickup-2").len(), 1, "but it is on the record");
        agent.shutdown().await;
    }

    /// CN2's witness and its rule, as an **explicit interleaving** — the kernel has no scheduler
    /// seam yet, so the race is written out rather than scheduled. Plain path: A plans, B plans,
    /// A commits, B commits — both `Ok`, and the head holds B's: a double award, kept by LWW. The
    /// linearizable path, same order: A commits, B is `AlreadyAwarded` with A's award. Both
    /// declarers are on one node, so consensus is local: this shows the mechanism, not a
    /// multi-node race.
    #[tokio::test]
    async fn two_declarers_racing_the_plain_award_both_win_but_the_linearizable_award_refuses_the_second() {
        let agent = live().await;
        let hub_a = ContractNet::new(Arc::clone(&agent), "hub-a");
        let hub_b = ContractNet::new(Arc::clone(&agent), "hub-b");
        let d = ContractNet::new(Arc::clone(&agent), "driver-a");

        hub_a.announce("plain", "1 crate", "any", FAR_DEADLINE, 0);
        d.offer("plain", "3 km", 1);
        hub_a.close("plain", None, FAR_DEADLINE).unwrap();
        let plan_a = hub_a.plan_award("plain", AwardRule::LowestParticipant, 10).unwrap();
        let plan_b = hub_b.plan_award("plain", AwardRule::LowestParticipant, 11).unwrap();
        assert!(hub_a.commit_award(plan_a).await.is_ok());
        assert!(hub_b.commit_award(plan_b).await.is_ok(), "the plain path lets the second commit too");
        assert_eq!(hub_a.award_of("plain").unwrap().declarer, "hub-b", "LWW kept the later write: a double award, silently");

        hub_a.announce("linear", "1 crate", "any", FAR_DEADLINE, 0);
        d.offer("linear", "3 km", 1);
        hub_a.close("linear", None, FAR_DEADLINE).unwrap();
        let plan_a = hub_a.plan_award("linear", AwardRule::LowestParticipant, 10).unwrap();
        let plan_b = hub_b.plan_award("linear", AwardRule::LowestParticipant, 11).unwrap();
        let first = hub_a.commit_award_linearizable(plan_a).await.expect("the first round commits");
        assert_eq!(first.award.declarer, "hub-a");
        match hub_b.commit_award_linearizable(plan_b).await {
            Err(CommitmentRefusal::AlreadyAwarded(existing)) => assert_eq!(existing.declarer, "hub-a"),
            other => panic!("the second declarer must be refused with the committed award, got {other:?}"),
        }
        assert_eq!(hub_b.award_of("linear").unwrap().declarer, "hub-a", "and everyone reads the one award");
        agent.shutdown().await;
    }

    /// CN2's replay half, as far as it goes today — **a pin on a gap, not a claim**. A linearizable
    /// award recorded under the kernel on a whole node and replayed on the same identity
    /// **diverges**, and the divergence is the one the inventory predicted: at seq 7 the recording
    /// has the membership governor's `rng jitter` draw and the replay has the award round's
    /// `consensus/defer` timer — two tasks whose interleaving belongs to the scheduler, and no seam
    /// owns it (`replay-nondeterminism-inventory.md` §2.3, the unrouted `select!` row). The award's
    /// own effects replay; the node's do not. This test asserts the divergence *happens* and names
    /// it, so the day the scheduler seam lands it fails and is flipped into the claim.
    ///
    /// (A first attempt replayed on a node with a fresh port and diverged earlier, at a gossip
    /// `try_send`: the shard a key maps to hashes the key, and the key carries the node id. Not
    /// nondeterminism — a different node. Both runs now share one identity.)
    #[tokio::test]
    async fn a_whole_node_recording_of_a_linearizable_award_replays_under_the_scheduler_seam() {
        use mycelium::sim_seam::{install, take, SimContext};
        use mycelium_sim::{Kernel, Sources, Trace};

        async fn run(kernel: Kernel, port: u16, paused: bool) -> (Trace, Award) {
            install(SimContext {
                kernel,
                sources: Sources::seeded(7, 1_789_000_000_000),
                node: "n1".into(),
                offsets: Default::default(),
            });
            if paused {
                mycelium::sim_seam::pause_clock_for_replay();
            }
            let agent = Arc::new(GossipAgent::new(
                NodeId::new("127.0.0.1", port).unwrap(),
                GossipConfig { bind_port: port, ..Default::default() },
            ));
            agent.start().await.expect("the node binds its port");
            let hub = ContractNet::new(Arc::clone(&agent), "hub");
            let d = ContractNet::new(Arc::clone(&agent), "driver-a");
            hub.announce("replayed", "1 crate", "any", FAR_DEADLINE, 0);
            d.offer("replayed", "2 km", 1);
            hub.close("replayed", None, FAR_DEADLINE).expect("closes");
            let awarded = hub.award_linearizable("replayed", AwardRule::LowestParticipant, 10).await.expect("awarded");
            agent.shutdown().await;
            let ctx = take().expect("the kernel was installed on this thread");
            (ctx.kernel.trace().clone(), awarded.award)
        }

        let port = mycelium::test_util::alloc_port();
        let (trace, recorded) = run(Kernel::recording(), port, false).await;
        assert!(trace.len() > 6, "the round touched the seams: {} entries", trace.len());
        assert_eq!(recorded.participant, "driver-a");

        // The claim. The replay runs on the same thread (current-thread runtime), so the installed
        // kernel is the one it sees; a divergence panics inside `run`, which the join reports.
        let armed = tokio::spawn(run(Kernel::replaying(trace.clone()), port, true)).await;
        take();
        mycelium::sim_seam::resume_clock_after_replay();
        let (_, replayed) = armed.expect("the whole node's tasks keep their recorded order under the scheduler seam");
        assert_eq!(replayed.participant, recorded.participant, "and the award replays to the same acceptor");

        // The plant: the identical trace, the identical node, the arm disarmed. It must still
        // diverge — otherwise the claim above is about luck rather than about the seam.
        let unarmed = tokio::spawn(run(Kernel::replaying(trace), port, false)).await;
        take();
        let err = unarmed.expect_err("without the paused clock a whole-node replay still diverges");
        let msg = err.into_panic().downcast_ref::<String>().cloned().unwrap_or_default();
        assert!(msg.contains("replay diverged"), "the failure is a divergence, not something else: {msg}");
    }

    fn mandate_for(holder: &str, epoch: u64) -> Mandate {
        use mycelium::mandate::{PrincipalId, TermId};
        Mandate {
            holder: PrincipalId::new(holder).unwrap(),
            established_by: PrincipalId::new("coop-board").unwrap(),
            purpose: "drive pickups".into(),
            scope: "redistribution/pickups".into(),
            operations: vec!["accept".into()],
            epoch,
            term: TermId::new(format!("term-{epoch}")).unwrap(),
            valid_from_ms: 0,
            valid_until_ms: u64::MAX,
        }
    }

    /// CN3 — the negative case: the acceptor's mandate is checked against the resource's installed
    /// epoch **before** the award is written. A holder whose mandate was minted under epoch 1 is
    /// refused once the resource has installed epoch 2 — `Superseded { installed: 2, presented: 1 }`
    /// — and nothing was written; a mandate that is not the acceptor's own is refused by name; a
    /// current mandate's award commits, linearizably.
    #[tokio::test]
    async fn a_stale_holders_award_is_refused_as_superseded_before_any_write() {
        let agent = live().await;
        let hub = ContractNet::new(Arc::clone(&agent), "hub");
        let d = ContractNet::new(Arc::clone(&agent), "driver-a");
        hub.announce("mandated", "2 crates", "before close", FAR_DEADLINE, 0);
        d.offer("mandated", "4 km", 1);
        hub.close("mandated", None, FAR_DEADLINE).unwrap();
        let authority = ResourceAuthority::new("redistribution/pickups", 2);

        let stale = mandate_for("driver-a", 1);
        let plan = hub.plan_award("mandated", AwardRule::LowestParticipant, 10).unwrap();
        match hub.commit_award_under_mandate(plan.clone(), &stale, &authority, 10).await {
            Err(CommitmentRefusal::Mandate(MandateRefusal::Superseded { installed, presented })) => {
                assert_eq!((installed, presented), (2, 1));
            }
            other => panic!("a stale holder must be refused as superseded, got {other:?}"),
        }
        assert!(hub.award_of("mandated").is_none(), "refused before any write");

        let someone_elses = mandate_for("driver-b", 2);
        match hub.commit_award_under_mandate(plan.clone(), &someone_elses, &authority, 10).await {
            Err(CommitmentRefusal::MandateNotTheAcceptors { holder, acceptor }) => {
                assert_eq!((holder.as_str(), acceptor.as_str()), ("driver-b", "driver-a"));
            }
            other => panic!("another holder's mandate must be refused by name, got {other:?}"),
        }
        assert!(hub.award_of("mandated").is_none(), "still nothing written");

        let current = mandate_for("driver-a", 2);
        let awarded = hub.commit_award_under_mandate(plan, &current, &authority, 10).await.expect("a current mandate commits");
        assert_eq!(awarded.award.participant, "driver-a");
        assert_eq!(hub.award_of("mandated").unwrap().participant, "driver-a");
        agent.shutdown().await;
    }

    #[tokio::test]
    async fn reports_and_assessments_round_trip_and_an_unsigned_assessment_never_verifies() {
        use ed25519_dalek::SigningKey;
        let agent = live().await;
        let hub = ContractNet::new(Arc::clone(&agent), "hub");
        let d = ContractNet::new(Arc::clone(&agent), "driver-a");
        let auditor = ContractNet::new(Arc::clone(&agent), "food-bank-auditor");
        hub.announce("pickup-3", "3 crates", "signed off by the receiving pantry", FAR_DEADLINE, 0);
        d.offer("pickup-3", "2 km", 1);
        hub.close("pickup-3", None, FAR_DEADLINE).unwrap();
        let awarded = hub.award("pickup-3", AwardRule::LowestParticipant, 10).await.unwrap();

        d.report("pickup-3", Outcome::Fulfilled, awarded.award.operation_id.clone(), 20);
        d.report("pickup-3", Outcome::Unknown, awarded.award.operation_id.clone(), 21);
        let reports = hub.reports("pickup-3");
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].outcome, Outcome::Fulfilled);
        assert_eq!(reports[1].outcome, Outcome::Unknown, "unknown is a report, not a silence");
        assert!(reports.iter().all(|r| r.operation_id == "cn/pickup-3/award"));

        let key = SigningKey::from_bytes(&[3u8; 32]);
        auditor.assess("pickup-3", true, "pantry signed", Some(&key), 30);
        hub.assess("pickup-3", true, "declarer's own view", None, 31);
        let assessments = hub.assessments("pickup-3");
        assert_eq!(assessments.len(), 2);
        assert_eq!(assessments[0].assessor, "food-bank-auditor", "the declarer is not the only permitted assessor");
        assert!(verify_assessment(&assessments[0], &key.verifying_key().to_bytes()));
        assert!(!verify_assessment(&assessments[0], &[9u8; 32]), "and not under another key");
        assert!(!assessments[1].is_signed());
        assert!(!verify_assessment(&assessments[1], &key.verifying_key().to_bytes()), "unsigned never verifies");
        agent.shutdown().await;
    }

    /// **The crate's first rule, with a mechanism under it.**
    ///
    /// *"No component assigns another participant's obligation"* was, until now, a rule with
    /// nothing enforcing it: an offer names its participant in a field, and any member able to
    /// append to the stream could name somebody else. `LowestParticipant` would then dutifully
    /// award that participant work they never offered to do — and every reader of the streams,
    /// checking the award against the offers as the design intends, would agree it was correct.
    ///
    /// The forgery here is the realistic one. `attacker` does not merely *claim* to be `driver-a`:
    /// it signs its forged offer with **its own key**, which is the best a forger can do. It fails
    /// because the key the declarer checks against is the one belonging to the participant the
    /// offer *names*.
    #[tokio::test]
    async fn a_forged_offer_is_not_a_candidate_when_the_declarer_checks_provenance() {
        use ed25519_dalek::SigningKey;
        let agent = live().await;
        let hub = ContractNet::new(Arc::clone(&agent), "hub");
        let driver_a = ContractNet::new(Arc::clone(&agent), "driver-a");
        let driver_b = ContractNet::new(Arc::clone(&agent), "driver-b");
        // The attacker acts under its own name in the mesh and lies in the record.
        let attacker = ContractNet::new(Arc::clone(&agent), "attacker");

        let key_a = SigningKey::from_bytes(&[11u8; 32]);
        let key_b = SigningKey::from_bytes(&[12u8; 32]);
        let key_attacker = SigningKey::from_bytes(&[13u8; 32]);
        let directory = move |participant: &str| -> Option<[u8; 32]> {
            match participant {
                "driver-a" => Some(key_a.verifying_key().to_bytes()),
                "driver-b" => Some(key_b.verifying_key().to_bytes()),
                _ => None,
            }
        };

        hub.announce("pickup-9", "2 crates", "pantry signs off", FAR_DEADLINE, 0);
        driver_b.offer_signed("pickup-9", "5 km", Some(&SigningKey::from_bytes(&[12u8; 32])), 1);
        // The forgery: an offer NAMING driver-a, signed by the attacker's own key. `driver-a`
        // sorts lowest, so the deterministic rule would award it.
        let forged = Offer {
            requirement: "pickup-9".into(),
            participant: "driver-a".into(),
            bid: "0 km".into(),
            offered_at_ms: 2,
            signature: Vec::new(),
        };
        let forged = {
            use ed25519_dalek::Signer;
            let sig = key_attacker.sign(&forged.canonical_bytes()).to_bytes().to_vec();
            Offer { signature: sig, ..forged }
        };
        agent.kv().append(&offers_stream("pickup-9"), encode(&forged));
        let _ = &attacker; // the attacker's own handle is not needed to write the lie

        // 1. Unchecked, the forgery wins — which is the defect, stated as a test rather than a
        //    worry. This is today's behaviour for a declarer that does not check.
        hub.close("pickup-9", None, FAR_DEADLINE).unwrap();
        let unchecked = hub.plan_award("pickup-9", AwardRule::LowestParticipant, 10).expect("plans");
        assert_eq!(unchecked.participant, "driver-a", "the unchecked path awards the forged offer");

        // 2. Checked, it is not a candidate at all, and the award goes to the real offer.
        let verified = hub.offers_verified("pickup-9", &directory);
        assert_eq!(verified.len(), 1, "only driver-b's own signed offer survives");
        assert_eq!(verified[0].1.participant, "driver-b");
        let checked = hub
            .plan_award_from("pickup-9", AwardRule::LowestParticipant, 10, verified)
            .expect("plans from the verified set");
        assert_eq!(checked.participant, "driver-b", "no component assigns another participant's obligation");

        // 3. An UNSIGNED offer is not a candidate either — unproven is not the same as trusted,
        //    and a forger who simply omits the signature must not do better than one who tries.
        driver_a.offer("pickup-9", "1 km", 3);
        assert_eq!(
            hub.offers_verified("pickup-9", &directory).len(),
            1,
            "an unsigned offer is unproven, so it is not a candidate on the checked path",
        );

        // 4. A participant with no key in the directory is dropped, not assumed.
        let stranger = ContractNet::new(Arc::clone(&agent), "stranger");
        stranger.offer_signed("pickup-9", "9 km", Some(&SigningKey::from_bytes(&[14u8; 32])), 4);
        assert!(
            hub.offers_verified("pickup-9", &directory).iter().all(|(_, o)| o.participant != "stranger"),
            "a key we cannot find is not a key we may assume",
        );

        agent.shutdown().await;
    }

    /// An award names a declarer, and a signed one proves it. The mirror of the offer case: the
    /// participant checks who accepted, not merely that something was written at the award key.
    #[tokio::test]
    async fn a_signed_award_proves_its_declarer_and_an_unsigned_one_proves_nothing() {
        use ed25519_dalek::SigningKey;
        let agent = live().await;
        let hub = ContractNet::new(Arc::clone(&agent), "hub");
        let d = ContractNet::new(Arc::clone(&agent), "driver-a");
        let hub_key = SigningKey::from_bytes(&[21u8; 32]);

        hub.announce("pickup-11", "1 crate", "pantry signs off", FAR_DEADLINE, 0);
        d.offer("pickup-11", "2 km", 1);
        hub.close("pickup-11", None, FAR_DEADLINE).unwrap();
        let plan = hub.plan_award("pickup-11", AwardRule::LowestParticipant, 10).expect("plans");
        assert!(!plan.is_signed(), "a planned award is unsigned until the declarer signs it");

        let signed = plan.signed(&hub_key);
        assert!(verify_award(&signed, &hub_key.verifying_key().to_bytes()));
        assert!(!verify_award(&signed, &[9u8; 32]), "and not under another key");

        let awarded = hub.commit_award(signed.clone()).await.expect("commits");
        let read_back = hub.award_of("pickup-11").expect("the award is readable");
        assert!(
            verify_award(&read_back, &hub_key.verifying_key().to_bytes()),
            "the signature survives the write and the read — it covers the record, not the transport",
        );
        assert_eq!(read_back.operation_id, awarded.award.operation_id);

        // The unsigned award is still legal and still proves nothing, which is the honest pair.
        let unsigned = Award { signature: Vec::new(), ..read_back };
        assert!(!unsigned.is_signed());
        assert!(!verify_award(&unsigned, &hub_key.verifying_key().to_bytes()), "unsigned never verifies");
        agent.shutdown().await;
    }

    /// Wall-clock ms — the clock the log's HLC reads (a test may read it directly; the crate does not).
    fn wall_now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// Row H: `award_of` reports only a record that names **this** requirement and its operation. A
    /// record at `cn/{r}/award` copied from another requirement's slot — any member can write the head —
    /// was reported as r's award, and blocked r's own award as `AlreadyAwarded`.
    #[tokio::test]
    async fn award_of_reports_only_a_record_that_names_this_requirement_and_its_operation() {
        let agent = live().await;
        let hub = ContractNet::new(Arc::clone(&agent), "hub");
        let d = ContractNet::new(Arc::clone(&agent), "driver-a");
        let now = wall_now_ms();
        hub.announce("r1", "1 crate", "any", now + 60_000, now);
        hub.announce("r2", "2 crates", "any", now + 60_000, now);
        d.offer("r1", "1 km", now).expect("announced");
        d.offer("r2", "2 km", now).expect("announced");
        hub.close("r1", None, now + 60_000).unwrap();
        hub.close("r2", None, now + 60_000).unwrap();
        let r1 = hub.award("r1", AwardRule::LowestParticipant, now).await.expect("r1 awarded").award;

        // r1's award, written at r2's slot by a member that can write the head.
        let _ = agent.kv().set("cn/r2/award", encode(&r1));
        assert!(hub.award_of("r2").is_none(), "a record naming r1 is not r2's award");
        let planned = hub.plan_award("r2", AwardRule::LowestParticipant, now).expect("r2 is not already awarded");
        assert_eq!(planned.requirement, "r2");
        agent.shutdown().await;
    }

    /// Row H: `award_of_verified` reports an award only when the declarer **the caller pins** signed it.
    /// A member that writes an award naming the declarer — unsigned, or signed under its own key — or
    /// naming itself as declarer under its own valid key, is not reported; the declarer's own signed
    /// award is. (Seen failing first as a missing method: the only read was the unverified one.)
    #[tokio::test]
    async fn award_of_verified_reports_no_award_the_pinned_declarer_did_not_sign() {
        use ed25519_dalek::SigningKey;
        let agent = live().await;
        let hub = ContractNet::new(Arc::clone(&agent), "hub");
        let mallory = ContractNet::new(Arc::clone(&agent), "mallory");
        let hub_key = SigningKey::from_bytes(&[31u8; 32]);
        let mallory_key = SigningKey::from_bytes(&[32u8; 32]);
        let directory: std::collections::HashMap<&str, [u8; 32]> =
            [("hub", hub_key.verifying_key().to_bytes()), ("mallory", mallory_key.verifying_key().to_bytes())]
                .into_iter()
                .collect();
        let resolve = |p: &str| directory.get(p).copied();
        let now = wall_now_ms();
        hub.announce("r", "1 crate", "any", FAR_DEADLINE, now);
        mallory.offer("r", "0 km", now).expect("announced");
        let forged = |declarer: &str| Award {
            requirement: "r".into(),
            participant: "mallory".into(),
            declarer: declarer.into(),
            offer_hlc: 0,
            awarded_at_ms: now,
            operation_id: award_key("r"),
            close_sha256: String::new(),
            signature: Vec::new(),
        };

        // Unsigned, naming the hub.
        mallory.commit_award(forged("hub")).await.expect("any member can write the head");
        assert!(hub.award_of("r").is_some(), "the unverified read reports it — unproven, as documented");
        assert!(hub.award_of_verified("r", "hub", resolve).is_none(), "unsigned is unproven");
        // Signed by mallory, naming the hub: a valid signature under the wrong key.
        mallory.commit_award(forged("hub").signed(&mallory_key)).await.expect("written");
        assert!(hub.award_of_verified("r", "hub", resolve).is_none(), "the declarer's key is checked, not the writer's");
        // Signed by mallory, naming mallory: a valid signature by a party that did not announce r.
        mallory.commit_award(forged("mallory").signed(&mallory_key)).await.expect("written");
        assert!(hub.award_of_verified("r", "hub", resolve).is_none(), "the award's declarer must be the announcement's");
        // The hub's own, signed — and a directory without the hub's key still proves nothing.
        hub.commit_award(forged("hub").signed(&hub_key)).await.expect("written");
        let proven = hub.award_of_verified("r", "hub", resolve).expect("the announcer's signed award is reported");
        assert_eq!((proven.declarer.as_str(), proven.participant.as_str()), ("hub", "mallory"));
        assert!(hub.award_of_verified("r", "hub", |_| None).is_none(), "a key we cannot find is not a key we may assume");
        agent.shutdown().await;
    }

    // ── #609's review, round 1 — each first written against 461da69f's API and seen failing there ──

    /// Review finding 1: the HLC in an offer's log key is a number the offerer writes — any member can
    /// `kv().set("log/cn/{r}/offers/{hlc}/{node}", …)` with a backdated segment. An offer that reached the
    /// stream after the requirement closed is not a candidate, whatever its key says.
    #[tokio::test]
    async fn a_backdated_log_key_does_not_make_a_late_offer_a_candidate() {
        let agent = live().await;
        let hub = ContractNet::new(Arc::clone(&agent), "hub");
        let honest = ContractNet::new(Arc::clone(&agent), "driver-b");
        let now = wall_now_ms();
        hub.announce("closed", "1 crate", "any", now + 60_000, now);
        honest.offer("closed", "3 km", now).expect("announced");
        hub.close("closed", None, now + 60_000).expect("the declarer closes at the deadline, naming the honest offer");
        let late = Offer { requirement: "closed".into(), participant: "driver-a".into(), bid: "0 km".into(), offered_at_ms: 1, signature: Vec::new() };
        let _ = agent.kv().set("log/cn/closed/offers/0000000000010000/mallory", encode(&late));
        let award = hub.plan_award("closed", AwardRule::LowestParticipant, now + 60_000).expect("the honest offer arrived in time");
        assert_eq!(award.participant, "driver-b", "the backdated offer reached the stream late and is not a candidate");
        agent.shutdown().await;
    }

    /// Review finding 3: a signed offer copied from another requirement's stream verified as a candidate
    /// there — its signature covers `requirement`, but nothing compared it. Same shape for reports and
    /// assessments.
    #[tokio::test]
    async fn an_offer_report_or_assessment_naming_another_requirement_is_not_read_as_this_ones() {
        use ed25519_dalek::SigningKey;
        let agent = live().await;
        let hub = ContractNet::new(Arc::clone(&agent), "hub");
        let a = ContractNet::new(Arc::clone(&agent), "driver-a");
        let key = SigningKey::from_bytes(&[41u8; 32]);
        let pk = key.verifying_key().to_bytes();
        let now = wall_now_ms();
        hub.announce("r1", "1 crate", "any", FAR_DEADLINE, now);
        hub.announce("r2", "1 crate", "any", FAR_DEADLINE, now);
        a.offer_signed("r1", "1 km", Some(&key), now).expect("announced");
        let r1_offer = a.offers("r1").pop().expect("r1 has the offer").1;
        agent.kv().append("cn/r2/offers", encode(&r1_offer));
        assert!(a.offers("r2").is_empty(), "r1's offer is not an offer on r2");
        assert!(hub.offers_verified("r2", |_| Some(pk)).is_empty(), "nor a verified candidate there");
        a.report("r1", Outcome::Fulfilled, "cn/r1/award", now);
        let r1_report = a.reports("r1").pop().expect("r1 has the report");
        agent.kv().append("cn/r2/reports", encode(&r1_report));
        assert!(a.reports("r2").is_empty(), "r1's report is not a report on r2");
        a.assess("r1", true, "ok", Some(&key), now);
        let r1_assessment = a.assessments("r1").pop().expect("r1 has the assessment");
        agent.kv().append("cn/r2/assessments", encode(&r1_assessment));
        assert!(a.assessments("r2").is_empty(), "r1's assessment is not an assessment of r2");
        agent.shutdown().await;
    }

    /// Review finding 4: `award_of_verified` read the declarer from the writable head `cn/{r}`, so it
    /// fell in two writes — overwrite the head's declarer, then sign the award with that key.
    #[tokio::test]
    async fn award_of_verified_checks_the_declarer_the_caller_pins_not_the_writable_head() {
        use ed25519_dalek::SigningKey;
        let agent = live().await;
        let hub = ContractNet::new(Arc::clone(&agent), "hub");
        let mallory = ContractNet::new(Arc::clone(&agent), "mallory");
        let hub_key = SigningKey::from_bytes(&[42u8; 32]);
        let mallory_key = SigningKey::from_bytes(&[43u8; 32]);
        let dir: std::collections::HashMap<&str, [u8; 32]> =
            [("hub", hub_key.verifying_key().to_bytes()), ("mallory", mallory_key.verifying_key().to_bytes())].into_iter().collect();
        let now = wall_now_ms();
        hub.announce("r", "1 crate", "any", FAR_DEADLINE, now);
        mallory.announce("r", "1 crate", "any", FAR_DEADLINE, now); // write 1: the head now names mallory
        let forged = Award {
            requirement: "r".into(), participant: "mallory".into(), declarer: "mallory".into(), offer_hlc: 0,
            awarded_at_ms: now, operation_id: award_key("r"), close_sha256: String::new(), signature: Vec::new(),
        }
        .signed(&mallory_key);
        mallory.commit_award(forged).await.expect("write 2");
        assert!(hub.award_of_verified("r", "hub", |p| dir.get(p).copied()).is_none(), "the hub announced r; mallory's award is not proven");
        assert!(hub.award_of_verified("r", "mallory", |p| dir.get(p).copied()).is_some(), "it proves only what it is: mallory's own");
        agent.shutdown().await;
    }

    /// Review finding 5: a committed record that names another requirement surfaced as
    /// `Encoding("superseded, but the committed award did not decode")` — it decodes; it is somebody else's.
    #[tokio::test]
    async fn a_slot_holding_another_requirements_award_says_so() {
        let agent = live().await;
        let hub = ContractNet::new(Arc::clone(&agent), "hub");
        let now = wall_now_ms();
        let r1 = Award {
            requirement: "r1".into(), participant: "driver-a".into(), declarer: "hub".into(), offer_hlc: 0,
            awarded_at_ms: now, operation_id: award_key("r1"), close_sha256: String::new(), signature: Vec::new(),
        };
        agent.consensus().exclusive_propose_receipt(&award_key("r2"), encode(&r1), ConsensusConfig::default()).await.expect("r1's award committed at r2's slot");
        let r2 = Award { requirement: "r2".into(), operation_id: award_key("r2"), ..r1 };
        let e = hub.commit_award_linearizable(r2).await.expect_err("the slot is taken");
        let said = e.to_string();
        assert!(!matches!(e, CommitmentRefusal::Encoding(_)), "it decoded; it is not an encoding failure: {said}");
        assert!(said.contains("r1"), "the refusal names what the slot holds: {said}");
        assert!(matches!(e, CommitmentRefusal::SlotHoldsAnotherRecord { ref requirement, .. } if requirement == "r1"));
        agent.shutdown().await;
    }

    /// Review round 1: an award before the requirement is closed is refused — the declarer has not yet
    /// said which offers arrived in time.
    #[tokio::test]
    async fn an_award_before_the_requirement_is_closed_is_refused() {
        let agent = live().await;
        let hub = ContractNet::new(Arc::clone(&agent), "hub");
        let d = ContractNet::new(Arc::clone(&agent), "driver-a");
        let now = wall_now_ms();
        hub.announce("open", "1 crate", "any", now + 60_000, now);
        d.offer("open", "1 km", now).expect("announced");
        assert!(hub.plan_award("open", AwardRule::LowestParticipant, now).is_err(), "no award before the close");
        assert!(matches!(hub.plan_award("open", AwardRule::LowestParticipant, now), Err(CommitmentRefusal::NotClosed)));
        agent.shutdown().await;
    }

    /// Review round 1, findings 1, 4 and 5 on the checked path: `plan_award_verified` requires a close
    /// the pinned declarer signed, counts only verified offers the close names, and is not blocked by
    /// an unsigned award at the slot — where the unverified path is, as its doc says. A linearizable slot
    /// a forgery took is reported as `UnprovenAwardHolds`, not as the award.
    #[tokio::test]
    async fn the_checked_path_needs_a_signed_close_and_is_not_blocked_by_a_forged_award() {
        use ed25519_dalek::SigningKey;
        let agent = live().await;
        let hub = ContractNet::new(Arc::clone(&agent), "hub");
        let d = ContractNet::new(Arc::clone(&agent), "driver-b");
        let mallory = ContractNet::new(Arc::clone(&agent), "mallory");
        let hub_key = SigningKey::from_bytes(&[51u8; 32]);
        let d_key = SigningKey::from_bytes(&[52u8; 32]);
        let dir: std::collections::HashMap<&str, [u8; 32]> =
            [("hub", hub_key.verifying_key().to_bytes()), ("driver-b", d_key.verifying_key().to_bytes())].into_iter().collect();
        let resolve = |p: &str| dir.get(p).copied();
        let now = wall_now_ms();

        hub.announce("p", "1 crate", "any", now, now);
        d.offer_signed("p", "2 km", Some(&d_key), now).expect("announced");
        // An unsigned close of this very announcement — a member's, or the declarer's without a key — is
        // not proof on this path, and it does not stop the declarer closing (no delete needed: round 2).
        let ann = hub.announcement("p").unwrap();
        forge_close(&agent, "p", serde_json::json!({
            "requirement": "p", "declarer": "hub", "deadline_ms": now, "announcement_sha256": ann.sha256(),
            "closed_at_ms": now, "offers": [], "signature": [],
        }));
        assert!(matches!(hub.plan_award_verified("p", AwardRule::LowestParticipant, now, "hub", resolve), Err(CommitmentRefusal::CloseUnproven)));
        let close = hub.close("p", Some(&hub_key), now).expect("the hub closes, signed");
        assert!(verify_close(&close, &hub_key.verifying_key().to_bytes()));

        // A forged unsigned award at the slot: blocks the unverified path for good, not the checked one.
        let forged = Award {
            requirement: "p".into(), participant: "mallory".into(), declarer: "hub".into(), offer_hlc: 0,
            awarded_at_ms: now, operation_id: award_key("p"), close_sha256: String::new(), signature: Vec::new(),
        };
        mallory.commit_award(forged.clone()).await.expect("any member can write the head");
        assert!(matches!(hub.plan_award("p", AwardRule::LowestParticipant, now), Err(CommitmentRefusal::AlreadyAwarded(_))),
            "the unverified path lets any member block an award — stated, not hidden");
        let plan = hub.plan_award_verified("p", AwardRule::LowestParticipant, now, "hub", resolve).expect("the checked path plans");
        assert_eq!(plan.participant, "driver-b");
        hub.commit_award(plan.signed(&hub_key)).await.expect("the plain commit overwrites the forgery");
        assert_eq!(hub.award_of_verified("p", "hub", resolve).map(|a| a.participant), Some("driver-b".to_string()));

        // A linearizable slot a forgery took first stays taken, and says so.
        hub.announce("q", "1 crate", "any", now, now);
        d.offer_signed("q", "2 km", Some(&d_key), now).expect("announced");
        hub.close("q", Some(&hub_key), now).unwrap();
        let plan_q = hub.plan_award_verified("q", AwardRule::LowestParticipant, now, "hub", resolve).unwrap().signed(&hub_key);
        let forged_q = Award { requirement: "q".into(), operation_id: award_key("q"), ..forged };
        mallory.commit_award_linearizable(forged_q).await.expect("the forgery commits first");
        match hub.commit_award_linearizable_verified(plan_q, "hub", resolve).await {
            Err(CommitmentRefusal::UnprovenAwardHolds(a)) => assert_eq!(a.participant, "mallory"),
            other => panic!("expected UnprovenAwardHolds, got {other:?}"),
        }
        agent.shutdown().await;
    }

    // ── #609's review, round 2 — each first written against 9d4da849's API and seen failing there ──

    /// Write a forged close where either head reads one: 9d4da849's single key `cn/{r}/close` and the
    /// per-requirement close stream `cn/{r}/closes`. Raw JSON, so the same body runs on both heads.
    fn forge_close(agent: &GossipAgent, requirement: &str, close: serde_json::Value) {
        let bytes = serde_json::to_vec(&close).unwrap();
        let _ = agent.kv().set(format!("cn/{requirement}/close").as_str(), bytes.clone());
        agent.kv().append(&format!("cn/{requirement}/closes"), bytes);
    }

    /// Overwrite every close a reader could find — the single key and each entry on the close stream.
    fn overwrite_closes(agent: &GossipAgent, requirement: &str, close: serde_json::Value) {
        let bytes = serde_json::to_vec(&close).unwrap();
        let _ = agent.kv().set(format!("cn/{requirement}/close").as_str(), bytes.clone());
        for (key, _) in agent.kv().scan_prefix(&format!("log/cn/{requirement}/closes/")) {
            let _ = agent.kv().set(key.as_ref(), bytes.clone());
        }
    }

    fn keys_for(names: &[(&'static str, u8)]) -> std::collections::HashMap<&'static str, ed25519_dalek::SigningKey> {
        names.iter().map(|(n, b)| (*n, ed25519_dalek::SigningKey::from_bytes(&[*b; 32]))).collect()
    }

    /// Round 2, N2: a member's unsigned close written first stopped the declarer from ever closing —
    /// `close()` answered `AlreadyClosed` from the unverified read, and the checked path was
    /// `CloseUnproven` for good.
    #[tokio::test]
    async fn a_forged_close_written_first_does_not_stop_the_declarers_signed_close() {
        let agent = live().await;
        let keys = keys_for(&[("hub", 61), ("driver-b", 62)]);
        let resolve = |p: &str| keys.get(p).map(|k| k.verifying_key().to_bytes());
        let hub = ContractNet::new(Arc::clone(&agent), "hub");
        let d = ContractNet::new(Arc::clone(&agent), "driver-b");
        let now = wall_now_ms();
        hub.announce("n2", "1 crate", "any", now, now);
        d.offer_signed("n2", "2 km", Some(&keys["driver-b"]), now).expect("announced");
        forge_close(&agent, "n2", serde_json::json!({
            "requirement": "n2", "declarer": "hub", "closed_at_ms": now, "deadline_ms": now, "offers": [], "signature": [],
        }));
        hub.close("n2", Some(&keys["hub"]), now).expect("a forged close does not pre-empt the declarer's signed one");
        let plan = hub.plan_award_verified("n2", AwardRule::LowestParticipant, now, "hub", resolve).expect("the checked path plans");
        assert_eq!(plan.participant, "driver-b");
        agent.shutdown().await;
    }

    /// Round 2, N3: an overwrite of the declarer's genuine close left the checked path `CloseUnproven`
    /// and re-closing `AlreadyClosed` — permanent. At worst it must be recoverable by closing again.
    #[tokio::test]
    async fn an_overwritten_close_is_recovered_by_the_declarer_closing_again() {
        let agent = live().await;
        let keys = keys_for(&[("hub", 63), ("driver-b", 64)]);
        let resolve = |p: &str| keys.get(p).map(|k| k.verifying_key().to_bytes());
        let hub = ContractNet::new(Arc::clone(&agent), "hub");
        let d = ContractNet::new(Arc::clone(&agent), "driver-b");
        let now = wall_now_ms();
        hub.announce("n3", "1 crate", "any", now, now);
        d.offer_signed("n3", "2 km", Some(&keys["driver-b"]), now).expect("announced");
        hub.close("n3", Some(&keys["hub"]), now).expect("closes");
        overwrite_closes(&agent, "n3", serde_json::json!({
            "requirement": "n3", "declarer": "hub", "closed_at_ms": now, "deadline_ms": now, "offers": [], "signature": [],
        }));
        hub.close("n3", Some(&keys["hub"]), now).expect("the declarer closes again: an overwrite is recoverable");
        let plan = hub.plan_award_verified("n3", AwardRule::LowestParticipant, now, "hub", resolve).expect("the checked path plans");
        assert_eq!(plan.participant, "driver-b");
        agent.shutdown().await;
    }

    /// Round 2, N1: on the unverified path a forged close dated before the deadline, or one naming another
    /// declarer, steered the award — `plan_award_from` checked neither. What this does **not** show: a
    /// forged unsigned close dated at or after the deadline, naming the announcement's declarer, still
    /// steers the unverified path — that path is steerable by design, and says so.
    #[tokio::test]
    async fn on_the_unverified_path_a_close_dated_before_the_deadline_or_naming_another_declarer_is_refused() {
        let agent = live().await;
        let hub = ContractNet::new(Arc::clone(&agent), "hub");
        let mallory = ContractNet::new(Arc::clone(&agent), "a-mallory");
        let now = wall_now_ms();
        hub.announce("n1", "1 crate", "any", now + 60_000, now);
        mallory.offer("n1", "0 km", now).expect("announced");
        let (hlc, offer) = mallory.offers("n1").pop().expect("mallory's offer");
        let named = ClosedOffer::of(hlc, &offer);
        // Each forgery binds the real announcement (added after the 9d4da849 run, whose decoder ignores the
        // field), so what refuses it is the check under test and not a binding mismatch.
        let digest = hub.announcement("n1").unwrap().sha256();
        forge_close(&agent, "n1", serde_json::json!({
            "requirement": "n1", "declarer": "hub", "closed_at_ms": now, "deadline_ms": now + 60_000,
            "announcement_sha256": digest, "offers": [named.clone()], "signature": [],
        }));
        assert!(hub.plan_award("n1", AwardRule::LowestParticipant, now).is_err(), "a close before the deadline is not a close");
        forge_close(&agent, "n1", serde_json::json!({
            "requirement": "n1", "declarer": "a-mallory", "closed_at_ms": now + 60_000, "deadline_ms": now + 60_000,
            "announcement_sha256": digest, "offers": [named], "signature": [],
        }));
        assert!(hub.plan_award("n1", AwardRule::LowestParticipant, now + 60_000).is_err(), "a close by another declarer is not a close");
        assert!(matches!(hub.plan_award("n1", AwardRule::LowestParticipant, now + 60_000), Err(CommitmentRefusal::NotClosed)));
        agent.shutdown().await;
    }

    /// Round 2: an oversize close answered `Ok` and wrote nothing (`kv().set`'s refusal was discarded);
    /// every plan was then `NotClosed`. It is refused by name, before anything is written.
    #[tokio::test]
    async fn an_oversize_close_is_refused_by_name_not_reported_as_written() {
        let agent = live().await;
        let hub = ContractNet::new(Arc::clone(&agent), "hub");
        let now = wall_now_ms();
        hub.announce("big", "1 crate", "any", now, now);
        let offer = Offer { requirement: "big".into(), participant: "d".into(), bid: String::new(), offered_at_ms: now, signature: Vec::new() };
        let encoded = encode(&offer);
        for _ in 0..115_000 {
            agent.kv().append("cn/big/offers", encoded.clone());
        }
        let result = hub.close("big", None, now);
        let said = result.as_ref().err().map(|e| e.to_string()).unwrap_or_default();
        assert!(result.is_err(), "a close that cannot be written is not Ok");
        assert!(said.contains("too large"), "refused by name: {said}");
        assert!(matches!(result, Err(CommitmentRefusal::CloseTooLarge { .. })));
        assert!(hub.closes("big").is_empty(), "nothing was written");
        agent.shutdown().await;
    }

    /// Round 2, finding 5: an award records the close it was chosen from, and a close binds the
    /// announcement it closes — a close of an earlier announcement under the same name is not a close of
    /// the current one. (New API: no head before round 2 had either field.)
    #[tokio::test]
    async fn an_award_names_its_close_and_a_close_binds_its_announcement() {
        let agent = live().await;
        let hub = ContractNet::new(Arc::clone(&agent), "hub");
        let d = ContractNet::new(Arc::clone(&agent), "driver-a");
        let key = ed25519_dalek::SigningKey::from_bytes(&[71u8; 32]);
        let now = wall_now_ms();
        hub.announce("x", "1 crate", "any", now, now);
        d.offer("x", "1 km", now).expect("announced");
        let close = hub.close("x", Some(&key), now).expect("closes");
        assert_eq!((close.deadline_ms, close.announcement_sha256.clone()), (now, hub.announcement("x").unwrap().sha256()));
        let award = hub.plan_award("x", AwardRule::LowestParticipant, now).expect("plans");
        assert_eq!(award.close_sha256, close.sha256(), "the award names the close it was chosen from");

        hub.announce("x", "2 crates", "any", now, now + 1); // re-announced: new terms, same name
        assert!(matches!(hub.plan_award("x", AwardRule::LowestParticipant, now), Err(CommitmentRefusal::NotClosed)),
            "the earlier close does not close the new announcement");
        hub.close("x", Some(&key), now).expect("the new announcement is closed afresh");
        agent.shutdown().await;
    }

    // ── #609's review, round 3 — each first written against d90c2d51's API and seen failing there ──

    /// Round 3, finding 1: recovery from an overwrite must re-assert the declarer's original decision.
    /// The hub closes naming driver-a; a late signed offer arrives; every close is overwritten; the hub
    /// closes again. A re-close that recomputed from everything held now named the late offer, and the
    /// checked path awarded it.
    #[tokio::test]
    async fn a_reclose_after_an_overwrite_never_admits_an_offer_the_first_close_did_not() {
        let agent = live().await;
        let keys = keys_for(&[("hub", 81), ("driver-a", 82), ("a-mallory", 83)]);
        let resolve = |p: &str| keys.get(p).map(|k| k.verifying_key().to_bytes());
        let hub = ContractNet::new(Arc::clone(&agent), "hub");
        let a = ContractNet::new(Arc::clone(&agent), "driver-a");
        let mallory = ContractNet::new(Arc::clone(&agent), "a-mallory");
        let now = wall_now_ms();
        hub.announce("rc", "1 crate", "any", now, now);
        a.offer_signed("rc", "2 km", Some(&keys["driver-a"]), now).expect("announced");
        hub.close("rc", Some(&keys["hub"]), now).expect("closes naming driver-a");
        mallory.offer_signed("rc", "0 km", Some(&keys["a-mallory"]), now).expect("a late offer, signed");
        overwrite_closes(&agent, "rc", serde_json::json!({ "requirement": "rc", "declarer": "hub", "offers": [] }));
        let _ = hub.close("rc", Some(&keys["hub"]), now + 1);
        let plan = hub.plan_award_verified("rc", AwardRule::LowestParticipant, now + 1, "hub", resolve).expect("plans");
        assert_eq!(plan.participant, "driver-a", "the late offer was never in the hub's decision");
        agent.shutdown().await;
    }

    /// Round 3, finding 2: with two signed closes on the stream, which counts was chosen by stream key —
    /// a key any member writes. C1 decides driver-a; a second signed close C2 (naming a later offer too)
    /// is planted at a backdated key; the checked plan must still answer driver-a, from C1.
    #[tokio::test]
    async fn a_second_signed_close_at_a_backdated_key_does_not_change_the_award() {
        use ed25519_dalek::Signer;
        let agent = live().await;
        let keys = keys_for(&[("hub", 84), ("driver-a", 85), ("a-mallory", 86)]);
        let resolve = |p: &str| keys.get(p).map(|k| k.verifying_key().to_bytes());
        let hub = ContractNet::new(Arc::clone(&agent), "hub");
        let a = ContractNet::new(Arc::clone(&agent), "driver-a");
        let mallory = ContractNet::new(Arc::clone(&agent), "a-mallory");
        let now = wall_now_ms();
        hub.announce("newb", "1 crate", "any", now, now);
        a.offer_signed("newb", "2 km", Some(&keys["driver-a"]), now).expect("announced");
        let c1 = hub.close("newb", Some(&keys["hub"]), now).expect("C1");
        let first = hub.plan_award_verified("newb", AwardRule::LowestParticipant, now, "hub", resolve).expect("plans under C1");
        assert_eq!(first.participant, "driver-a");

        mallory.offer_signed("newb", "0 km", Some(&keys["a-mallory"]), now).expect("a later offer");
        let mut c2 = Close {
            closed_at_ms: now + 5_000,
            offers: hub.offers("newb").iter().map(|(h, o)| ClosedOffer::of(*h, o)).collect(),
            signature: Vec::new(),
            ..c1.clone()
        };
        c2.signature = keys["hub"].sign(&c2.canonical_bytes()).to_bytes().to_vec();
        let _ = agent.kv().set("log/cn/newb/closes/0000000000010000/backdated", encode(&c2));

        let again = hub.plan_award_verified("newb", AwardRule::LowestParticipant, now, "hub", resolve).expect("plans");
        assert_eq!(again.participant, "driver-a", "the award planned under C1 is the award every later plan computes");
        assert_eq!(again.close_sha256, c1.sha256(), "and it names C1");
        agent.shutdown().await;
    }

    /// Round 3: a declarer that restarted (a fresh handle, no memory of its close) recovers an overwritten
    /// close by re-asserting the close it kept — the same decision, never a new one. A close of another
    /// announcement is refused by name. (New API: no head before round 3 had `reassert_close`.)
    #[tokio::test]
    async fn a_restarted_declarer_reasserts_the_close_it_kept() {
        let agent = live().await;
        let keys = keys_for(&[("hub", 87), ("driver-a", 88), ("a-mallory", 89)]);
        let resolve = |p: &str| keys.get(p).map(|k| k.verifying_key().to_bytes());
        let hub = ContractNet::new(Arc::clone(&agent), "hub");
        let a = ContractNet::new(Arc::clone(&agent), "driver-a");
        let mallory = ContractNet::new(Arc::clone(&agent), "a-mallory");
        let now = wall_now_ms();
        hub.announce("rs", "1 crate", "any", now, now);
        a.offer_signed("rs", "2 km", Some(&keys["driver-a"]), now).expect("announced");
        let kept = hub.close("rs", Some(&keys["hub"]), now).expect("closes; the caller keeps this");
        mallory.offer_signed("rs", "0 km", Some(&keys["a-mallory"]), now).expect("a late offer");
        overwrite_closes(&agent, "rs", serde_json::json!({ "requirement": "rs" }));

        let restarted = ContractNet::new(Arc::clone(&agent), "hub");
        assert_eq!(restarted.reassert_close(&kept).expect("re-asserted"), kept, "byte for byte");
        let plan = restarted.plan_award_verified("rs", AwardRule::LowestParticipant, now, "hub", resolve).expect("plans");
        assert_eq!((plan.participant.as_str(), plan.close_sha256.clone()), ("driver-a", kept.sha256()));

        hub.announce("rs", "2 crates", "any", now, now + 1); // a new announcement under the same name
        assert!(matches!(restarted.reassert_close(&kept), Err(CommitmentRefusal::CloseNotForThisAnnouncement)));
        assert!(matches!(mallory.reassert_close(&kept), Err(CommitmentRefusal::NotTheDeclarer { .. })));
        agent.shutdown().await;
    }
}
