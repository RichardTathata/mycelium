//! **The electorate migration**: declare the successor, retire the predecessor, carry, complete.
//!
//! Two electorate groups decide one slot: the predecessor, and a successor that names it. An acceptor's memory
//! of the slot is one record, whichever group asks (as `TaskCtx::consensus_accepted` is), so a node in both
//! groups answers both from it.
//!
//! - **The predecessor's ballots** run message by message through a network that may delay, reorder, duplicate
//!   or lose any message — the race that matters is a ballot in flight against the group closing under it.
//! - **The successor's ballots** — the carry, then an ordinary one of its own — are each one step: the proposer
//!   and the rest of a quorum run prepare, promise, proposal, vote and commit with nothing in between. A carry
//!   ballot that does not decide is run again.
//! - **The three decisions of the migration itself** — the successor's genesis, the retirement, the completion
//!   — are each one certificate: the votes of every member (a genesis) or of a strict majority, cast one member
//!   at a time, and visible at a node once it learns them. Their own ballots are the acceptor's, which
//!   [`crate::slot`] checks.
//!
//! No decision record is modelled here: every decision is permanent, and a live record never ends a call.

use std::collections::BTreeSet;

use stateright::{Model, Property};

use crate::shared::{self, Acc, Choice, Node, Set, Val};

/// The configuration of a run: the bound, and the planted fault.
#[derive(Clone, Debug)]
pub struct MigrationModel {
    /// The predecessor's members.
    pub predecessor: Vec<Node>,
    /// The successor's members.
    pub successor:   Vec<Node>,
    /// The predecessor's proposers, each with one call of one ballot and a value of its own.
    pub proposers:   Vec<Node>,
    /// The successor's member that carries, completes, and then proposes a value of its own.
    pub carrier:     Node,
    /// The highest ballot drawn.
    pub max_ballot:  u64,
    /// **Planted fault** — the successor decides early: its ordinary proposals are admitted, at the proposer's
    /// door and at every acceptor, without the predecessor's retirement or the carry's completion.
    pub successor_decides_early: bool,
}

impl MigrationModel {
    /// A rolling change of who decides: the predecessor `{0, 1, 2}`, the successor `{1, 2, 3}`; node 0 proposes
    /// in the predecessor, node 3 carries and then proposes a value of its own; ballots to 3.
    pub fn rolling() -> Self {
        Self {
            predecessor: vec![0, 1, 2], successor: vec![1, 2, 3], proposers: vec![0], carrier: 3, max_ballot: 3,
            successor_decides_early: false,
        }
    }

    /// [`rolling`](Self::rolling) with both predecessor proposers contending — nodes 0 and 1 — and ballots to 2.
    pub fn contended() -> Self {
        Self { proposers: vec![0, 1], max_ballot: 2, ..Self::rolling() }
    }

    /// The planted fault of the migration area: a successor deciding early.
    pub fn with_successor_deciding_early(mut self) -> Self {
        self.successor_decides_early = true;
        self
    }

    fn nodes(&self) -> usize {
        self.predecessor.iter().chain(&self.successor).copied().max().map_or(0, |n| usize::from(n) + 1)
    }

    fn members(&self, role: Role) -> &[Node] {
        match role { Role::Predecessor(_) => &self.predecessor, Role::Carry | Role::Successor => &self.successor }
    }

    fn quorum(&self, role: Role) -> usize { shared::quorum(self.members(role)) }

    fn proposer(&self, role: Role) -> Node {
        match role { Role::Predecessor(p) => p, Role::Carry | Role::Successor => self.carrier }
    }

    /// Whether the predecessor's retirement is decided: a strict majority of it promised (and closed).
    pub fn retired(&self, s: &MigrationState) -> bool {
        self.predecessor.iter().filter(|n| s.nodes[usize::from(**n)].retire_vote).count() >= shared::quorum(&self.predecessor)
    }

    /// Whether the carry's completion is decided: a strict majority of the successor voted for it.
    pub fn completed(&self, s: &MigrationState) -> bool {
        self.successor.iter().filter(|n| s.nodes[usize::from(**n)].complete_vote).count() >= shared::quorum(&self.successor)
    }

    /// **Whether `n` answers a claim of `role`** — the model's `electorate_admits` and gate (`claim_gated`): a
    /// member of the group; for the predecessor, not closed here; for a carry ballot, the retirement visible
    /// here (`carry_admissible`); for anything else of the successor, the carry's completion visible here
    /// (`migration_complete`: `successor_ready` and no `incomplete_carry`).
    fn admits(&self, s: &MigrationState, role: Role, n: Node) -> bool {
        let node = &s.nodes[usize::from(n)];
        if !shared::is_member(self.members(role), n) { return false; }
        match role {
            Role::Predecessor(_) => !node.closed,
            Role::Carry => node.sees_successor && node.sees_retirement,
            Role::Successor => node.sees_successor && (self.successor_decides_early || node.sees_completion),
        }
    }
}

/// Whose ballot a message belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Role {
    /// An ordinary proposal in the predecessor, by this member.
    Predecessor(Node),
    /// The successor's carry ballot (`CarryPrepare` / `CarryPropose`).
    Carry,
    /// An ordinary proposal in the successor.
    Successor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Msg {
    Prepare { role: Role, ballot: u64 },
    Ack { role: Role, ballot: u64, voter: Node, accepted: Option<(u64, Val)> },
    Propose { role: Role, ballot: u64, value: Val },
    Vote { role: Role, ballot: u64, voter: Node, value: Val },
    /// A decision: `successor` says in which group.
    Commit { successor: bool, ballot: u64, value: Val },
    /// A retired member's manifest for the slot: what it promised and accepted, read after it closed.
    Manifest { from: Node, promised: u64, accepted: Option<(u64, Val)> },
}

/// One ballot of one proposer.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    /// Not begun.
    Idle,
    Preparing { own: Val, ballot: u64, promisers: Set, reports: Vec<(u64, Val)> },
    Proposing { value: Val, ballot: u64, voters: Set },
    /// The call is over.
    Done,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct NodeState {
    pub acc:             Acc,
    /// The predecessor is closed here, for good.
    pub closed:          bool,
    /// Promised the predecessor's retirement (which closes it here).
    pub retire_vote:     bool,
    /// Voted for the completion of the successor's carry.
    pub complete_vote:   bool,
    /// The successor's verified genesis is held here.
    pub sees_successor:  bool,
    /// The predecessor's verified retirement is held here.
    pub sees_retirement: bool,
    /// The verified completion of the carry is held here.
    pub sees_completion: bool,
    /// Accepted a carry ballot of the successor.
    pub accepted_carry:  bool,
    /// Accepted an ordinary ballot of the successor.
    pub accepted_own:    bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MigrationState {
    pub nodes:     Vec<NodeState>,
    pub net:       BTreeSet<Msg>,
    /// The successor's genesis is decided.
    pub declared:  bool,
    /// The predecessor's proposers, in the order of `MigrationModel::proposers`.
    pub proposing: Vec<Phase>,
    /// The manifests the carrier holds: who reported, the highest acceptance, the highest ballot named.
    pub manifests: (Set, Option<(u64, Val)>, u64),
    pub carry:     Phase,
    /// The carry is done: the carry ballot committed, or the manifests held nothing to carry.
    pub carried:   bool,
    pub ordinary:  Phase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    /// The successor's genesis is decided — by every member it names, which hold it from then on.
    Declare,
    /// A node learns the successor's genesis, the retirement, the completion.
    LearnSuccessor(Node),
    LearnRetirement(Node),
    LearnCompletion(Node),
    /// A predecessor member promises the retirement, and closes.
    RetireVote(Node),
    /// A retired member closes and reports its manifest.
    Manifest(Node),
    /// A successor member votes for the completion.
    CompleteVote(Node),
    /// A predecessor's proposer begins its ballot.
    Begin(Role, u64),
    Deliver(Msg, Node),
    /// A whole ballot of the successor — the carry, or an ordinary one — with these members as the rest of its
    /// quorum: prepare, promise, proposal, vote and commit in one step.
    Ballot(Role, u64, Set),
}

impl MigrationState {
    fn phase(&mut self, m: &MigrationModel, role: Role) -> &mut Phase {
        match role {
            Role::Predecessor(p) => {
                let i = m.proposers.iter().position(|q| *q == p).expect("a predecessor role names a proposer");
                &mut self.proposing[i]
            }
            Role::Carry => &mut self.carry,
            Role::Successor => &mut self.ordinary,
        }
    }
}

impl MigrationModel {
    fn filter(&self, role: Role) -> shared::Filter { shared::vote_filter(Some(self.members(role))) }

    fn note_accepted(node: &mut NodeState, role: Role) {
        match role {
            Role::Predecessor(_) => {}
            Role::Carry => node.accepted_carry = true,
            Role::Successor => node.accepted_own = true,
        }
    }

    fn finish_phase1(&self, s: &mut MigrationState, role: Role) {
        let p = self.proposer(role);
        let Phase::Preparing { own, ballot, ref reports, .. } = *s.phase(self, role) else { return };
        let value = match shared::phase1(own, own, reports, 0) {
            Choice::Keep => own,
            Choice::Adopt(_, adopted) => adopted,
            Choice::Blocked => { *s.phase(self, role) = Phase::Done; return; }
        };
        // The proposer's own vote passes the same gate and the same claim as any acceptor's.
        let claimed = if self.admits(s, role, p) { shared::accept(&s.nodes[usize::from(p)].acc, ballot, value, p, 0) } else { None };
        let Some(acc) = claimed else { *s.phase(self, role) = Phase::Done; return; };
        s.nodes[usize::from(p)].acc = acc;
        Self::note_accepted(&mut s.nodes[usize::from(p)], role);
        *s.phase(self, role) = Phase::Proposing { value, ballot, voters: Set::default().with(p) };
        s.net.insert(Msg::Propose { role, ballot, value });
        if self.quorum(role) <= 1 { self.commit(s, role); }
    }

    fn commit(&self, s: &mut MigrationState, role: Role) {
        let Phase::Proposing { value, ballot, .. } = *s.phase(self, role) else { return };
        *s.phase(self, role) = Phase::Done;
        if role == Role::Carry { s.carried = true; }
        s.net.insert(Msg::Commit { successor: !matches!(role, Role::Predecessor(_)), ballot, value });
    }
}

impl Model for MigrationModel {
    type State = MigrationState;
    type Action = Action;

    fn init_states(&self) -> Vec<MigrationState> {
        vec![MigrationState {
            nodes: vec![NodeState::default(); self.nodes()],
            net: BTreeSet::new(),
            declared: false,
            proposing: vec![Phase::Idle; self.proposers.len()],
            manifests: (Set::default(), None, 0),
            carry: Phase::Idle,
            carried: false,
            ordinary: Phase::Idle,
        }]
    }

    fn actions(&self, s: &MigrationState, out: &mut Vec<Action>) {
        if !s.declared { out.push(Action::Declare); }
        let all = 0..self.nodes() as Node;
        // Each record is learned where something reads it: the genesis by a predecessor member (to retire), the
        // retirement by everyone, the completion by a successor member.
        out.extend(self.predecessor.iter().filter(|n| s.declared && !s.nodes[usize::from(**n)].sees_successor).map(|n| Action::LearnSuccessor(*n)));
        out.extend(all.filter(|n| !s.nodes[usize::from(*n)].sees_retirement).map(Action::LearnRetirement));
        out.extend(self.successor.iter().filter(|n| !s.nodes[usize::from(**n)].sees_completion).map(|n| Action::LearnCompletion(*n)));
        out.extend(self.predecessor.iter().map(|n| Action::RetireVote(*n)));
        out.extend(self.predecessor.iter().map(|n| Action::Manifest(*n)));
        out.extend(self.successor.iter().map(|n| Action::CompleteVote(*n)));
        // A ballot is drawn one above the shared ballot key, which is gossip (see `slot`).
        let top = s.nodes.iter().map(|n| n.acc.promised).max().unwrap_or(0).saturating_add(1).min(self.max_ballot);
        out.extend(self.proposers.iter().flat_map(|p| (1..=top).map(move |b| Action::Begin(Role::Predecessor(*p), b))));
        for role in [Role::Carry, Role::Successor] {
            let others = (0u16..1 << self.nodes()).map(Set).filter(move |q| {
                !q.has(self.carrier) && q.len() + 1 == self.quorum(role)
                    && (0..self.nodes() as Node).all(|n| !q.has(n) || self.successor.contains(&n))
            });
            out.extend(others.flat_map(|q| (1..=top).map(move |b| Action::Ballot(role, b, q))));
        }
        for msg in &s.net {
            match *msg {
                Msg::Prepare { role, .. } | Msg::Propose { role, .. } => {
                    let from = self.proposer(role);
                    out.extend(self.members(role).iter().filter(|n| **n != from).map(|n| Action::Deliver(*msg, *n)));
                }
                Msg::Ack { role, .. } | Msg::Vote { role, .. } => out.push(Action::Deliver(*msg, self.proposer(role))),
                Msg::Manifest { .. } => out.push(Action::Deliver(*msg, self.carrier)),
                Msg::Commit { .. } => {}
            }
        }
    }

    fn next_state(&self, last: &MigrationState, action: Action) -> Option<MigrationState> {
        let mut s = last.clone();
        match action {
            Action::Declare => {
                s.declared = true;
                for n in &self.successor { s.nodes[usize::from(*n)].sees_successor = true; }
            }
            Action::LearnSuccessor(n) => {
                if !s.declared { return None; }
                s.nodes[usize::from(n)].sees_successor = true;
            }
            Action::LearnRetirement(n) => {
                if !self.retired(&s) { return None; }
                s.nodes[usize::from(n)].sees_retirement = true;
            }
            Action::LearnCompletion(n) => {
                if !self.completed(&s) { return None; }
                s.nodes[usize::from(n)].sees_completion = true;
            }
            Action::RetireVote(n) => {
                // Successor first: a member promises a retirement only to a successor it holds, naming this
                // group (`has_live_successor_naming`). The promise closes the group here before anything else.
                let node = &mut s.nodes[usize::from(n)];
                if !node.sees_successor { return None; }
                node.closed = true;
                node.retire_vote = true;
            }
            Action::Manifest(n) => {
                // `close_for_carry`: a member reports nothing until it holds the retirement; then it closes, and
                // reads its memory after.
                let node = &mut s.nodes[usize::from(n)];
                if !node.sees_retirement { return None; }
                node.closed = true;
                s.net.insert(Msg::Manifest { from: n, promised: node.acc.promised, accepted: node.acc.accepted });
            }
            Action::CompleteVote(n) => {
                // The completion is proposed once the carry is done, and accepted where the retirement is visible
                // (`completion_acceptable`).
                if !s.carried { return None; }
                let node = &mut s.nodes[usize::from(n)];
                if !node.sees_successor || !node.sees_retirement { return None; }
                node.complete_vote = true;
            }
            Action::Begin(role, ballot) => {
                if *s.phase(self, role) != Phase::Idle { return None; }
                let p = self.proposer(role);
                let own = match role {
                    Role::Predecessor(_) | Role::Successor =>
                        Val { proposer: p, call: u8::from(role == Role::Successor), lineage: ballot, expires: None },
                    Role::Carry => {
                        // Enumerate, then carry: manifests from a strict majority of the predecessor; the value is
                        // the highest acceptance they report, at a ballot above every ballot they name.
                        let (reported, best, above) = s.manifests;
                        if reported.len() < shared::quorum(&self.predecessor) { return None; }
                        let Some((_, value)) = best else {
                            if s.carried { return None; }
                            s.carried = true; // nothing to carry
                            s.carry = Phase::Done;
                            return Some(s);
                        };
                        if ballot <= above { return None; }
                        value
                    }
                };
                // The proposer's door, then its own promise through the gate.
                if !self.admits(&s, role, p) { return None; }
                let (acc, promised) = shared::prepare(&s.nodes[usize::from(p)].acc, ballot, p, 0);
                let Some(accepted) = promised else {
                    *s.phase(self, role) = Phase::Done;
                    return Some(s);
                };
                s.nodes[usize::from(p)].acc = acc;
                *s.phase(self, role) = Phase::Preparing {
                    own, ballot, promisers: Set::default().with(p), reports: accepted.into_iter().collect(),
                };
                if self.quorum(role) <= 1 { self.finish_phase1(&mut s, role); } else { s.net.insert(Msg::Prepare { role, ballot }); }
            }
            Action::Deliver(Msg::Prepare { role, ballot }, n) => {
                if !self.admits(&s, role, n) { return None; }
                let node = &mut s.nodes[usize::from(n)];
                let (acc, promised) = shared::prepare(&node.acc, ballot, self.proposer(role), 0);
                let accepted = promised?;
                node.acc = acc;
                s.net.insert(Msg::Ack { role, ballot, voter: n, accepted });
            }
            Action::Deliver(Msg::Ack { role, ballot, voter, accepted }, _) => {
                let quorum = self.quorum(role);
                let filter = self.filter(role);
                let Phase::Preparing { ballot: at, ref mut promisers, ref mut reports, .. } = *s.phase(self, role) else { return None };
                if at != ballot || promisers.has(voter) || !shared::counted(filter.as_deref(), voter) { return None; }
                if let Some(report) = accepted && !reports.contains(&report) {
                    reports.push(report);
                    reports.sort_unstable();
                }
                *promisers = promisers.with(voter);
                if promisers.len() >= quorum { self.finish_phase1(&mut s, role); }
            }
            Action::Deliver(Msg::Propose { role, ballot, value }, n) => {
                if !self.admits(&s, role, n) { return None; }
                let node = &mut s.nodes[usize::from(n)];
                node.acc = shared::accept(&node.acc, ballot, value, self.proposer(role), 0)?;
                Self::note_accepted(node, role);
                s.net.insert(Msg::Vote { role, ballot, voter: n, value });
            }
            Action::Deliver(Msg::Vote { role, ballot, voter, value }, _) => {
                let quorum = self.quorum(role);
                let filter = self.filter(role);
                let Phase::Proposing { value: mine, ballot: at, ref mut voters } = *s.phase(self, role) else { return None };
                if at != ballot || mine != value || voters.has(voter) || !shared::counted(filter.as_deref(), voter) { return None; }
                *voters = voters.with(voter);
                if voters.len() >= quorum { self.commit(&mut s, role); }
            }
            Action::Deliver(Msg::Manifest { from, promised, accepted }, _) => {
                // The carrier enumerates once it holds the retirement, until the carry is done.
                if !s.nodes[usize::from(self.carrier)].sees_retirement || s.carried { return None; }
                let (reported, best, above) = &mut s.manifests;
                if reported.has(from) { return None; }
                *reported = reported.with(from);
                *above = (*above).max(promised).max(accepted.map_or(0, |(b, _)| b));
                if accepted.is_some_and(|(b, _)| best.is_none_or(|(top, _)| b > top)) { *best = accepted; }
            }
            Action::Deliver(Msg::Commit { .. }, _) => return None,
            Action::Ballot(role, ballot, others) => {
                if matches!(role, Role::Predecessor(_)) { return None; }
                let step = |s: MigrationState, a: Action| self.next_state(&s, a).unwrap_or(s);
                let members = || self.successor.iter().copied().filter(|n| others.has(*n));
                s = self.next_state(&s, Action::Begin(role, ballot))?;
                for n in members() { s = step(s, Action::Deliver(Msg::Prepare { role, ballot }, n)); }
                for m in s.net.clone() {
                    if matches!(m, Msg::Ack { role: r, ballot: b, .. } if r == role && b == ballot) { s = step(s, Action::Deliver(m, self.carrier)); }
                }
                for m in s.net.clone() {
                    if matches!(m, Msg::Propose { role: r, ballot: b, .. } if r == role && b == ballot) {
                        for n in members() { s = step(s, Action::Deliver(m, n)); }
                    }
                }
                for m in s.net.clone() {
                    if matches!(m, Msg::Vote { role: r, ballot: b, .. } if r == role && b == ballot) { s = step(s, Action::Deliver(m, self.carrier)); }
                }
                // The ballot is over, whatever it came to: nothing of it is left in flight but its decision. A carry
                // ballot that did not decide is run again (`complete_migration` is retried); an ordinary call is one ballot.
                *s.phase(self, role) = if role == Role::Carry && !s.carried { Phase::Idle } else { Phase::Done };
                s.net.retain(|m| !matches!(m, Msg::Prepare { role: r, .. } | Msg::Ack { role: r, .. }
                    | Msg::Propose { role: r, .. } | Msg::Vote { role: r, .. } if *r == role));
            }
        }
        (s != *last).then_some(s)
    }

    fn properties(&self) -> Vec<Property<Self>> {
        vec![
            Property::<Self>::always(RETIREMENT_FIRST, |_, s| {
                s.nodes.iter().all(|n| !(n.accepted_carry || n.accepted_own) || n.sees_retirement || n.sees_completion)
            }),
            Property::<Self>::always(COMPLETION_FIRST, |_, s| {
                s.nodes.iter().all(|n| !n.accepted_own || n.sees_completion)
            }),
            Property::<Self>::always(ONE_VALUE_ACROSS, |_, s| {
                let decided = decisions(s);
                decided.iter().all(|(_, v)| decided.iter().all(|(_, w)| v == w))
            }),
            Property::<Self>::sometimes(CARRIED_OVER, |_, s| {
                let decided = decisions(s);
                decided.iter().any(|(succ, v)| !succ && decided.contains(&(true, *v)))
            }),
            Property::<Self>::sometimes(SUCCESSOR_DECIDES, |m, s| {
                decisions(s).iter().any(|(succ, v)| *succ && v.proposer == m.carrier)
            }),
        ]
    }
}

/// Invariant: nothing decided by a successor before the predecessor's retirement is visible — the verified
/// retirement, or its proof, the verified completion, as `ConsensusEngine::predecessor_state` reads it.
pub const RETIREMENT_FIRST: &str = "nothing is decided by the successor before the predecessor's retirement is visible";
/// Invariant: the successor decides outside the carry only once the carry is complete.
pub const COMPLETION_FIRST: &str = "the successor decides nothing of its own before the carry is complete";
/// Invariant: the slot has one value, whichever group decided it.
pub const ONE_VALUE_ACROSS: &str = "the predecessor and the successor decide one value";
/// Reachability: a value decided in the predecessor is decided again by the carry.
pub const CARRIED_OVER: &str = "a value the predecessor decided is carried";
/// Reachability: the successor decides a value of its own.
pub const SUCCESSOR_DECIDES: &str = "the successor decides a value of its own";

/// Every decision: `(in the successor, value)`.
fn decisions(s: &MigrationState) -> Vec<(bool, Val)> {
    s.net.iter().filter_map(|m| match m { Msg::Commit { successor, value, .. } => Some((*successor, *value)), _ => None }).collect()
}
