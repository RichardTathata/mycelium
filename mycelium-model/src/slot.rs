//! **One slot, decided again and again**: the acceptor, slot reuse after a release or a lapsed lease, and the
//! lease's wall-clock end under a skew bound δ.
//!
//! Nodes exchange the engine's messages through a network that may delay, reorder, duplicate or lose any of
//! them (a message once sent can be delivered at any later step, any number of times, or never). Each node has
//! a wall clock; clocks only advance, and never differ by more than `skew` ticks. A proposer may give a ballot
//! up at any step (a timeout) and draws any ballot above its floor, up to one above the highest any node has
//! promised — the shared ballot key is gossip, so a proposer may hold any earlier value of it.
//!
//! A configuration chooses its grain ([`SlotModel::whole_ballots`]): every message its own step, or a ballot as
//! one step with everything else — each clock, each record or marker learned, each release — still its own.
//!
//! **The floor here is weaker than the engine's.** A node refuses, and draws above, the ballot of its top
//! record when that decision is over; the engine also holds the decided floor (`consensus/decided/`), which
//! travels as a key of its own and is not modelled. The model's acceptors therefore answer more than the
//! engine's do, and the invariants hold without that floor.

use std::collections::BTreeSet;

use stateright::{Model, Property};

use crate::shared::{self, Acc, Choice, Node, Set, Val};

/// The configuration of a run: the bound, and the planted faults.
#[derive(Clone, Debug)]
pub struct SlotModel {
    /// The electorate: nodes `0..members`.
    pub members:    u8,
    /// Whether one node outside the electorate (node `members`) answers: it promises every prepare, having
    /// accepted nothing, and votes for every proposal. The proposer's vote filter is what keeps its answers out.
    pub outsider:   bool,
    /// Nodes `0..proposers` propose.
    pub proposers:  u8,
    /// Calls (`distributed_lock`) per proposer.
    pub calls:      u8,
    /// The highest ballot drawn.
    pub max_ballot: u64,
    /// A lease, in ticks.
    pub lease:      u64,
    /// δ: the most two wall clocks differ by, in ticks.
    pub skew:       u64,
    /// How long before its deadline a holder stops. As designed (critique plan row CR11): δ.
    pub margin:     u64,
    /// The last tick of any clock.
    pub horizon:    u64,
    /// Whether a holder may release.
    pub release:    bool,
    /// Whether a ballot is one step: the proposer and the rest of a quorum run prepare, promise, proposal, vote
    /// and commit with nothing in between, so ballots never overlap. What stays interleaved is everything else —
    /// each clock, each node learning a record or a marker, each release.
    pub whole_ballots: bool,
    /// **Planted fault** — votes and promises are counted from any node, the electorate's roster ignored.
    pub count_outside_votes: bool,
    /// Whether a call draws a further ballot after one fails (and, with it, whether a refused proposal is
    /// answered with what the refuser holds — `PromiseTerm` — for the next ballot to carry). Without it a call
    /// is one ballot.
    pub retries:    bool,
}

impl SlotModel {
    /// **The acceptor, inside one life**: three members and a node outside the electorate, two proposers with one
    /// call each, ballots to 2, every message its own step, a failed ballot retried and a refusal's report
    /// adopted. No clock and no release: nothing ends a decision.
    pub fn acceptor() -> Self {
        Self {
            members: 3, outsider: true, proposers: 2, calls: 1, max_ballot: 2, lease: 1, skew: 0, margin: 0,
            horizon: 0, release: false, retries: true, whole_ballots: false, count_outside_votes: false,
        }
    }

    /// **Slot reuse after a lapsed lease**, every message its own step: [`acceptor`](Self::acceptor) with a
    /// one-tick lease on one shared clock that runs to tick 2, and a call of one ballot.
    pub fn reuse_after_lapse() -> Self {
        Self { horizon: 2, retries: false, ..Self::acceptor() }
    }

    /// **Slot reuse after a release**, every message its own step: [`acceptor`](Self::acceptor) with a holder
    /// that may release, and a call of one ballot.
    pub fn reuse_after_release() -> Self {
        Self { release: true, retries: false, ..Self::acceptor() }
    }

    /// **Slot reuse, three lives**: three proposers with one call each, ballots to 3, a one-tick lease on one
    /// shared clock that runs to tick 2, releases — each ballot one step.
    pub fn reuse() -> Self {
        Self {
            proposers: 3, max_ballot: 3, horizon: 2, release: true, retries: false, whole_ballots: true,
            ..Self::acceptor()
        }
    }

    /// **The lease, one tick from the edge**: [`lease`](Self::lease) with clocks up to δ = 3 ticks apart, a
    /// five-tick lease, clocks to tick 8, no releases — and a margin of **δ − 1 = 2**. On whole ticks the reader
    /// ends a lease at `now > expiry` and the holder stops at `now + margin < expiry`, both strict, so δ − 1 is
    /// the least margin that holds and δ − 2 breaks. This run and its planted twin (`margin: 1`) show that edge;
    /// [`lease`](Self::lease), with its margin of δ, sits one tick inside it and cannot.
    pub fn lease_edge() -> Self {
        Self { lease: 5, skew: 3, margin: 2, horizon: 8, release: false, ..Self::lease() }
    }

    /// **The lease's wall-clock end**: three members, two proposers with one call each, ballots to 3, a
    /// four-tick lease, every node its own clock, no two more than δ = 2 ticks apart, running to tick 6; the
    /// holder stops δ before its deadline and may release — each ballot one step.
    pub fn lease() -> Self {
        Self {
            members: 3, outsider: false, proposers: 2, calls: 1, max_ballot: 3, lease: 4, skew: 2, margin: 2,
            horizon: 6, release: true, retries: false, whole_ballots: true, count_outside_votes: false,
        }
    }

    /// The planted fault of the slot-reuse area: a vote counted from outside the electorate.
    pub fn with_outside_votes_counted(mut self) -> Self {
        self.count_outside_votes = true;
        self
    }

    /// The planted fault of the lease area: the lease ended with no margin — which is the engine as it stands
    /// until CR11 is built.
    pub fn with_no_margin(mut self) -> Self {
        self.margin = 0;
        self
    }

    fn nodes(&self) -> u8 { self.members }

    fn quorum(&self) -> usize { shared::quorum(&self.roster()) }

    fn roster(&self) -> Vec<Node> { (0..self.members).collect() }

    fn filter(&self) -> shared::Filter {
        let roster = self.roster();
        shared::vote_filter(if self.count_outside_votes { None } else { Some(&roster) })
    }
}

/// A message of the engine (`ConsensusMsg`), reduced to what a decision reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Msg {
    /// `PrepareTerm` / `PrepareIn`.
    Prepare { ballot: u64, from: Node },
    /// `PrepareAckTerm`: the promise, what the promiser had accepted, and the value of its live decision.
    Ack { ballot: u64, to: Node, voter: Node, accepted: Option<(u64, Val)>, live: Option<(Node, u8)> },
    /// `ProposeTerm` / `ProposeIn`.
    Propose { ballot: u64, from: Node, value: Val },
    /// `VoteForValue`: bound to the value.
    Vote { ballot: u64, voter: Node, value: Val },
    /// `PromiseTerm`: a refused proposal, reporting what the refuser holds. `moves`: it names a ballot at or
    /// above the proposal's, which is when the proposer acts on it.
    Refuse { ballot: u64, to: Node, from: Node, accepted: Option<(u64, Val)>, moves: bool },
    /// `CommitTerm`: the decision record.
    Commit { ballot: u64, value: Val },
    /// A release marker, gossiped.
    Marker { value: Val },
}

/// Where a proposer is in a call.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    Idle,
    /// Between two ballots of one call.
    Between { own: Val, current: Val, adopted_from: u64, last: u64 },
    Preparing { own: Val, current: Val, adopted_from: u64, ballot: u64, promisers: Set, reports: Vec<(u64, Val)> },
    Proposing { own: Val, value: Val, adopted_from: u64, ballot: u64, voters: Set },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NodeState {
    pub clock:    u64,
    pub acc:      Acc,
    /// The decision records held, by ballot.
    pub records:  Vec<(u64, Val)>,
    /// The release markers held.
    pub released: Vec<Val>,
    pub calls:    u8,
    pub phase:    Phase,
    /// The guard this node was issued and has not released.
    pub guard:    Option<Val>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SlotState {
    pub nodes:   Vec<NodeState>,
    pub net:     BTreeSet<Msg>,
    /// Every guard ever issued.
    pub granted: BTreeSet<Val>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    /// One node's clock advances (`None`: the one shared clock, when δ = 0).
    Tick(Option<Node>),
    /// A call begins at this ballot.
    Begin(Node, u64),
    /// The call's next ballot.
    Retry(Node, u64),
    /// The ballot times out.
    TimeOut(Node),
    /// The call returns without a decision.
    Return(Node),
    /// The holder releases.
    Release(Node),
    /// The network delivers a message to a node.
    Deliver(Msg, Node),
    /// The node outside the electorate answers this proposer's prepare or proposal.
    Outsider(Node),
    /// A whole ballot of a new call, with these nodes as the rest of its quorum.
    Ballot(Node, u64, Set),
}

/// What a node reads of the slot on its own clock.
struct View {
    /// The ballot at or below which it refuses and above which it draws: `ConsensusEngine::floor` without the
    /// decided floor (see the module documentation).
    floor: u64,
    /// The ballot of its top record when that decision is over (`ConsensusEngine::ended`).
    ended: u64,
    /// Its live decision (`ConsensusEngine::live_digest`).
    live:  Option<Val>,
}

fn view(n: &NodeState) -> View {
    match shared::read(&n.records, &n.released, n.clock) {
        Some(top) if top.ended => View { floor: top.ballot, ended: top.ballot, live: None },
        Some(top) => View { floor: 0, ended: 0, live: Some(top.value) },
        None => View { floor: 0, ended: 0, live: None },
    }
}

/// Whether `n` acts as the holder: it was issued a guard, has not released it, and its own clock is before
/// the deadline less the margin.
pub fn holding(n: &NodeState, margin: u64) -> bool {
    n.guard.is_some_and(|g| g.expires.is_none_or(|at| n.clock.saturating_add(margin) < at))
}

impl SlotModel {
    fn start_ballot(&self, s: &mut SlotState, p: Node, own: Val, current: Val, adopted_from: u64, ballot: u64) {
        let node = &mut s.nodes[usize::from(p)];
        let v = view(node);
        // `superseded_by_live`: a live decision of another value ends the call.
        if v.live.is_some_and(|l| l.raw() != current.raw()) {
            node.phase = Phase::Idle;
            return;
        }
        let (acc, promised) = shared::prepare(&node.acc, ballot, p, v.floor);
        let Some(accepted) = promised else {
            node.phase = self.failed(own, current, adopted_from, ballot);
            return;
        };
        node.acc = acc;
        node.phase = Phase::Preparing {
            own, current, adopted_from, ballot,
            promisers: Set::default().with(p),
            reports: accepted.into_iter().collect(),
        };
        if self.quorum() <= 1 {
            self.finish_phase1(s, p);
        } else {
            s.net.insert(Msg::Prepare { ballot, from: p });
        }
    }

    fn finish_phase1(&self, s: &mut SlotState, p: Node) {
        let node = &mut s.nodes[usize::from(p)];
        let Phase::Preparing { own, current, adopted_from, ballot, ref reports, .. } = node.phase else { return };
        let v = view(node);
        let (value, adopted_from) = match shared::phase1(own, current, reports, v.ended) {
            Choice::Keep => (current, adopted_from),
            Choice::Adopt(at, adopted) => (adopted, at),
            Choice::Blocked => {
                node.phase = self.failed(own, current, adopted_from, ballot);
                return;
            }
        };
        if v.live.is_some_and(|l| l.raw() != value.raw()) {
            node.phase = Phase::Idle;
            return;
        }
        // The proposer's own vote passes the same claim as any acceptor's.
        let Some(acc) = shared::accept(&node.acc, ballot, value, p, v.floor) else {
            node.phase = self.failed(own, value, adopted_from, ballot);
            return;
        };
        node.acc = acc;
        node.phase = Phase::Proposing { own, value, adopted_from, ballot, voters: Set::default().with(p) };
        s.net.insert(Msg::Propose { ballot, from: p, value });
        if self.quorum() <= 1 { self.commit(s, p); }
    }

    /// `try_commit_if_ready` and `commit_decision`, then `lock_guard_after_commit`.
    fn commit(&self, s: &mut SlotState, p: Node) {
        let node = &mut s.nodes[usize::from(p)];
        let Phase::Proposing { own, value, ballot, .. } = node.phase else { return };
        node.phase = Phase::Idle;
        if view(node).live.is_some_and(|l| l.raw() != value.raw()) { return; }
        learn(node, ballot, value);
        s.net.insert(Msg::Commit { ballot, value });
        // `own_or_superseded`: the call succeeded only when what was decided is what it asked for; the guard
        // is issued when this node's top record is a live decision of that value, and not past the deadline.
        if value.raw() != own.raw() { return; }
        let Some(top) = shared::read(&node.records, &node.released, node.clock) else { return };
        if top.ended || top.value.raw() != own.raw() { return; }
        node.guard = Some(top.value);
        if holding(node, self.margin) {
            s.granted.insert(top.value);
        } else {
            node.guard = None;
        }
    }
}

impl SlotModel {
    /// A ballot failed: the call goes on to another, or — a call of one ballot — ends.
    fn failed(&self, own: Val, current: Val, adopted_from: u64, last: u64) -> Phase {
        if self.retries { Phase::Between { own, current, adopted_from, last } } else { Phase::Idle }
    }
}

fn learn(node: &mut NodeState, ballot: u64, value: Val) -> bool {
    if node.records.iter().any(|(b, _)| *b == ballot) { return false; }
    node.records.push((ballot, value));
    node.records.sort_unstable();
    true
}

impl Model for SlotModel {
    type State = SlotState;
    type Action = Action;

    fn init_states(&self) -> Vec<SlotState> {
        let node = NodeState {
            clock: 0, acc: Acc::default(), records: Vec::new(), released: Vec::new(), calls: 0,
            phase: Phase::Idle, guard: None,
        };
        vec![SlotState {
            nodes: vec![node; usize::from(self.nodes())],
            net: BTreeSet::new(),
            granted: BTreeSet::new(),
        }]
    }

    fn actions(&self, s: &SlotState, out: &mut Vec<Action>) {
        if self.skew == 0 {
            out.push(Action::Tick(None));
        } else {
            out.extend((0..self.nodes()).map(|n| Action::Tick(Some(n))));
        }
        // A ballot is drawn one above the shared ballot key, which is gossip: a proposer may hold any value of it
        // up to the highest ballot any node has promised, so it draws anything up to one above that.
        let top = s.nodes.iter().map(|n| n.acc.promised).max().unwrap_or(0).saturating_add(1).min(self.max_ballot);
        for p in 0..self.proposers {
            let node = &s.nodes[usize::from(p)];
            match &node.phase {
                Phase::Idle if node.calls < self.calls && self.whole_ballots => {
                    let others = (0u16..1 << self.members).map(Set)
                        .filter(|q| !q.has(p) && q.len() + 1 == self.quorum());
                    out.extend(others.flat_map(|q| (1..=top).map(move |b| Action::Ballot(p, b, q))));
                }
                Phase::Idle if node.calls < self.calls =>
                    out.extend((1..=top).map(|b| Action::Begin(p, b))),
                Phase::Idle => {}
                Phase::Between { last, .. } => {
                    out.extend((last + 1..=top).map(|b| Action::Retry(p, b)));
                    out.push(Action::Return(p));
                }
                // A call of one ballot that is this proposer's last ends in nothing a stalled ballot does not.
                Phase::Preparing { .. } | Phase::Proposing { .. } =>
                    if self.retries || node.calls < self.calls { out.push(Action::TimeOut(p)) },
            }
            if self.release && node.guard.is_some() { out.push(Action::Release(p)); }
            if self.outsider && matches!(node.phase, Phase::Preparing { .. } | Phase::Proposing { .. }) {
                out.push(Action::Outsider(p));
            }
        }
        for msg in &s.net {
            match *msg {
                Msg::Prepare { from, .. } | Msg::Propose { from, .. } =>
                    out.extend((0..self.nodes()).filter(|n| *n != from).map(|n| Action::Deliver(*msg, n))),
                Msg::Ack { to, .. } | Msg::Refuse { to, .. } => out.push(Action::Deliver(*msg, to)),
                Msg::Vote { .. } => out.extend((0..self.proposers).map(|p| Action::Deliver(*msg, p))),
                Msg::Commit { .. } | Msg::Marker { .. } =>
                    out.extend((0..self.nodes()).map(|n| Action::Deliver(*msg, n))),
            }
        }
    }

    fn next_state(&self, last: &SlotState, action: Action) -> Option<SlotState> {
        let mut s = last.clone();
        let filter = self.filter();
        match action {
            Action::Tick(None) => {
                if s.nodes.iter().any(|n| n.clock >= self.horizon) { return None; }
                for n in &mut s.nodes { n.clock += 1; }
            }
            Action::Tick(Some(n)) => {
                let slowest = s.nodes.iter().map(|n| n.clock).min().unwrap_or(0);
                let node = &mut s.nodes[usize::from(n)];
                if node.clock >= self.horizon || node.clock + 1 - slowest > self.skew { return None; }
                node.clock += 1;
            }
            Action::Begin(p, ballot) => {
                let node = &mut s.nodes[usize::from(p)];
                if node.phase != Phase::Idle || node.calls >= self.calls { return None; }
                // A ballot is drawn above the floor.
                if ballot <= view(node).floor { return None; }
                let own = Val { proposer: p, call: node.calls, lineage: ballot, expires: Some(node.clock + self.lease) };
                node.calls += 1;
                self.start_ballot(&mut s, p, own, own, 0, ballot);
            }
            Action::Retry(p, ballot) => {
                let Phase::Between { own, current, adopted_from, last } = s.nodes[usize::from(p)].phase else { return None };
                if ballot <= last { return None; }
                self.start_ballot(&mut s, p, own, current, adopted_from, ballot);
            }
            Action::TimeOut(p) => {
                let node = &mut s.nodes[usize::from(p)];
                node.phase = match node.phase {
                    Phase::Preparing { own, current, adopted_from, ballot, .. } =>
                        self.failed(own, current, adopted_from, ballot),
                    Phase::Proposing { own, value, adopted_from, ballot, .. } =>
                        self.failed(own, value, adopted_from, ballot),
                    _ => return None,
                };
            }
            Action::Return(p) => {
                let node = &mut s.nodes[usize::from(p)];
                if !matches!(node.phase, Phase::Between { .. }) { return None; }
                node.phase = Phase::Idle;
            }
            Action::Release(p) => {
                let node = &mut s.nodes[usize::from(p)];
                let value = node.guard.take()?;
                if !node.released.contains(&value) {
                    node.released.push(value);
                    node.released.sort_unstable();
                }
                s.net.insert(Msg::Marker { value });
            }
            Action::Deliver(Msg::Prepare { ballot, from }, n) => {
                let node = &mut s.nodes[usize::from(n)];
                let v = view(node);
                let (acc, promised) = shared::prepare(&node.acc, ballot, from, v.floor);
                let accepted = promised?;
                node.acc = acc;
                s.net.insert(Msg::Ack { ballot, to: from, voter: n, accepted, live: v.live.map(|l| l.raw()) });
            }
            Action::Deliver(Msg::Ack { ballot, to, voter, accepted, live }, _) => {
                let node = &mut s.nodes[usize::from(to)];
                let Phase::Preparing { current, ballot: at, ref mut promisers, ref mut reports, .. } = node.phase else { return None };
                if at != ballot || promisers.has(voter) || !shared::counted(filter.as_deref(), voter) { return None; }
                // A promiser's live decision of another value ends the call (`Phase1::DecidedOtherwise`).
                if live.is_some_and(|l| l != current.raw()) {
                    node.phase = Phase::Idle;
                } else {
                    if let Some(report) = accepted && !reports.contains(&report) {
                        reports.push(report);
                        reports.sort_unstable();
                    }
                    *promisers = promisers.with(voter);
                    if promisers.len() >= self.quorum() { self.finish_phase1(&mut s, to); }
                }
            }
            Action::Deliver(Msg::Propose { ballot, from, value }, n) => {
                let node = &mut s.nodes[usize::from(n)];
                let v = view(node);
                // A live decision of another value refuses the proposal outright, whatever its ballot.
                let decided_otherwise = v.live.is_some_and(|l| l.raw() != value.raw());
                let claimed = if decided_otherwise { None } else { shared::accept(&node.acc, ballot, value, from, v.floor) };
                match claimed {
                    Some(acc) => {
                        node.acc = acc;
                        s.net.insert(Msg::Vote { ballot, voter: n, value });
                    }
                    None if !self.retries => return None,
                    None => {
                        let moves = node.acc.promised.max(v.floor) >= ballot;
                        s.net.insert(Msg::Refuse { ballot, to: from, from: n, accepted: node.acc.accepted, moves });
                    }
                }
            }
            Action::Deliver(Msg::Refuse { ballot, to, from, accepted, moves }, _) => {
                let node = &mut s.nodes[usize::from(to)];
                let Phase::Proposing { own, value, adopted_from, ballot: at, .. } = node.phase else { return None };
                // Only a member's refusal moves a proposal on an electorate group (`sent_by_member`).
                if at != ballot || !moves || !shared::counted(filter.as_deref(), from) { return None; }
                // Accepted-value preservation: the next ballot carries what the refuser holds.
                let (current, adopted_from) = match accepted {
                    Some((at, held)) if at >= adopted_from => (held, at),
                    _ => (value, adopted_from),
                };
                node.phase = Phase::Between { own, current, adopted_from, last: ballot };
            }
            Action::Deliver(Msg::Vote { ballot, voter, value }, p) => {
                let node = &mut s.nodes[usize::from(p)];
                let Phase::Proposing { value: mine, ballot: at, ref mut voters, .. } = node.phase else { return None };
                if at != ballot || mine != value || voters.has(voter) || !shared::counted(filter.as_deref(), voter) {
                    return None;
                }
                *voters = voters.with(voter);
                if voters.len() >= self.quorum() { self.commit(&mut s, p); }
            }
            Action::Ballot(p, ballot, others) => {
                if !self.whole_ballots { return None; }
                let step = |s: SlotState, a: Action| self.next_state(&s, a).unwrap_or(s);
                let members = || (0..self.members).filter(|n| others.has(*n));
                s = self.next_state(&s, Action::Begin(p, ballot))?;
                for n in members() { s = step(s, Action::Deliver(Msg::Prepare { ballot, from: p }, n)); }
                for m in s.net.clone() {
                    if matches!(m, Msg::Ack { ballot: b, to, .. } if b == ballot && to == p) { s = step(s, Action::Deliver(m, p)); }
                }
                for m in s.net.clone() {
                    if matches!(m, Msg::Propose { ballot: b, from, .. } if b == ballot && from == p) {
                        for n in members() { s = step(s, Action::Deliver(m, n)); }
                    }
                }
                for m in s.net.clone() {
                    if matches!(m, Msg::Vote { ballot: b, .. } if b == ballot) { s = step(s, Action::Deliver(m, p)); }
                }
                // The ballot is over, whatever it came to: nothing of it is left in flight but its decision.
                s.nodes[usize::from(p)].phase = Phase::Idle;
                s.net.retain(|m| matches!(m, Msg::Commit { .. } | Msg::Marker { .. }));
            }
            Action::Outsider(p) => {
                let out = self.members;
                if !self.outsider || !shared::counted(filter.as_deref(), out) { return None; }
                let node = &mut s.nodes[usize::from(p)];
                match node.phase {
                    Phase::Preparing { ref mut promisers, .. } => {
                        *promisers = promisers.with(out);
                        if promisers.len() >= self.quorum() { self.finish_phase1(&mut s, p); }
                    }
                    Phase::Proposing { ref mut voters, .. } => {
                        *voters = voters.with(out);
                        if voters.len() >= self.quorum() { self.commit(&mut s, p); }
                    }
                    _ => return None,
                }
            }
            Action::Deliver(Msg::Commit { ballot, value }, n) => {
                if !learn(&mut s.nodes[usize::from(n)], ballot, value) { return None; }
            }
            Action::Deliver(Msg::Marker { value }, n) => {
                let node = &mut s.nodes[usize::from(n)];
                if node.released.contains(&value) { return None; }
                node.released.push(value);
                node.released.sort_unstable();
            }
        }
        (s != *last).then_some(s)
    }

    fn properties(&self) -> Vec<Property<Self>> {
        let mut out = vec![
            Property::<Self>::always(ONE_VALUE_PER_BALLOT, |_, s| {
                let decided = decisions(s);
                decided.iter().all(|(b, v)| decided.iter().all(|(c, w)| b != c || v == w))
            }),
            Property::<Self>::always(ONE_VALUE_PER_LIFE, |_, s| {
                let decided = decisions(s);
                decided.iter().all(|(b, v)| decided.iter().all(|(c, w)| b >= c || v == w || over(s, v)))
            }),
            Property::<Self>::always(ONE_HOLDER, |m, s| {
                s.nodes.iter().filter(|n| holding(n, m.margin)).count() <= 1
            }),
            Property::<Self>::always(GRANT_OUTSIDE_SKEW, |m, s| {
                s.nodes.iter().filter(|n| holding(n, m.skew)).count() <= 1
            }),
            Property::<Self>::sometimes(DECIDED, |_, s| !decisions(s).is_empty()),
        ];
        // What a configuration can reach, it must reach: a bound that never decides twice checks no reuse. Which
        // of these a configuration carries follows from its fields; `tests/model.rs` names the list each run must
        // carry, so a configuration edited until one drops out fails there rather than passing with less.
        if self.retries {
            out.push(Property::<Self>::sometimes(REDECIDED, |_, s| {
                let decided = decisions(s);
                decided.iter().any(|(b, v)| decided.iter().any(|(c, w)| b != c && v == w))
            }));
        }
        if self.horizon > self.lease {
            out.push(Property::<Self>::sometimes(REUSED_AFTER_LAPSE, |_, s| {
                let decided = decisions(s);
                decided.iter().any(|(b, v)| decided.iter().any(|(c, w)| b < c && v.raw() != w.raw() && !released(s, v)))
            }));
        }
        if self.release {
            out.push(Property::<Self>::sometimes(REUSED_AFTER_RELEASE, |_, s| {
                let decided = decisions(s);
                let clock = s.nodes.iter().map(|n| n.clock).max().unwrap_or(0);
                decided.iter().any(|(b, v)| decided.iter().any(|(c, w)| {
                    b < c && v.raw() != w.raw() && released(s, v) && v.expires.is_none_or(|at| clock <= at)
                }))
            }));
        }
        if self.release || self.horizon > self.lease {
            out.push(Property::<Self>::sometimes(SECOND_HOLDER, |_, s| {
                s.granted.iter().map(|v| v.proposer).collect::<BTreeSet<_>>().len() >= 2
            }));
        }
        if self.proposers >= 3 && self.release && self.horizon > self.lease {
            out.push(Property::<Self>::sometimes(THIRD_LIFE, |_, s| {
                decisions(s).iter().map(|(_, v)| v.raw()).collect::<BTreeSet<_>>().len() >= 3
                    && s.granted.iter().map(|v| v.proposer).collect::<BTreeSet<_>>().len() >= 3
            }));
        }
        out
    }
}

/// Invariant: a ballot decides one value.
pub const ONE_VALUE_PER_BALLOT: &str = "a ballot decides one value";
/// Invariant: at most one value decided per slot and life.
pub const ONE_VALUE_PER_LIFE: &str = "at most one value is decided per slot and life";
/// Invariant: no node is granted while another still acts as the holder — which a holder does until its own
/// deadline less its margin. With the margin as designed (δ) this is the plan's sentence, "no holder granted while
/// another is inside its own deadline minus δ"; with no margin it is "one holder at a time", which skew breaks.
pub const ONE_HOLDER: &str = "no holder is granted while another is inside its own deadline less its margin";
/// Invariant: the plan's sentence taken literally, whatever the holder does — no node is granted while another
/// is inside its own deadline less δ. It is a fact about readers and the skew bound, so no margin can break it.
pub const GRANT_OUTSIDE_SKEW: &str = "no holder is granted while another is inside its own deadline less the skew bound";
/// Reachability: something is decided.
pub const DECIDED: &str = "a value is decided";
/// Reachability: a later ballot decides the value an earlier one decided (a proposer adopted it).
pub const REDECIDED: &str = "a later ballot decides the same value again";
/// Reachability: the slot is decided again after its lease lapsed.
pub const REUSED_AFTER_LAPSE: &str = "the slot is decided again after a lapsed lease";
/// Reachability: the slot is decided again after a release, inside the released lease.
pub const REUSED_AFTER_RELEASE: &str = "the slot is decided again after a release";
/// Reachability: three proposers' values are each decided, and each proposer is issued a guard.
pub const THIRD_LIFE: &str = "the slot is decided a third time, and a third node is granted the lock";
/// Reachability: two nodes are each issued a guard.
pub const SECOND_HOLDER: &str = "a second node is granted the lock";

/// Every decision: a `Commit` is sent exactly when a proposer's vote quorum formed.
fn decisions(s: &SlotState) -> Vec<(u64, Val)> {
    s.net.iter().filter_map(|m| match m { Msg::Commit { ballot, value } => Some((*ballot, *value)), _ => None }).collect()
}

fn released(s: &SlotState, v: &Val) -> bool {
    s.net.contains(&Msg::Marker { value: *v })
}

/// The ground truth of a life's end: its holder released it, or its lease is past on some node's clock.
fn over(s: &SlotState, v: &Val) -> bool {
    released(s, v) || v.expires.is_some_and(|at| s.nodes.iter().any(|n| n.clock > at))
}
