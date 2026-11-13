//! The bridge between a model's small, hashable state and the engine's decision functions
//! (`mycelium::model_internals`). Every function here converts and calls; none decides.

use std::cell::RefCell;
use std::collections::HashMap;

use bytes::Bytes;
use mycelium::model_internals as engine;
use mycelium::NodeId;

/// The one slot every model decides.
pub const SLOT: &str = "lock/model";

/// A node of the model: an index.
pub type Node = u8;

/// A decision envelope, as the model holds it: who first proposed it, in which of its calls, the lineage (its
/// first ballot) and the lease's wall-clock end (`None`: permanent). Two envelopes are the same decision exactly
/// when all four agree — the engine's `Envelope::identity` with the term.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Val {
    pub proposer: Node,
    pub call:     u8,
    pub lineage:  u64,
    pub expires:  Option<u64>,
}

impl Val {
    /// The caller's value inside the envelope — `{holder}:{nonce}` in `distributed_lock`: unique per call.
    pub fn raw(&self) -> (Node, u8) {
        (self.proposer, self.call)
    }
}

/// The node's identity in the engine.
pub fn node_id(n: Node) -> NodeId {
    NodeId::new("127.0.0.1", 7000 + u16::from(n)).expect("a loopback address is a node id")
}

/// `NodeId::id_hash` — what the engine's acceptor and vote filter compare.
pub fn id_hash(n: Node) -> u64 {
    thread_local! { static HASHES: RefCell<HashMap<Node, u64>> = RefCell::new(HashMap::new()); }
    HASHES.with(|cache| *cache.borrow_mut().entry(n).or_insert_with(|| node_id(n).id_hash()))
}

thread_local! {
    static BYTES: RefCell<HashMap<Val, Bytes>> = RefCell::new(HashMap::new());
    static READS: RefCell<HashMap<ReadKey, Option<Top>>> = RefCell::new(HashMap::new());
    static PREPARES: RefCell<HashMap<(Acc, u64, Node, u64), Promise>> = RefCell::new(HashMap::new());
    static ACCEPTS: RefCell<HashMap<AcceptKey, Option<Acc>>> = RefCell::new(HashMap::new());
    static CHOICES: RefCell<HashMap<ChoiceKey, Choice>> = RefCell::new(HashMap::new());
    static FILTERS: RefCell<HashMap<Option<Vec<Node>>, Filter>> = RefCell::new(HashMap::new());
}

type ChoiceKey = (Val, Val, Vec<(u64, Val)>, u64);
type AcceptKey = (Acc, u64, Val, Node, u64);

/// The voters a proposal counts, as node hashes; `None`: no filter.
pub type Filter = Option<std::rc::Rc<[u64]>>;

type ReadKey = (Vec<(u64, Val)>, Vec<Val>, u64);

/// The engine's envelope bytes for `v` (`Envelope::new(..).encode()`), cached per thread.
pub fn bytes(v: Val) -> Bytes {
    BYTES.with(|cache| {
        cache.borrow_mut().entry(v).or_insert_with(|| {
            let lease = v.expires.map(|at| (1, at));
            let token = (u64::from(v.proposer) << 8) | u64::from(v.call);
            engine::envelope(SLOT, &[v.proposer, v.call], lease, v.lineage, &node_id(v.proposer), token)
        }).clone()
    })
}

/// One acceptor's memory of the slot.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Acc {
    pub promised:    u64,
    pub promised_to: Option<Node>,
    pub accepted:    Option<(u64, Val)>,
}

impl Acc {
    fn to_engine(&self) -> engine::Acceptor {
        engine::Acceptor {
            promised:    self.promised,
            promised_to: self.promised_to.map(id_hash),
            accepted:    self.accepted.map(|(b, v)| (b, bytes(v))),
        }
    }
}

type Promise = (Acc, Option<Option<(u64, Val)>>);

/// The engine's `prepare_slot`: the acceptor afterwards and, when it promised, what it had accepted. Cached per
/// thread, as every bridge function is: the engine's function is pure, and runs once per distinct input.
pub fn prepare(acc: &Acc, ballot: u64, proposer: Node, floor: u64) -> Promise {
    let key = (acc.clone(), ballot, proposer, floor);
    if let Some(hit) = PREPARES.with(|cache| cache.borrow().get(&key).cloned()) { return hit; }
    let (after, answer) = engine::prepare(&acc.to_engine(), SLOT, ballot, id_hash(proposer), floor);
    let out = match answer {
        engine::Prepared::Promised(_) => {
            assert_eq!((after.promised, after.promised_to), (ballot, Some(id_hash(proposer))));
            (Acc { promised: after.promised, promised_to: Some(proposer), accepted: acc.accepted }, Some(acc.accepted))
        }
        engine::Prepared::Refused(_) => (acc.clone(), None),
    };
    PREPARES.with(|cache| cache.borrow_mut().insert(key, out.clone()));
    out
}

/// The engine's `claim_envelope`: the acceptor afterwards when it accepted `value` at `ballot`.
pub fn accept(acc: &Acc, ballot: u64, value: Val, proposer: Node, floor: u64) -> Option<Acc> {
    let key = (acc.clone(), ballot, value, proposer, floor);
    if let Some(hit) = ACCEPTS.with(|cache| cache.borrow().get(&key).cloned()) { return hit; }
    let out = engine::accept(&acc.to_engine(), SLOT, ballot, &bytes(value), id_hash(proposer), floor).map(|after| {
        assert_eq!(after.accepted, Some((ballot, bytes(value))));
        Acc { promised: after.promised, promised_to: Some(proposer), accepted: Some((ballot, value)) }
    });
    ACCEPTS.with(|cache| cache.borrow_mut().insert(key, out.clone()));
    out
}

/// What a proposer does after its promise quorum answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Keep,
    Adopt(u64, Val),
    Blocked,
}

/// The engine's phase-1 rule (`set_aside_finished`, then `phase1_choice`) over the quorum's reports.
pub fn phase1(own: Val, current: Val, reports: &[(u64, Val)], ended: u64) -> Choice {
    let key: ChoiceKey = (own, current, reports.to_vec(), ended);
    if let Some(hit) = CHOICES.with(|cache| cache.borrow().get(&key).copied()) { return hit; }
    let reported: Vec<(u64, Bytes)> = reports.iter().map(|(b, v)| (*b, bytes(*v))).collect();
    let out = match engine::phase1(SLOT, &bytes(own), &bytes(current), &reported, ended) {
        engine::Phase1::Keep => Choice::Keep,
        engine::Phase1::Blocked => Choice::Blocked,
        engine::Phase1::Adopt(ballot, env) => {
            let adopted = reports.iter().find(|(b, v)| *b == ballot && bytes(*v) == env)
                .expect("the engine adopts one of the reports it was given");
            Choice::Adopt(ballot, adopted.1)
        }
    };
    CHOICES.with(|cache| cache.borrow_mut().insert(key, out));
    out
}

/// The top decision a node holds, as the engine's `read_slot` reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Top {
    pub ballot: u64,
    pub value:  Val,
    /// Released, or its lease is past on the reader's wall clock.
    pub ended:  bool,
}

/// The engine's `read_slot` over a store holding `records` and a release marker for each of `released`, at
/// `now` on the reader's wall clock. Cached per thread: the function is pure.
pub fn read(records: &[(u64, Val)], released: &[Val], now: u64) -> Option<Top> {
    if records.is_empty() { return None; }
    let key: ReadKey = (records.to_vec(), released.to_vec(), now);
    if let Some(hit) = READS.with(|cache| cache.borrow().get(&key).copied()) { return hit; }
    let recs: Vec<(u64, Bytes)> = records.iter().map(|(b, v)| (*b, bytes(*v))).collect();
    let marks: Vec<Bytes> = released.iter().map(|v| bytes(*v)).collect();
    let top = engine::read_top(SLOT, &recs, &marks, now).map(|t| {
        let value = records.iter().find(|(b, _)| *b == t.ballot).expect("the top is one of the records").1;
        assert_eq!(bytes(value), t.envelope, "the engine's top record is the model's");
        Top { ballot: t.ballot, value, ended: t.ended }
    });
    READS.with(|cache| cache.borrow_mut().insert(key, top));
    top
}

/// An electorate of these members, as the engine declares one.
fn electorate(members: &[Node]) -> mycelium::ElectorateDecl {
    let mut decl = mycelium::ElectorateDecl::default();
    decl.group = "model".to_string();
    decl.epoch = 1;
    decl.members = members.iter().map(|n| node_id(*n)).collect();
    decl
}

/// The electorate's quorum — `ElectorateDecl::quorum`, a strict majority of its members.
pub fn quorum(members: &[Node]) -> usize {
    thread_local! { static QUORUMS: RefCell<HashMap<Vec<Node>, usize>> = RefCell::new(HashMap::new()); }
    QUORUMS.with(|cache| *cache.borrow_mut().entry(members.to_vec()).or_insert_with(|| electorate(members).quorum()))
}

/// Whether `n` is a member — `ElectorateDecl::contains`.
pub fn is_member(members: &[Node], n: Node) -> bool {
    thread_local! { static MEMBERS: RefCell<HashMap<(Vec<Node>, Node), bool>> = RefCell::new(HashMap::new()); }
    MEMBERS.with(|cache| {
        *cache.borrow_mut().entry((members.to_vec(), n)).or_insert_with(|| electorate(members).contains(&node_id(n)))
    })
}

/// The voters a proposal counts (the engine's `electorate_vote_filter`), as node hashes; `None`: no filter.
pub fn vote_filter(roster: Option<&[Node]>) -> Filter {
    let key = roster.map(<[Node]>::to_vec);
    if let Some(hit) = FILTERS.with(|cache| cache.borrow().get(&key).cloned()) { return hit; }
    let hashes: Option<Vec<u64>> = roster.map(|r| r.iter().map(|n| id_hash(*n)).collect());
    let out = engine::vote_filter(None, hashes.as_deref()).map(std::rc::Rc::from);
    FILTERS.with(|cache| cache.borrow_mut().insert(key, out.clone()));
    out
}

/// Whether a vote or a promise from `voter` is counted — the engine's inline `trust_set.contains(&voter.id_hash())`.
pub fn counted(filter: Option<&[u64]>, voter: Node) -> bool {
    filter.is_none_or(|f| f.contains(&id_hash(voter)))
}

/// A set of nodes as a bit mask.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Set(pub u16);

impl Set {
    pub fn with(self, n: Node) -> Self { Set(self.0 | (1 << n)) }
    pub fn has(self, n: Node) -> bool { self.0 & (1 << n) != 0 }
    pub fn len(self) -> usize { self.0.count_ones() as usize }
    pub fn is_empty(self) -> bool { self.0 == 0 }
}
