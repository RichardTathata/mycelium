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
//! caller (`now_ms`), so every decision here is pure in time and a harness can drive it — with one
//! deliberate exception: **the deadline is judged on the log's clock**, the HLC the stream stamped
//! when an offer was appended, not on the stamp the offerer wrote into its record (post-360 row H;
//! an offerer used to choose whether its own offer was late). That HLC is minted by the substrate,
//! monotone, and never below anything the offerer's node had already observed — so a late offer
//! cannot be backdated past the announcement's arrival — but it is the offerer's *node's* clock
//! within the mesh's drift bound (`GossipConfig::max_clock_drift_ms`), which is the residual.
//!
//! An award read back is checked the same way an offer is: [`ContractNet::award_of`] reports only a
//! record that names its requirement and operation, and [`ContractNet::award_of_verified`] only one
//! the announcement's declarer signed.

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
    /// Offers whose **log position** is after this are not considered by an award: the HLC the
    /// stream stamped when the offer was appended, read as wall-clock ms (`hlc::physical_ms`) —
    /// never the `offered_at_ms` the offerer wrote into its own record. So a deadline is expressed on
    /// the mesh's clock, and an offerer decides *whether* to offer, not whether its offer was late.
    pub deadline_ms: u64,
    pub announced_at_ms: u64,
}

/// A participant's willingness to undertake a requirement. Not yet an obligation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    pub requirement: String,
    pub participant: String,
    pub bid: String,
    /// The offerer's own stamp — part of the signed record, **not** what the deadline is judged on
    /// (that is the offer's log position; see [`Announcement::deadline_ms`]).
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
}

impl ContractNet {
    pub fn new(agent: Arc<GossipAgent>, me: impl Into<String>) -> Self {
        Self { agent, me: me.into() }
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

    /// Every offer on the requirement, with its log position, in log order.
    pub fn offers(&self, requirement: &str) -> Vec<(u64, Offer)> {
        self.agent
            .kv()
            .scan_log(&offers_stream(requirement), 0, u64::MAX)
            .into_iter()
            .filter_map(|e| decode::<Offer>(&e.value).map(|o| (e.hlc, o)))
            .collect()
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
    /// The deadline and the already-awarded check still apply — they are the requirement's, not the
    /// candidate set's.
    pub fn plan_award_from(
        &self,
        requirement: &str,
        rule: AwardRule,
        now_ms: u64,
        candidates: Vec<(u64, Offer)>,
    ) -> Result<Award, CommitmentRefusal> {
        let announcement = self.announcement(requirement).ok_or(CommitmentRefusal::NotAnnounced)?;
        if let Some(existing) = self.award_of(requirement) {
            return Err(CommitmentRefusal::AlreadyAwarded(Box::new(existing)));
        }
        // The deadline is judged on the **log's clock**: the HLC the stream stamped when the offer was
        // appended, read as wall-clock ms — never on the `offered_at_ms` the offerer wrote into its own
        // record, which let an offerer backdate a late offer or future-stamp a timely one (post-360 row H).
        let eligible: Vec<(u64, Offer)> = candidates
            .into_iter()
            .filter(|(hlc, _)| mycelium::hlc::physical_ms(*hlc) <= announcement.deadline_ms)
            .collect();
        let (offer_hlc, offer) = rule.choose(&eligible).cloned().ok_or(CommitmentRefusal::NoOffers)?;
        Ok(Award {
            requirement: requirement.to_string(),
            participant: offer.participant,
            declarer: self.me.clone(),
            offer_hlc,
            awarded_at_ms: now_ms,
            operation_id: award_key(requirement),
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
    pub async fn commit_award_linearizable(&self, award: Award) -> Result<AwardedLinearizable, CommitmentRefusal> {
        let slot = award_key(&award.requirement);
        // An award is an exclusive outcome (P2, #601's review): marked safety-sensitive and decided where the
        // fleet decides those — its exclusive-default electorate group, or the whole cluster when none is declared.
        match self.agent.consensus().exclusive_propose_receipt(&slot, encode(&award), ConsensusConfig::default()).await {
            Ok(commit) => Ok(AwardedLinearizable { award, commit }),
            Err(CommitError::Superseded { .. }) => {
                let existing = self.award_of(&award.requirement).ok_or_else(|| {
                    CommitmentRefusal::Encoding("superseded, but the committed award did not decode".into())
                })?;
                Err(CommitmentRefusal::AlreadyAwarded(Box::new(existing)))
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
    /// crate's usual sense — a claim, until a signature under the announcement's declarer says
    /// otherwise.
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

    /// The award, if the requirement has one **and the announcement's declarer signed it** — the
    /// read a participant that cares about provenance should make before acting on an acceptance.
    ///
    /// Reported only when [`award_of`](Self::award_of)'s checks pass, the award's `declarer` is the
    /// declarer the announcement head names, and the signature verifies under the key `resolve`
    /// gives for that declarer. An unsigned award is not reported, nor one signed by anybody else —
    /// a member that writes an award naming the declarer under its own valid key, or naming itself as
    /// declarer, does not pass, because the key checked is the *announcer's*. `resolve` returning
    /// `None` drops the award: a key we cannot find is not a key we may assume. Where the key comes
    /// from is the caller's business, as for [`offers_verified`](Self::offers_verified).
    ///
    /// The announcement head is itself unsigned and declarer-owned by convention, so what this
    /// establishes is *the party that announced the requirement accepted this offer*, under the
    /// same `require_identity_proofs` caveat every signature in this crate carries.
    pub fn award_of_verified(&self, requirement: &str, resolve: impl Fn(&str) -> Option<[u8; 32]>) -> Option<Award> {
        let announcement = self.announcement(requirement)?;
        let award = self.award_of(requirement)?;
        (award.declarer == announcement.declarer
            && resolve(&award.declarer).is_some_and(|k| verify_award(&award, &k)))
        .then_some(award)
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

    /// Every report on the requirement, in log order.
    pub fn reports(&self, requirement: &str) -> Vec<Report> {
        self.agent
            .kv()
            .scan_log(&reports_stream(requirement), 0, u64::MAX)
            .into_iter()
            .filter_map(|e| decode(&e.value))
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

    /// Every assessment on the requirement, in log order.
    pub fn assessments(&self, requirement: &str) -> Vec<Assessment> {
        self.agent
            .kv()
            .scan_log(&assessments_stream(requirement), 0, u64::MAX)
            .into_iter()
            .filter_map(|e| decode(&e.value))
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

    /// A deadline far in the future on the log's clock (2096), so an offer appended now is eligible —
    /// a constant, so the sim-replay test's recording and replay announce the same bytes.
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
        // A deadline 100 ms after the epoch is long past on the log's clock, which is what the deadline
        // is judged on: an offer appended now is late whatever `now_ms` it carries.
        hub.announce("pickup-2", "1 crate", "any time today", 100, 0);
        assert!(matches!(hub.award("pickup-2", AwardRule::LowestParticipant, 10).await, Err(CommitmentRefusal::NoOffers)));
        d.offer("pickup-2", "late", 101); // appended after the deadline
        assert!(matches!(hub.award("pickup-2", AwardRule::LowestParticipant, 200).await, Err(CommitmentRefusal::NoOffers)),
            "an offer after the deadline is not considered");
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
        let plan_a = hub_a.plan_award("plain", AwardRule::LowestParticipant, 10).unwrap();
        let plan_b = hub_b.plan_award("plain", AwardRule::LowestParticipant, 11).unwrap();
        assert!(hub_a.commit_award(plan_a).await.is_ok());
        assert!(hub_b.commit_award(plan_b).await.is_ok(), "the plain path lets the second commit too");
        assert_eq!(hub_a.award_of("plain").unwrap().declarer, "hub-b", "LWW kept the later write: a double award, silently");

        hub_a.announce("linear", "1 crate", "any", FAR_DEADLINE, 0);
        d.offer("linear", "3 km", 1);
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

    /// Post-360 row H: the deadline is judged on the **log's clock** — the HLC the stream stamped when
    /// the offer was appended — never on the `offered_at_ms` the offerer wrote into its own record.
    /// Before this, an offer appended after the deadline with a backdated stamp was eligible and one
    /// appended in time with a future stamp was not: the offerer chose whether its offer was late.
    #[tokio::test]
    async fn an_offers_deadline_is_judged_on_the_logs_clock_not_the_offerers_stamp() {
        let agent = live().await;
        let hub = ContractNet::new(Arc::clone(&agent), "hub");
        let d = ContractNet::new(Arc::clone(&agent), "driver-a");
        let now = wall_now_ms();

        // 1. Offers closed a minute ago on the log's clock; the offer appended now claims time 0.
        hub.announce("closed", "1 crate", "any", now - 60_000, now - 120_000);
        d.offer("closed", "backdated", 0).expect("announced");
        assert!(
            matches!(hub.plan_award("closed", AwardRule::LowestParticipant, now), Err(CommitmentRefusal::NoOffers)),
            "an offer appended after the deadline is late whatever stamp it carries",
        );

        // 2. Offers open for another minute; the offer appended now claims the end of time.
        hub.announce("open", "1 crate", "any", now + 60_000, now);
        d.offer("open", "future-stamped", u64::MAX).expect("announced");
        let award = hub
            .plan_award("open", AwardRule::LowestParticipant, now)
            .expect("an offer appended before the deadline is eligible whatever stamp it carries");
        assert_eq!(award.participant, "driver-a");
        agent.shutdown().await;
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
        let r1 = hub.award("r1", AwardRule::LowestParticipant, now).await.expect("r1 awarded").award;

        // r1's award, written at r2's slot by a member that can write the head.
        let _ = agent.kv().set("cn/r2/award", encode(&r1));
        assert!(hub.award_of("r2").is_none(), "a record naming r1 is not r2's award");
        let planned = hub.plan_award("r2", AwardRule::LowestParticipant, now).expect("r2 is not already awarded");
        assert_eq!(planned.requirement, "r2");
        agent.shutdown().await;
    }

    /// Row H: `award_of_verified` reports an award only when the **announcement's declarer** signed it.
    /// A member that writes an award naming the declarer — unsigned, or signed under its own key — or
    /// naming itself as declarer under its own valid key, is not reported; the declarer's own signed
    /// award is. (Seen failing first as a missing method: the only read was the unverified one.)
    #[tokio::test]
    async fn award_of_verified_reports_no_award_the_announcements_declarer_did_not_sign() {
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
            signature: Vec::new(),
        };

        // Unsigned, naming the hub.
        mallory.commit_award(forged("hub")).await.expect("any member can write the head");
        assert!(hub.award_of("r").is_some(), "the unverified read reports it — unproven, as documented");
        assert!(hub.award_of_verified("r", resolve).is_none(), "unsigned is unproven");
        // Signed by mallory, naming the hub: a valid signature under the wrong key.
        mallory.commit_award(forged("hub").signed(&mallory_key)).await.expect("written");
        assert!(hub.award_of_verified("r", resolve).is_none(), "the declarer's key is checked, not the writer's");
        // Signed by mallory, naming mallory: a valid signature by a party that did not announce r.
        mallory.commit_award(forged("mallory").signed(&mallory_key)).await.expect("written");
        assert!(hub.award_of_verified("r", resolve).is_none(), "the award's declarer must be the announcement's");
        // The hub's own, signed — and a directory without the hub's key still proves nothing.
        hub.commit_award(forged("hub").signed(&hub_key)).await.expect("written");
        let proven = hub.award_of_verified("r", resolve).expect("the announcer's signed award is reported");
        assert_eq!((proven.declarer.as_str(), proven.participant.as_str()), ("hub", "mallory"));
        assert!(hub.award_of_verified("r", |_| None).is_none(), "a key we cannot find is not a key we may assume");
        agent.shutdown().await;
    }
}
