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
    static VALS: RefCell<HashMap<Bytes, Val>> = RefCell::new(HashMap::new());
    static READS: RefCell<HashMap<ReadKey, Option<Top>>> = RefCell::new(HashMap::new());
    static PREPARES: RefCell<HashMap<(Acc, u64, Node, u64), Promise>> = RefCell::new(HashMap::new());
    static ACCEPTS: RefCell<HashMap<AcceptKey, Option<Acc>>> = RefCell::new(HashMap::new());
    static CHOICES: RefCell<HashMap<ChoiceKey, Choice>> = RefCell::new(HashMap::new());
    static FILTERS: RefCell<HashMap<Option<Vec<Node>>, Filter>> = RefCell::new(HashMap::new());
}

type ChoiceKey = (Val, Val, Vec<(u64, Val)>, u64, Option<Vec<Made>>);
type AcceptKey = (Acc, u64, Val, Node, u64, Made);

/// The voters a proposal counts, as node hashes; `None`: no filter.
pub type Filter = Option<std::rc::Rc<[u64]>>;

type ReadKey = (Vec<(u64, Val)>, Vec<Val>, u64);

/// The engine's envelope bytes for `v` (`Envelope::new(..).encode()`), cached per thread.
pub fn bytes(v: Val) -> Bytes {
    BYTES.with(|cache| {
        cache.borrow_mut().entry(v).or_insert_with(|| {
            let lease = v.expires.map(|at| (1, at));
            let token = (u64::from(v.proposer) << 8) | u64::from(v.call);
            let env = engine::envelope(SLOT, &[v.proposer, v.call], lease, v.lineage, &node_id(v.proposer), token);
            VALS.with(|back| back.borrow_mut().insert(env.clone(), v));
            env
        }).clone()
    })
}

/// The model's value for envelope bytes the engine handed back. The engine only ever returns bytes it was
/// given, and every envelope it is given went through [`bytes`] on this thread.
fn val(env: &Bytes) -> Val {
    VALS.with(|back| back.borrow().get(env).copied())
        .expect("the engine returned an envelope the model never gave it")
}

/// The model's node for a proposer hash the engine handed back.
fn node(hash: u64) -> Node {
    (0..=Node::MAX).find(|n| id_hash(*n) == hash).expect("the engine returned a proposer the model never named")
}

/// The predecessor's group name in the engine, and the successor's.
pub const PREDECESSOR: &str = "pred";
pub const SUCCESSOR: &str = "succ";

/// Where an acceptance was made — the engine's `MadeIn` (critique plan CR30), over the model's two groups.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Made {
    /// Not known: a node that does not record it (before 2.41.0, or restarted from a record without it).
    #[default]
    Unknown,
    /// Outside any electorate group.
    NoGroup,
    /// In the predecessor.
    Predecessor,
    /// In the successor.
    Successor,
}

impl Made {
    fn to_engine(self) -> engine::Made {
        match self {
            Made::Unknown => engine::Made::Unknown,
            Made::NoGroup => engine::Made::NoGroup,
            Made::Predecessor => engine::Made::Group(PREDECESSOR.to_string()),
            Made::Successor => engine::Made::Group(SUCCESSOR.to_string()),
        }
    }

    fn of_engine(m: &engine::Made) -> Self {
        match m {
            engine::Made::Unknown => Made::Unknown,
            engine::Made::NoGroup => Made::NoGroup,
            engine::Made::Group(g) if g == PREDECESSOR => Made::Predecessor,
            engine::Made::Group(g) if g == SUCCESSOR => Made::Successor,
            engine::Made::Group(g) => panic!("the engine returned a group the model never named: {g}"),
        }
    }
}

/// One acceptor's memory of the slot.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Acc {
    pub promised:    u64,
    pub promised_to: Option<Node>,
    pub accepted:    Option<(u64, Val)>,
    /// Where `accepted` was made, as the engine keeps it.
    pub made_in:     Made,
}

impl Acc {
    fn to_engine(&self) -> engine::Acceptor {
        engine::Acceptor {
            promised:    self.promised,
            promised_to: self.promised_to.map(id_hash),
            accepted:    self.accepted.map(|(b, v)| (b, bytes(v))),
            made_in:     self.made_in.to_engine(),
        }
    }

    /// The acceptor as the engine left it — every field read back from the engine's state, none kept from the
    /// model's.
    fn from_engine(after: &engine::Acceptor) -> Self {
        Self {
            promised:    after.promised,
            promised_to: after.promised_to.map(node),
            accepted:    after.accepted.as_ref().map(|(b, env)| (*b, val(env))),
            made_in:     Made::of_engine(&after.made_in),
        }
    }
}

type Promise = (Acc, Option<(Option<(u64, Val)>, Made)>);

/// The engine's `prepare_slot_tagged`: the acceptor afterwards and, when it promised, what it had accepted and
/// where that was made. Cached per thread, as every bridge function is: the engine's function is pure, and runs
/// once per distinct input.
pub fn prepare_tagged(acc: &Acc, ballot: u64, proposer: Node, floor: u64) -> Promise {
    let key = (acc.clone(), ballot, proposer, floor);
    if let Some(hit) = PREPARES.with(|cache| cache.borrow().get(&key).cloned()) { return hit; }
    let (after, answer) = engine::prepare(&acc.to_engine(), SLOT, ballot, id_hash(proposer), floor);
    // Every part of the answer is the engine's: the acceptor it left behind, what its promise reports, and where
    // that was made.
    let out = match answer {
        engine::Prepared::Promised(reported, made_in) =>
            (Acc::from_engine(&after), Some((reported.map(|(b, env)| (b, val(&env))), Made::of_engine(&made_in)))),
        engine::Prepared::Refused(_) => (Acc::from_engine(&after), None),
    };
    PREPARES.with(|cache| cache.borrow_mut().insert(key, out.clone()));
    out
}

/// [`prepare_tagged`] without where the report was made.
pub fn prepare(acc: &Acc, ballot: u64, proposer: Node, floor: u64) -> (Acc, Option<Option<(u64, Val)>>) {
    let (after, answer) = prepare_tagged(acc, ballot, proposer, floor);
    (after, answer.map(|(reported, _)| reported))
}

/// The engine's `claim_envelope_in`: the acceptor afterwards when it accepted `value` at `ballot`, the acceptance
/// recorded as made in `made_in`.
pub fn accept_in(acc: &Acc, ballot: u64, value: Val, proposer: Node, floor: u64, made_in: Made) -> Option<Acc> {
    let key = (acc.clone(), ballot, value, proposer, floor, made_in);
    if let Some(hit) = ACCEPTS.with(|cache| cache.borrow().get(&key).cloned()) { return hit; }
    let out = engine::accept(&acc.to_engine(), SLOT, ballot, &bytes(value), id_hash(proposer), floor, &made_in.to_engine())
        .map(|after| Acc::from_engine(&after));
    ACCEPTS.with(|cache| cache.borrow_mut().insert(key, out.clone()));
    out
}

/// [`accept_in`] for a proposal outside any electorate group — the engine's cluster-scoped lock.
pub fn accept(acc: &Acc, ballot: u64, value: Val, proposer: Node, floor: u64) -> Option<Acc> {
    accept_in(acc, ballot, value, proposer, floor, Made::NoGroup)
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
    choose(own, current, reports, ended, None)
}

/// The engine's phase-1 rule for a carry ballot into the successor that asked where each report was made
/// (`carry_hears` first): `made_in[i]` is where `reports[i]` was made; the lineage is the successor and the
/// predecessor.
pub fn phase1_carry(own: Val, current: Val, reports: &[(u64, Val)], made_in: &[Made], ended: u64) -> Choice {
    choose(own, current, reports, ended, Some(made_in))
}

fn choose(own: Val, current: Val, reports: &[(u64, Val)], ended: u64, made_in: Option<&[Made]>) -> Choice {
    let key: ChoiceKey = (own, current, reports.to_vec(), ended, made_in.map(<[Made]>::to_vec));
    if let Some(hit) = CHOICES.with(|cache| cache.borrow().get(&key).copied()) { return hit; }
    let reported: Vec<(u64, Bytes)> = reports.iter().map(|(b, v)| (*b, bytes(*v))).collect();
    let made: Option<Vec<engine::Made>> = made_in.map(|m| m.iter().map(|x| x.to_engine()).collect());
    let hears = made.as_deref().map(|m| engine::CarryHears { made_in: m, group: SUCCESSOR, lineage: &[SUCCESSOR, PREDECESSOR] });
    let out = match engine::phase1(SLOT, &bytes(own), &bytes(current), &reported, ended, hears.as_ref()) {
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
