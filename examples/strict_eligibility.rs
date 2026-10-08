//! **Strict eligibility, from a history source that establishes its own coverage** — realignment
//! repairs A3 (`docs/plans/realignment-repairs.md` §3.4, D4).
//!
//! ```text
//! cargo run --example strict_eligibility --features tls
//! ```
//!
//! # The setting
//!
//! A food co-op rotates its coordinator role every four weeks, and its members agreed that nobody
//! holds it for more than **two terms in a row** — so the work of running the pick-ups and the
//! supplier rota keeps being learned by new people. The members' assembly is the appointing
//! authority: it records each appointment in the knowledge layer and publishes a **signed head** on
//! one stream, `appointments/coordinator`, each head naming the head before it.
//!
//! # The claim this exists to make checkable
//!
//! *Unknown history is never eligible* — and the history's coverage is established by the **source**,
//! never asserted by the caller. `mandate::history_source::history_from_appointment_stream` walks back
//! from **this node's reader's checkpoint** along the heads' `prev` digests, authenticating each against
//! the assembly's keys as this node holds them; a linked head whose appointment record this node cannot
//! fetch is a gap, and only what follows the last gap is vouched for. `eligibility::eligible_strict`
//! then answers per configured rule: `Eligible`, `Ineligible`, or `Unknown` naming what is missing.
//!
//! Six acts, each asserted — the program exits non-zero on a miss:
//!
//! 1. the assembly publishes six appointments; a member's node verifies them and its reader advances;
//! 2. a candidate who rested for two terms is **eligible**;
//! 3. a candidate who has served two terms in a row is **not eligible**;
//! 4. **unknown**: a node missing one appointment record (the lenient check says yes on the same
//!    records), and a node whose reader holds no checkpoint for the stream (all heads presented to it);
//! 5. revoking the assembly's old signing key does **not** end the role's history — for a reader whose
//!    checkpoint is under the current key; one whose checkpoint is under the revoked key vouches for nothing;
//! 6. readiness: the incoming coordinator has read the handover journal **and** is eligible.
//!
//! # What it does not demonstrate
//!
//! That the assembly published every appointment there was — a source vouches for the chain it holds.
//! That this node's reader has heard the assembly's newest head: "current" means as of the reader's
//! checkpoint. Records and heads are handed over in-process here rather than gossiped; the gossip path is
//! the knowledge layer's own (`examples/knowledge_layer.rs`).

use ed25519_dalek::SigningKey;
use mycelium::knowledge::heads::{HeadCheckpoints, HeadVerdict, MemoryCheckpointStore, SignedHead};
use mycelium::knowledge::issuer::{MemberKeys, TrustedExternalIssuers};
use mycelium::knowledge::store::{Head, KnowledgeStore, SignedRecord};
use mycelium::knowledge::{IssuerId, KnowledgeRecord, RecordId, RecordKind};
use mycelium::mandate::eligibility::{eligible_strict, ready, NotReadyStrict, RuleVerdict, StrictEligibility, TermHistory};
use mycelium::mandate::handover::{eligible, EntryKind, HandoverJournal, IncumbencyRules, Ineligible, JournalEntry, Successor, TermRecord};
use mycelium::mandate::history_source::{history_from_appointment_stream, AppointmentStream, StreamOrigin};
use mycelium::mandate::{PrincipalId, TermId};
use mycelium::NodeId;
use std::collections::{HashMap, HashSet};

const DAY: u64 = 86_400_000;
const TERM: u64 = 28 * DAY;
const STREAM: &str = "appointments/coordinator";
const SUBJECT: &str = "riverside-food-coop/role/coordinator";

type Reader = HeadCheckpoints<MemoryCheckpointStore>;

fn step(n: u8, title: &str) {
    println!("\n\x1b[1m{n}. {title}\x1b[0m");
}

fn note(s: impl AsRef<str>) {
    println!("   {}", s.as_ref());
}

fn pid(s: &str) -> PrincipalId {
    PrincipalId::new(s).expect("a principal")
}

fn tid(n: u64) -> TermId {
    TermId::new(format!("term-{n}")).expect("a term id")
}

fn sign(sk: &SigningKey, bytes: &[u8]) -> Vec<u8> {
    mycelium_core::tls::sign_bytes(sk, bytes).to_vec()
}

/// Who held term `n` (terms 1..=6).
fn holder(n: u64) -> &'static str {
    match n {
        1 => "amara",
        2 | 3 => "bea",
        4 => "chidi",
        _ => "dana", // 5 and 6
    }
}

/// The members' assembly: its node identity, the key it signed with before its annual meeting rotated
/// it, and the key it signs with since.
struct Assembly {
    node: NodeId,
    issuer: IssuerId,
    first_key: SigningKey,
    second_key: SigningKey,
}

/// What the assembly published: each appointment as a signed record, and the signed head on the stream
/// that points at it.
struct Published {
    records: Vec<SignedRecord>,
    heads: Vec<SignedHead>,
}

impl Assembly {
    fn new() -> Self {
        let node = NodeId::new("10.20.0.1", 7311).expect("a node id");
        Self {
            issuer: IssuerId::for_node(&node),
            node,
            first_key: SigningKey::from_bytes(&[11u8; 32]),
            second_key: SigningKey::from_bytes(&[13u8; 32]),
        }
    }

    /// The assembly's keys as a member's node holds them: both retained, and `revoked` revoked.
    fn key_view(&self, revoked: &[&SigningKey]) -> HashMap<NodeId, MemberKeys> {
        let keys = MemberKeys {
            retained: vec![self.first_key.verifying_key().to_bytes(), self.second_key.verifying_key().to_bytes()],
            revoked: revoked.iter().map(|k| k.verifying_key().to_bytes()).collect::<HashSet<_>>(),
        };
        HashMap::from([(self.node.clone(), keys)])
    }

    /// Terms 1..=6, four weeks each. Terms 1–3 were signed under the first key, 4–6 under the second;
    /// each head chains from the assembly's own previous head.
    fn publish(&self) -> Published {
        let mut records = Vec::new();
        let mut heads: Vec<SignedHead> = Vec::new();
        for n in 1..=6u64 {
            let sk = if n <= 3 { &self.first_key } else { &self.second_key };
            let term = TermRecord { holder: pid(holder(n)), term: tid(n), started_ms: n * TERM, ended_ms: (n + 1) * TERM };
            let body = serde_json::to_vec(&term).expect("a term record serialises");
            let record = KnowledgeRecord::new(self.issuer.clone(), RecordKind::Claim, n * TERM, SUBJECT, body, vec![])
                .expect("a well-formed appointment");
            let head = Head {
                issuer: self.issuer.clone(),
                stream: STREAM.into(),
                record: record.id().clone(),
                seq: n,
                prev: heads.last().map(|h| h.head.digest()),
            };
            heads.push(SignedHead { signature: sign(sk, &head.canonical_bytes()), head });
            records.push(SignedRecord { signature: sign(sk, &record.canonical_bytes()), record });
        }
        Published { records, heads }
    }
}

/// A member's node: a knowledge store of verified records, and a reader holding checkpoints on streams.
struct MemberNode {
    store: KnowledgeStore,
    reader: Reader,
}

impl MemberNode {
    fn new() -> Self {
        Self { store: KnowledgeStore::new(), reader: HeadCheckpoints::open(MemoryCheckpointStore::new()).expect("a reader") }
    }

    /// Verify and keep each record (attributed to the assembly under a key this node holds).
    fn receive_records<'a>(&mut self, a: &Assembly, records: impl IntoIterator<Item = &'a SignedRecord>) {
        let keys = a.key_view(&[]);
        for r in records {
            let at = self.store.put_signed(r.clone(), &keys, &TrustedExternalIssuers::new());
            assert!(at.is_ok(), "the assembly's record verifies: {at:?}");
        }
    }

    /// Offer heads in order, as gossip would deliver them; each must advance the checkpoint.
    fn receive_heads(&mut self, a: &Assembly, heads: &[SignedHead]) {
        let keys = a.key_view(&[]);
        for h in heads {
            let v = self.reader.offer(h, heads, &keys, &TrustedExternalIssuers::new());
            assert!(matches!(v, HeadVerdict::Advanced { .. }), "the reader advances: {v:?}");
        }
    }

    /// An appointment as this node can fetch it: the assembly's record on the coordinator role, decoded.
    /// `None` for anything else — a record that is not an appointment is a gap, never a term.
    fn appointment(&self, id: &RecordId) -> Option<TermRecord> {
        let r = self.store.get(id)?;
        if r.subject() != SUBJECT || r.kind() != RecordKind::Claim {
            return None;
        }
        serde_json::from_slice(r.body()).ok()
    }

    /// The history this node's reader supports, given whatever heads are presented to it.
    fn history(&self, a: &Assembly, presented: &[SignedHead], keys: &HashMap<NodeId, MemberKeys>) -> TermHistory {
        let stream = AppointmentStream::from_reader(&self.reader, &a.issuer, STREAM);
        history_from_appointment_stream(
            &stream,
            presented,
            |id| self.appointment(id),
            keys,
            &TrustedExternalIssuers::new(),
            StreamOrigin::Unknown,
        )
    }
}

fn show(name: &str, e: &StrictEligibility) {
    note(format!("{name}: consecutive terms → {:?}", e.consecutive.as_ref().expect("configured")));
}

fn main() {
    println!("\x1b[1mStrict eligibility — a food co-op's rotating coordinator\x1b[0m");
    let rules = IncumbencyRules { max_consecutive_terms: Some(2), ..IncumbencyRules::default() };
    let now = 7 * TERM; // the start of term 7: who may coordinate it?
    note("the members' rule: nobody coordinates for more than two terms in a row");
    note("terms 1–6: amara · bea · bea · chidi · dana · dana — term 7 is being filled");

    let assembly = Assembly::new();
    let published = assembly.publish();
    let keys = assembly.key_view(&[]);

    step(1, "the members' assembly publishes six appointments; the pantry's node follows the stream");
    let mut pantry = MemberNode::new();
    pantry.receive_records(&assembly, &published.records);
    pantry.receive_heads(&assembly, &published.heads);
    let cp = pantry.reader.checkpoint(&assembly.issuer, STREAM).expect("a checkpoint");
    note(format!("six signed appointment records verified; the reader's checkpoint is head {}", cp.seq));
    assert_eq!(cp.seq, 6, "the reader holds the newest head");
    let history = pantry.history(&assembly, &published.heads, &keys);
    note(format!("coverage the source established: {:?}", history.coverage()));
    assert!(history.coverage().from_genesis && history.coverage().through_head, "the walk reached the role's first head");
    assert_eq!(history.coverage().continuous_suffix, 6, "every term vouched for");

    step(2, "bea, who coordinated terms 2–3 and has rested since, is eligible");
    let bea = eligible_strict(&rules, &history, &pid("bea"), now);
    show("bea", &bea);
    assert_eq!(bea.verdict(), RuleVerdict::Eligible);
    note("her run of two was broken by chidi inside the verified history — decided, in her favour.");

    step(3, "dana, who coordinated terms 5 and 6, is not eligible for a third in a row");
    let dana = eligible_strict(&rules, &history, &pid("dana"), now);
    show("dana", &dana);
    assert_eq!(dana.verdict(), RuleVerdict::Ineligible(Ineligible::ConsecutiveTerms { served: 2, limit: 2 }));
    note("two in a row meets the limit; someone else learns the role this time.");

    step(4, "unknown is never eligible");
    note("(a) the bakery's node follows the same stream, but term 5's appointment record never reached it");
    let mut bakery = MemberNode::new();
    bakery.receive_records(&assembly, published.records.iter().enumerate().filter(|(i, _)| *i != 4).map(|(_, r)| r));
    bakery.receive_heads(&assembly, &published.heads);
    let gapped = bakery.history(&assembly, &published.heads, &keys);
    note(format!("coverage: {:?}", gapped.coverage()));
    assert_eq!(gapped.coverage().continuous_suffix, 1, "only term 6, after the gap, is vouched for");
    let held: Vec<TermRecord> = (1..=6).filter(|n| *n != 5).map(|n| bakery.appointment(&published.heads[n as usize - 1].head.record).expect("held")).collect();
    let lenient = eligible(&rules, &held, &pid("dana"), now);
    note(format!("lenient check over the records it holds: {lenient:?} — one visible term, so yes"));
    assert!(lenient.is_ok(), "the lenient check is fooled by the missing record");
    let dana_gapped = eligible_strict(&rules, &gapped, &pid("dana"), now);
    show("dana (strict)", &dana_gapped);
    assert!(matches!(dana_gapped.verdict(), RuleVerdict::Unknown(_)), "her run reaches the gap: an earlier term may extend it");
    let bea_gapped = eligible_strict(&rules, &gapped, &pid("bea"), now);
    show("bea (strict)", &bea_gapped);
    assert_eq!(bea_gapped.verdict(), RuleVerdict::Eligible, "a gap leaves unknown only what it could change");
    note("the gap cannot change bea's answer (her run is zero at the head), so it stays decided.");

    bakery.receive_records(&assembly, [&published.records[4]]);
    let restored = bakery.history(&assembly, &published.heads, &keys);
    let dana_restored = eligible_strict(&rules, &restored, &pid("dana"), now);
    show("dana, once term 5's record arrives", &dana_restored);
    assert!(matches!(dana_restored.verdict(), RuleVerdict::Ineligible(Ineligible::ConsecutiveTerms { .. })));

    note("(b) the orchard's node has just joined: every head and record is presented to it, but its");
    note("    reader holds no checkpoint for the stream yet");
    let mut orchard = MemberNode::new();
    orchard.receive_records(&assembly, &published.records);
    let fresh = orchard.history(&assembly, &published.heads, &keys);
    note(format!("coverage: {:?}", fresh.coverage()));
    assert_eq!(fresh.coverage().continuous_suffix, 0, "the head is the reader's own — presented heads do not supply it");
    let bea_fresh = eligible_strict(&rules, &fresh, &pid("bea"), now);
    show("bea (orchard)", &bea_fresh);
    assert!(matches!(bea_fresh.verdict(), RuleVerdict::Unknown(_)), "no checkpoint: nothing is current, so nothing is decided");
    note("a presenter cannot make the history look current; the orchard decides once its reader advances.");

    step(5, "the assembly's first signing key is revoked (a lost laptop)");
    let after_revocation = assembly.key_view(&[&assembly.first_key]);
    let history = pantry.history(&assembly, &published.heads, &after_revocation);
    note(format!("pantry (checkpoint on head 6, under the current key): {:?}", history.coverage()));
    assert!(history.coverage().from_genesis && history.coverage().through_head, "terms 1–3 still count");
    assert_eq!(history.coverage().continuous_suffix, 6);
    assert_eq!(eligible_strict(&rules, &history, &pid("dana"), now), dana, "the same verdict after revocation");
    note("heads 1–3 were signed under the revoked key, but head 4 — under the current key — committed to");
    note("them by digest, so the role's history does not end at the revocation.");
    let mut early = MemberNode::new();
    early.receive_records(&assembly, &published.records);
    early.receive_heads(&assembly, &published.heads[..3]);
    let early_history = early.history(&assembly, &published.heads, &after_revocation);
    note(format!("a reader still on head 3 (under the revoked key): {:?}", early_history.coverage()));
    assert_eq!(early_history.coverage().continuous_suffix, 0, "a checkpoint under a revoked key vouches for nothing");
    assert!(matches!(eligible_strict(&rules, &early_history, &pid("bea"), now).verdict(), RuleVerdict::Unknown(_)));

    step(6, "bea takes term 7 once she has read the handover journal");
    let mut journal = HandoverJournal::new();
    journal.record(JournalEntry {
        term: tid(6),
        author: pid("dana"),
        at_ms: 7 * TERM - DAY,
        kind: EntryKind::Observation,
        text: "the orchard's Saturday drop moved to 9am; the dairy invoices are two weeks behind".into(),
    });
    let mut incoming = Successor::new();
    let unread = ready(&incoming, &journal, &bea);
    note(format!("before reading: {:?}", unread.as_ref().map_err(ToString::to_string)));
    assert!(matches!(unread, Err(NotReadyStrict::Unread(_))), "reading comes first");
    incoming.read_through(journal.len());
    let ok = ready(&incoming, &journal, &bea);
    note(format!("after reading: {:?}", ok.as_ref().map_err(ToString::to_string)));
    assert!(ok.is_ok(), "read, and eligible on history the source vouched for");
    let unknown = ready(&incoming, &journal, &bea_fresh);
    assert!(matches!(unknown, Err(NotReadyStrict::HistoryUnknown(_))), "an unknown verdict never makes anyone ready");

    step(7, "what this demonstration does not establish");
    note("· that the assembly published every appointment there was — the source vouches for the chain it holds");
    note("· that a reader has heard the newest head — \"current\" means as of this reader's checkpoint");
    note("· gossip delivery — records and heads are handed over in-process here");

    println!("\nAll assertions passed.");
}
