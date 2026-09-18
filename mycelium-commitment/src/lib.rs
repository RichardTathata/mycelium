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
//! | **offer** — a participant's willingness; *not yet an obligation* | `append` on a log stream | `log/cn/{requirement}/offers` |
//! | **award** — acceptance: one participant, one requirement, once | the deterministic lowest-participant rule (the tuple-space primary election's rule), written with **`set_with_receipt`** | `cn/{requirement}/award` |
//! | **report** — the awardee's receipt of the work: outcome, or *unknown* | `append` | `log/cn/{requirement}/reports` |
//! | **assess** — whether the requirement was satisfied; signed | `append`, Ed25519 over the record | `log/cn/{requirement}/assessments` |
//!
//! ## The rules, in the plan's words
//!
//! - **No component assigns another participant's obligation.** An award only records an *offer* the
//!   participant made; a requirement with no offers is a visible state ([`CommitmentRefusal::NoOffers`]),
//!   not a retry loop.
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
//!   [`verify_assessment`] says `false` for it rather than true-by-absence.
//!
//! Heads and terms are in the gossip KV; offers, reports and assessments are the companion's streams
//! under one prefix (`cn/`, registered in `mycelium`'s namespace table). Time enters through the
//! caller (`now_ms`), so every decision here is pure in time and a harness can drive it.

use bytes::Bytes;
use mycelium::{GossipAgent, OperationId, ReceiptError, WriteReceipt};
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
    /// Offers stamped after this are not considered by an award.
    pub deadline_ms: u64,
    pub announced_at_ms: u64,
}

/// A participant's willingness to undertake a requirement. Not yet an obligation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    pub requirement: String,
    pub participant: String,
    pub bid: String,
    pub offered_at_ms: u64,
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

/// Why an award was not made. **Each is a visible state, not a retry.**
#[derive(Debug)]
pub enum CommitmentRefusal {
    /// No announcement head exists for the requirement.
    NotAnnounced,
    /// The requirement was announced and nobody offered before its deadline.
    NoOffers,
    /// The requirement already has an award; it is returned, never overwritten.
    AlreadyAwarded(Award),
    /// The award's write did not yield a receipt (item 1's vocabulary: the fate may be unknown).
    Receipt(ReceiptError),
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
        self.announcement(requirement)?;
        let o = Offer { requirement: requirement.to_string(), participant: self.me.clone(), bid: bid.into(), offered_at_ms: now_ms };
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

    /// **Award** the requirement by `rule` over the offers made by its deadline, writing the award
    /// **with a receipt** under `operation_id = cn/{requirement}/award`. Refused — not overwritten —
    /// when an award already exists; refused visibly when nobody offered.
    pub async fn award(&self, requirement: &str, rule: AwardRule, now_ms: u64) -> Result<Awarded, CommitmentRefusal> {
        let announcement = self.announcement(requirement).ok_or(CommitmentRefusal::NotAnnounced)?;
        if let Some(existing) = self.award_of(requirement) {
            return Err(CommitmentRefusal::AlreadyAwarded(existing));
        }
        let eligible: Vec<(u64, Offer)> =
            self.offers(requirement).into_iter().filter(|(_, o)| o.offered_at_ms <= announcement.deadline_ms).collect();
        let (offer_hlc, offer) = rule.choose(&eligible).cloned().ok_or(CommitmentRefusal::NoOffers)?;
        let operation_id = award_key(requirement);
        let award = Award {
            requirement: requirement.to_string(),
            participant: offer.participant,
            declarer: self.me.clone(),
            offer_hlc,
            awarded_at_ms: now_ms,
            operation_id: operation_id.clone(),
        };
        let receipt = self
            .agent
            .kv()
            .set_with_receipt(&OperationId::new(operation_id), award_key(requirement).as_str(), encode(&award))
            .await
            .map_err(CommitmentRefusal::Receipt)?;
        Ok(Awarded { award, receipt })
    }

    /// The award, if the requirement has one.
    pub fn award_of(&self, requirement: &str) -> Option<Award> {
        self.agent.kv().get(&award_key(requirement)).and_then(|b| decode(&b))
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

    fn offer(participant: &str) -> Offer {
        Offer { requirement: "r".into(), participant: participant.into(), bid: "".into(), offered_at_ms: 0 }
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
        hub.announce("pickup-1", "2 crates, 14:00", "delivered before 15:00", 100, 0);
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
        hub.announce("pickup-2", "1 crate", "any time today", 100, 0);
        assert!(matches!(hub.award("pickup-2", AwardRule::LowestParticipant, 10).await, Err(CommitmentRefusal::NoOffers)));
        d.offer("pickup-2", "late", 101); // after the deadline
        assert!(matches!(hub.award("pickup-2", AwardRule::LowestParticipant, 200).await, Err(CommitmentRefusal::NoOffers)),
            "an offer after the deadline is not considered");
        assert_eq!(hub.offers("pickup-2").len(), 1, "but it is on the record");
        agent.shutdown().await;
    }

    #[tokio::test]
    async fn reports_and_assessments_round_trip_and_an_unsigned_assessment_never_verifies() {
        use ed25519_dalek::SigningKey;
        let agent = live().await;
        let hub = ContractNet::new(Arc::clone(&agent), "hub");
        let d = ContractNet::new(Arc::clone(&agent), "driver-a");
        let auditor = ContractNet::new(Arc::clone(&agent), "food-bank-auditor");
        hub.announce("pickup-3", "3 crates", "signed off by the receiving pantry", 100, 0);
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
}
