# Post-360 hardening — the next batch, and consensus as a protocol, not a service

**Status:** adopted 2026-10-10, rev 1.0. Nothing delivered yet. It follows the 2026-10-09 360 review and the eleven
PRs that addressed its must-do list (#582–#592, private #28); those are recorded in the CHANGELOG and are not
repeated here. Rows close on evidence (verification policy rule 2): a row is closed by quoting each promise next to
the code or test that delivers it, and a promise that is not delivered is written *not built*.

## 1 · Why one plan

After the 360 review's must-do list was fixed, three sources were left with open items: the review's own lower-tier
findings, the independent adversarial reviews of the fix PRs (each recorded a stated limit or a follow-up), and an
external review of 2026-10-10. This plan puts them in one order, and records one decision about consensus that
shapes several rows (§2).

Every row follows the standing rules: the regression test is written first and seen to fail on the unfixed code;
every entry point that reads or writes the changed thing is enumerated, with the grep; an independent adversarial
review runs before merge; a behaviour change carries a CHANGELOG entry and, where a client can notice, an upgrade
note.

**Out of scope, by decision:** the 100-node nightly scale runner (V1 in [`what-is-proven.md`](../operations/what-is-proven.md)).
It needs a machine rather than a commit, and is deferred, not dropped.

## 2 · Consensus is a protocol, never a service

The question (2026-10-10): is a full Paxos at Layer III counter to the philosophy? **No.** [`philosophy.md`](../philosophy.md)
§ *Anderson — More Is Different* already allows it: an emergent layer adds laws of its own level — "ballots,
quorums, roles, listeners, an explicit lifecycle — protocol machinery is what protocols are made of" — and may hold
transient broken-symmetry states. Coase's firm is the analogy: coordinators emerge where coordination pays and
dissolve when it stops paying. The 2.30.0 prepare phase and the durable acceptor record are inside those bounds.

The constraint is the corrected litmus's *may never* column, not how rich the protocol is. The line this plan draws
explicitly:

- **A consensus protocol is legitimate.** Whichever nodes participate in a group run it; roles form per ballot; the
  coalition dissolves when the decision completes; its state is ordinary keys and signals; Layers I and II know
  nothing of it.
- **A consensus service is the coordinator trap.** A fixed tier of nodes that everyone else must reach in order to
  agree — the etcd or ZooKeeper shape — is "symmetry breaking baked into the laws": permanent, privileged and
  load-bearing. No deployment shape, configuration default or documentation may make a named node set the place
  agreement happens.

Four places strain that line today, and the rows in §3 address each:

| Strain | Why it crosses or approaches the line | Row |
|---|---|---|
| `elect_leader` commits permanently, with no lease and no release path; a dead leader is reported for ever | a role that escapes evaporation | C1 |
| acceptor memory is never collected (and since #585 is durable) | an output that does not decay ("mandate TTL applies to decisions too") | C2 |
| item 2, a fixed electorate for safety-sensitive consensus, could harden into "these nodes are the consensus nodes" | a designed-in privileged node set | P2 |
| the forged-floor finding tempts a Layer I write guard on `consensus/` | the substrate learning a Layer III law | held: #591 added tripwires above, no guard below |

## 3 · The rows

### 3.1 · Before the batch

| Row | Scope | Promises | Depends on |
|---|---|---|---|
| **R0** | Release **2.32.0** once the merge train (#582–#592) is merged | the release runbook (`RELEASING.md`), step 2b on the branch released from; upgrade notes checked as one set (the deprecation windows from #585 and #587, `SystemStats`/`ConsensusResult` fields, `/api/tuple`, `rpc/respond`, `mycelium-reason` 0.8.0, the three SDK versions); the private companion re-pinned with `COMPATIBILITY.md` updated | the train |
| **Φ** | Philosophy: "protocol, not service" | a paragraph in the corrected-litmus section stating §2's line, and the deployment rule that no shape may make a named node set the place agreement happens; the wiki's runtime-invariants page cites it | — |

### 3.2 · The external review's items (2026-10-10)

| Row | Scope | Promises | Depends on |
|---|---|---|---|
| **P1** | Refuse an exposed, unauthenticated gateway | `start()` refuses by name when the gateway binds a non-loopback address with no credential model (no token, no token table, no `[oidc]`), unless an explicit insecure opt-in is set; the opt-in warns once at start; the guarantee report and the secure profile reflect it; loopback development is unchanged | R0 |
| **P2** | A consensus electorate is a governed group | safety-sensitive consensus (the threat model §7 supported profile) requires a governed group, whose membership moves only through a governance route; the secure profile checks it; the electorate is named as a group, never as node identities; versioned electorates with joint-consensus transitions are recorded as protocol work for a later plan, not built here. **Two tensions P2 must resolve** (found while recording the decision, `docs/design/consensus-electorate.md`): (a) *governed is not fixed* — the opt-in `MembershipGovernor` exists to resize a governed group, a membership intent lapses after 5 minutes without re-publication, and the electorate floor after 30 s, so P2 must define an electorate group that does not resize while a slot is open (or exclude the governor from electorate groups); (b) the current §7 profile names voters by node identity (`declare_trust(group, &[NodeId])`), which D1 rules out as the *name* of an electorate — decide whether trust slices stay underneath as the vote filter. The gateway-only governance boundary (an embedded `join_group` or a `grp/` write still moves a governed group) is part of P2's scope. | R0 |
| **P3** | `#![deny(unsafe_code)]` in `mycelium-core` | the one production site (`erasure.rs`'s volatile key wipe) replaced by `zeroize` (already in the tree) or carried as a single scoped `#[allow]` with its justification; test-only `set_var` sites scoped likewise | — |
| **P5** | Split `src/agent/http.rs` (8,394 lines) by route family | behaviour-preserving; one shared authorisation-and-evidence layer every family goes through; the cross-door tests (the C7 bypass matrix, the public-surface check, the scope table) unchanged and green; the lock-order table and `required_scope` still complete | R0; before row E |

### 3.3 · The batch

| Row | Scope | Promises | Depends on |
|---|---|---|---|
| **A** | Locks and leases (with C1, C2) | a released lock whose tombstone was garbage-collected is not re-committed to its old holder; a reopened lease does not adopt the expired holder's value before the decided floor arrives; a late COMMIT does not re-stamp a released lock fleet-wide; **C1** `elect_leader` is lease-based by default with a release path (permanence stays available, opt-in); **C2** acceptor state is collected once its decision or lease is over, without weakening the promise a live slot depends on | #585, #591 merged |
| **B** | Core resource bounds | a handshake and idle timeout on the gossip accept path; connect, write and flush timeouts on the outbound writer, and no unbounded pile of anti-entropy replies behind a stalled peer; one stalled SSE or serve subscriber cannot veto admission of its kind node-wide; the signal log is bounded per sender-chosen kind; a stopped agent leaks no task context (`RpcRequestRx` ends at shutdown, as its doc says) | R0 |
| **C** | Companion WALs (tuple space, blackboard) | compaction is crash-durable (temp file and directory fsynced); a torn or undecodable record poisons the writer and is refused at open rather than silently dropping every later record; one owner per file | R0 |
| **D** | WASM execution limits | epoch interruption bounds a guest call; guest code runs off the runtime's worker threads; a trap does not recompile the component on every call | R0 |
| **E** | Gateway remainder | client-supplied timer periods (`interval_secs`) bounded; the log-group subscribe keeps the core's `(hlc, key)` tiebreak the mailbox deletes an entry only after the client has it (today it tombstones on entering the 256-slot channel, so a dropped stream loses queued events — #595's review); prompt keys cannot alias across `ns`/`name` (`prompts/{ns}/{name}` with a `/` in either); and saves the group offset only after the entry is sent (today it saves first, so an entry in flight when a stream drops is never re-sent — found by row G); the caller envelope binds freshness | P5 |
| **F** | Wire and crypto, one MINOR with a window | a TLS node refuses unsigned `Data` frames; the removal check reads a peer's key structurally, not by byte-scanning the certificate; KV `SignedData` is domain-tagged; each change carries a mixed-fleet window and an upgrade note in the 2.30.0 shape; the #585 and #587 windows close in the same MINOR | R0, ideally its own release |
| **G** | SDKs | Python whole-second timeouts and a non-truncating tuple `take`; path segments escaped; TypeScript `SupersededError`; one `scatterGather` default, documented; SSE reconnect is designed with a gateway resume point or recorded as not built | R0 |
| **H** | Companions | the commitment deadline judged on the log's clock, not the offerer's; `award_of` verifies before it reports; the reason blob store bounded; caller-chosen `run_id` validated | R0 |
| **I** | Private repo P3s | RA5 correction chains and competing corrections; attempt keys carry the operation id; a lock-order table; the outbox directory fsynced at create; Cedar policies schema-validated at load; the CI pin check refuses `[patch]`, `rev =` and multi-line tables | R0's re-pin |
| **J** | Hygiene | the sim-seam gate's test-skip bug (the one the KV gate fixed in #591); every feature and bench linted or built by some CI step; the two CI jobs on `stable` pinned; unreferenced public items deprecated, not removed (nothing is removed on the 2.x line); only the duplicates that disagree are unified (`hex_line`'s whitespace, `now_ms` copies outside the seam) | — |

### 3.4 · Documentation follow-through

A row is not closed until the documents that describe its subject say what the code now does. Several pages record a
position as *planned* or *not built* (the consensus electorate decision record `docs/design/consensus-electorate.md`,
the proof ledger, the threat model, the guide, the FAQ, the wiki); each must be flipped in the same PR that delivers
the change, or the row stays open. In particular:

| When this lands | Update |
|---|---|
| **P2** (governed electorate required) | the decision record's *enforced today* section; `what-is-proven.md`'s "Agreement across an electorate change" row (move what is now gated); `threat-model.md` §7; `guide/04-consensus.md` § *Discovery is not an electorate*; the FAQ answer; the wiki runtime-invariants page |
| **C1** (leased leadership) and **C2** (acceptor state decays) | the decision record and philosophy's "protocol, not service" paragraph (strike *not built*); `guide/04-consensus.md` on `elect_leader`; `runtime-invariants.md` § *Acceptor memory* |
| **P1** (exposed-gateway refusal) | README's and `positioning.md`'s default-posture sentence; `production-readiness.md`; `configuration.md`; the guarantee catalogue golden |
| **F** (wire and crypto MINOR) | `deprecations.md` (close the #585/#587 windows); the threat model's Boundary A; `what-is-proven.md`'s KV `SignedData` residual |
| **every row** | the CHANGELOG entry, a dated wiki `.log/` entry, and any `what-is-proven.md` row the change moves between tables |

The next `doc-coverage` run after each MINOR checks this table: a page still saying *not built* for a delivered row is a
calibration entry.

## 4 · Decisions taken

| # | Decision | Recorded |
|---|---|---|
| D1 | Consensus is a Layer III protocol and never a service (§2) | 2026-10-10 |
| D2 | Unreferenced public items are deprecated, not removed | 2026-10-10 |
| D3 | No blanket dedupe; only disagreeing duplicates are unified | 2026-10-10 |
| D4 | SSE reconnect and the wire/crypto changes are features with windows, not patches | 2026-10-10 |
| D5 | The scale runner is deferred | 2026-10-10 |
| D6 | P5 lands before row E, so gateway fixes are written once | 2026-10-10 |

## 5 · Open questions

- Should a consensus `Timeout` result carry its cause (today only a metric label)? Doc-coverage run 23, code gap 1.
- Should the MCP bridge and LLM backend clients get timeouts? PR #590's review.
- Versioned electorates with joint-consensus transitions: a later plan, under D1.

## 6 · Recorded limits that stay limits

Single-decree safety rather than a proof of the whole protocol; DNS rebinding outside the egress claim; a member that
*signs* consensus messages at the ballot ceiling; a forgery that reaches a node only through anti-entropy before it
holds its own record.
