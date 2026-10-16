# Post-360 hardening — the next batch, and consensus as a protocol, not a service

**Status:** adopted 2026-10-10, rev 1.10 (2026-10-16: the v2.34.0 cut — F2, S and T closed on evidence in §3.5 by an independent reviewer, T's block written then, its `dev` log line *not built*; rev 1.9, 2026-10-13: the v2.33.0 cut — every row since 2.32.0 closed on evidence in §3.5, and the table's state markers moved; rev 1.8, 2026-10-12: Φ closed; P5, J, I, H, E and F delivered; rows K, L, M added from the Strands Box comparison; rows N, O, P added — Strands agents on Mycelium, after an `a2a-sdk` probe; Q delivered; rows S and T added). **R0 was released as v2.32.0 (2026-10-11)**, delivering rows **G, P3, C, D,
P1, A (with C1, C2), B (as narrowed 2026-10-11) and P2 (without migration, as decided 2026-10-11)**; R0's private re-pin landed as private #29. **v2.33.0 (cut 2026-10-13) delivers rows Φ (#607), P2's migration (#606, with #604 and #605), J (#608), P5 (#610), H (#609), E (#613), F (#611, with its window — the flip is F2), Q (#616) and, in the private repository on that pin, I (private #30)**; its private re-pin landed as private #31 — each closed on evidence in §3.5. **v2.34.0 (cut 2026-10-16) delivers rows F2 (#624), S (#623) and T (#622)**, with three §3.6 follow-ups (#627–#629) and
the wasmtime/rustls advisories (#625). Open: K, L, M, N, O, P. It follows the 2026-10-09 360 review and the eleven
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
| **R0** — *released: v2.32.0 (2026-10-11; private re-pin #29)* | Release **2.32.0** once the merge train (#582–#592) is merged | the release runbook (`RELEASING.md`), step 2b on the branch released from; upgrade notes checked as one set (the deprecation windows from #585 and #587, `SystemStats`/`ConsensusResult` fields, `/api/tuple`, `rpc/respond`, `mycelium-reason` 0.8.0, the three SDK versions); the private companion re-pinned with `COMPATIBILITY.md` updated | the train |
| **Φ** — *delivered 2.33.0 (#607), §3.5* | Philosophy: "protocol, not service" | a paragraph in the corrected-litmus section stating §2's line, and the deployment rule that no shape may make a named node set the place agreement happens; the wiki's runtime-invariants page cites it | — |

### 3.2 · The external review's items (2026-10-10)

| Row | Scope | Promises | Depends on |
|---|---|---|---|
| **P1** — *delivered 2.32.0 (#598), §3.5* | Refuse an exposed, unauthenticated gateway | `start()` refuses by name when the gateway binds a non-loopback address with no credential model (no token, no token table, no `[oidc]`), unless an explicit insecure opt-in is set; the opt-in warns once at start; the guarantee report and the secure profile reflect it; loopback development is unchanged | R0 |
| **P2** — *delivered 2.32.0 without migration (#601); the migration 2.33.0 (#606), §3.5* | A consensus electorate is a governed group | safety-sensitive consensus (the threat model §7 supported profile) requires a governed group, whose membership moves only through a governance route; the secure profile checks it; the electorate is named as a group, never as node identities; versioned electorates with joint-consensus transitions are recorded as protocol work for a later plan, not built here. **Two tensions P2 must resolve** (found while recording the decision, `docs/design/consensus-electorate.md`): (a) *governed is not fixed* — the opt-in `MembershipGovernor` exists to resize a governed group, a membership intent lapses after 5 minutes without re-publication, and the electorate floor after 30 s, so P2 must define an electorate group that does not resize while a slot is open (or exclude the governor from electorate groups); (b) the current §7 profile names voters by node identity (`declare_trust(group, &[NodeId])`), which D1 rules out as the *name* of an electorate — decide whether trust slices stay underneath as the vote filter. The gateway-only governance boundary (an embedded `join_group` or a `grp/` write still moves a governed group) is part of P2's scope. **The maintainer's decisions (2026-10-11), recorded on #601:** after its round-4 review, **freeze membership** — an electorate group's members never change once its genesis is decided (the live one-member steps with a drain before each, rounds 2–3, were removed); and after its round-5 review, **P2 ships in 2.32.0 without migration** — changing an electorate's members (retire into a successor that carries its decisions, built in rounds 4–5) is not built in that release; **delivered after it** as a migration that terminates — close, enumerate, carry, complete — with the default moving by retirement (`docs/design/consensus-electorate.md` §8.3, §4.2). | R0 |
| **P3** — *delivered 2.32.0 (#596), §3.5* | `#![deny(unsafe_code)]` in `mycelium-core` | the one production site (`erasure.rs`'s volatile key wipe) replaced by `zeroize` (already in the tree) or carried as a single scoped `#[allow]` with its justification; test-only `set_var` sites scoped likewise | — |
| **P5** — *delivered 2.33.0 (#610), §3.5* | Split `src/agent/http.rs` (8,394 lines at the review; 9,889 at the split) by route family | behaviour-preserving; one shared authorisation-and-evidence layer every family goes through; the cross-door tests (the C7 bypass matrix, the public-surface check, the scope table) unchanged and green; the lock-order table and `required_scope` still complete | R0; before row E |

### 3.3 · The batch

| Row | Scope | Promises | Depends on |
|---|---|---|---|
| **A** — *delivered 2.32.0 (#600), §3.5* | Locks and leases (with C1, C2) | a released lock whose tombstone was garbage-collected is not re-committed to its old holder; a reopened lease does not adopt the expired holder's value before the decided floor arrives; a late COMMIT does not re-stamp a released lock fleet-wide; **C1** `elect_leader` is lease-based by default with a release path (permanence stays available, opt-in); **C2** acceptor state is collected once its decision or lease is over, without weakening the promise a live slot depends on | #585, #591 merged |
| **B** — *delivered 2.32.0 as narrowed (#602), §3.5* | Core resource bounds | a handshake and idle timeout on the gossip accept path, and read-progress bounds with a 1 KiB/s floor — **the accept path is bounded against silent and slow connections; an attacker that keeps talking can hold the connection slots it obtained from free permits**, mitigated by mTLS (a non-member fails the handshake within `handshake_timeout_ms`) and an opt-in per-source cap (narrowed by the maintainer's decision, 2026-10-11, after review rounds 3–5 of #602 found connection preemption could displace honest peers); connect, write and flush timeouts on the outbound writer, and no unbounded pile of anti-entropy replies behind a stalled peer; one stalled SSE or serve subscriber cannot veto admission of its kind node-wide; the signal log is bounded per sender-chosen kind; a stopped agent leaks no task context (`RpcRequestRx` ends at shutdown, as its doc says) | R0 |
| **C** — *delivered 2.32.0 (#597), §3.5* | Companion WALs (tuple space, blackboard) | compaction is crash-durable (temp file and directory fsynced); a torn or undecodable record poisons the writer and is refused at open rather than silently dropping every later record; one owner per file | R0 |
| **D** — *delivered 2.32.0 (#599), §3.5* | WASM execution limits | epoch interruption bounds a guest call; guest code runs off the runtime's worker threads; a trap does not recompile the component on every call | R0 |
| **E** — *delivered 2.33.0 (#613), §3.5* | Gateway remainder | client-supplied timer periods (`interval_secs`, `lease_secs`) bounded; the log-group subscribe keeps the core's `(hlc, key)` tiebreak and saves the group offset only after the entry is sent (today it saves first, so an entry in flight when a stream drops is never re-sent — found by row G); the mailbox deletes an entry only after the client has it (today it tombstones on entering the 256-slot channel, so a dropped stream loses queued events — #595's review); prompt keys cannot alias across `ns`/`name` (`prompts/{ns}/{name}` with a `/` in either); the caller envelope binds freshness (a replay inside the window *not built* — no nonce cache) | P5 |
| **F** — *delivered 2.33.0 (#611), §3.5 (window; the flip is F2)* | Wire and crypto, one MINOR with a window | a TLS node refuses unsigned `Data` frames; the removal check reads a peer's key structurally, not by byte-scanning the certificate; KV `SignedData` is domain-tagged; each change carries a mixed-fleet window and an upgrade note in the 2.30.0 shape; the #585 and #587 windows close in the same MINOR (#587 opened none — §3.5 F) | R0, ideally its own release |
| **F2** — *delivered 2.34.0 (#624), §3.5* | The refusal unconditional (the MINOR after F) | a TLS node refuses an unsigned `Data` frame and a bare KV `SignedData` signature whatever `kv_require_signed_frames` says (the setting retired or kept as a no-op, stated); the window counters `kv_unsigned_frames_accepted` and `kv_untagged_signatures` stop rising or are removed, stated in `deprecations.md`; `mesh.signed_writes` resolves `enforced` under `[tls]` and is considered for `secure-single-domain`'s required set; the anti-entropy residual (an unsigned, authorless `StateResponse`) is either closed or restated | F released |
| **G** — *delivered 2.32.0 (#595), §3.5* | SDKs | Python whole-second timeouts and a non-truncating tuple `take`; path segments escaped; TypeScript `SupersededError`; one `scatterGather` default, documented; SSE reconnect is designed with a gateway resume point or recorded as not built | R0 |
| **H** — *delivered 2.33.0 (#609), §3.5* | Companions | the commitment deadline judged on the log's clock, not the offerer's — **delivered as the declarer's signed close** (no clock in the log is offerer-proof, §3.5); `award_of` verifies before it reports (the declarer pinned by the caller; the unverified path stated, *not built*); the reason blob store bounded in total (eviction *not built*); caller-chosen `run_id` validated | R0 |
| **I** — *delivered with 2.33.0 (private #30), §3.5; Cedar narrower* | Private repo P3s | RA5 correction chains and competing corrections; attempt keys carry the operation id; a lock-order table; the outbox directory fsynced at create; Cedar policies schema-validated at load; the CI pin check refuses `[patch]`, `rev =` and multi-line tables | R0's re-pin |
| **J** — *delivered 2.33.0 (#608), §3.5* | Hygiene | the sim-seam gate's test-skip bug (the one the KV gate fixed in #591); every feature and bench linted or built by some CI step; the two CI jobs on `stable` pinned; unreferenced public items deprecated, not removed (nothing is removed on the 2.x line); only the duplicates that disagree are unified (`hex_line`'s whitespace, `now_ms` copies outside the seam) | — |
| **K** | Policy over history | the action evaluator can decide on what an actor has already done, not only on the request: a bounded, read-only history view (the actor's recent decisions and executions from the evidence journal — `src/agent/evidence_journal.rs`) reaches `ActionEvaluator::evaluate` beside the envelope (`ActionEnvelope`, `src/agent/action_evaluator.rs:101`, which today carries no history); a rule such as *at most three posts in ten minutes* is expressible and tested in the reference evaluator and the AE contract suite (`mycelium::ae_contract`); the scope of a count is stated by name — **per node** (the journal is node-local and never gossiped) unless the design question in §5 is decided otherwise; a history that cannot be read is `Indeterminate`, never an empty history; replay records the history view as a decision input | design question §5 |
| **L** | Credentials attached at egress | a secret need not enter a component's or an activation command's environment: an outbound request names a credential binding, and the egress gate (`EgressPolicy`, `mycelium-core/src/config.rs:280`, which today checks the host and every redirect hop) attaches it on the way out — bearer, header and basic forms at least — only to the host the binding names; a redirect off that host drops it; the binding's use is counted and its value never logged, traced or written to the evidence journal; the places a secret is read into the node today (`[[serve]].api_key_env`, federation credentials, an activation command's environment) are listed and each either moved behind a binding or stated as unchanged *From NVIDIA OpenShell (2026-10-12):* the component holds an opaque placeholder and never the value — the gate resolves the placeholder only at an endpoint the binding authorises — so a leaked environment or log carries nothing usable. | — |
| **M** | Activation commands confined | an `[[activation]]` command — today "not a sandbox: there is no allowlist and no confinement" (`docs/reference/unit-file.md:276`) and run with the stem's privileges — can be run under an OS confinement profile the unit file names (macOS Seatbelt first, Linux namespaces/Landlock or seccomp where available): filesystem access limited to the placed artifact's directory and declared paths, network limited to declared hosts or none; a profile the platform cannot apply refuses the activation by name, never runs it unconfined; the unconfined default stays, stated, and the guarantee report says which applies *From NVIDIA OpenShell (2026-10-12):* a profile's filesystem and syscall limits are fixed for the activation's life and its network allow-list may change live (OpenShell's split: filesystem and process locked at creation, network hot-reloadable); OpenShell itself (containers, seccomp, an unprivileged identity; needs Docker) is a candidate backend for a stem, beside a native Seatbelt/Landlock profile, rather than something to rebuild. | — |
| **N** | A2A at the current specification | an A2A client built on the current `a2a-sdk` (1.2.2, the library Strands Agents uses) reaches Mycelium's skills through `/a2a`, with the same authority as today — **the 2026-10-12 probe found it cannot**: the card is served only at `/.well-known/agent.json` (the SDK reads `/.well-known/agent-card.json`, 404); the card's `url` is the node's **gossip** address, not its gateway, and omits `/a2a` (`src/agent/a2a.rs:445`, so the SDK's `send_message` failed with a network error); the methods are the pre-0.2 `tasks/send` / `tasks/sendSubscribe` / `tasks/get` / `tasks/cancel` (`SendMessage` and `message/send` both answer `-32601`); and the skill is chosen by a Mycelium-only `skillId` parameter that a standard client never sends. Promises: the card at `/.well-known/agent-card.json` (the old path kept for one release), naming the gateway's own URL and scheme (https under `gateway_tls`) with `/a2a` and its supported interfaces and protocol versions; the A2A 1.0 methods (`SendMessage`, `SendStreamingMessage`, `GetTask`, `CancelTask`) and the 0.3 names (`message/send`, `message/stream`) served beside the old ones, which stay for one release with a deprecation entry; a message reaches a skill without `skillId` (a declared default skill, or the skill named in the message's metadata — decided in the PR, stated on the card); every new method runs the same doors as `tasks/send` — the action evaluator, mandates, evidence, task ownership — and the C7 bypass matrix gains each new method; the probe becomes a CI test driving the real `a2a-sdk` client against a node | — |
| **O** | A Strands session store on Mycelium | a companion package (Python, beside `langgraph-checkpoint-mycelium`) implementing Strands Agents' `SessionManager` over the Mycelium Python SDK, so an agent's conversation and state, and a swarm's shared memory, live in the fleet's KV rather than a file or a bucket; a save returns the SDK's receipt and its docs say which rung it names (a local application, not replication — the checkpointer's wording); an incomplete or unreadable session is an error, never an empty one; concurrent agents in one session do not lose each other's writes, or the limit is stated; its live suite runs in CI against a real node; pinned to a Strands release and re-checked when Strands changes the interface | — (N for the cross-host demo) |
| **P** | A Strands fleet, demonstrated | a runnable example, in CI like the co-op demos, in which Strands agents on three Mycelium nodes plan one shared task (a constructive domain — food redistribution or a microgrid), keep their session and swarm memory in the fleet through O, and call fleet skills through `/a2a` (N): a call outside an agent's mandate is refused and the refusal is in the evidence journal; one node is stopped and the work resumes on another from the stored session; the README states what Strands provides and what Mycelium adds, and which properties the run shows versus which it does not | N, O |
| **Q** — *delivered 2.33.0 (#616), §3.5* | Revocation reaches every verifier | a key a node has revoked (`rotate_identity_on_compromise`, `sys/revocation/`) is filtered from the `peer_keys`-based KV `SignedData` verifier, from identity-proof chaining (`validate_and_merge_identity`'s trusted signer) and from the unsigned identity exemption (`connection::self_authenticating_identity`), not only from consensus and `known_verifying_keys`; a test that a `SignedData` signed by a revoked key is refused at a peer that has seen the revocation; `src/agent/revocation.rs`'s `revoke_key` doc made true (corrected in #611 to say what holds today) | — (independent of F2) |
| **S** — *delivered 2.34.0 (#623), §3.5* | Identity proofs on by default | `require_identity_proofs` defaults to `true` (today `false`, `mycelium-core/src/config.rs`; the `secure-single-domain` profile already requires it as `id.proofs_required`): a bare `sys/identity/{N}` key list without a proof is refused everywhere a node's keys are learned, closing the identity-poisoning residual rows F and Q state — a member writing its own key into another's identity, which still reaches the verify-only history set (forged audit history, a steered checkpoint anchor) and fully impersonates a node on a legacy certificate; a mixed-fleet window in the 2.30.0 shape (the MINOR that flips it says so; a node from before 2.3.0, which cannot prove, is cut off until upgraded, and `false` stays available as an explicit opt-out for that window and for TLS-less development fleets, where the setting is inert); an upgrade note in `deprecations.md`; the guarantee `id.proofs_required` resolves `enforced` by default; the 2.14.0 attempt that was reverted is re-examined first (the failure it was blamed on was not its doing) | — (best in the MINOR with F2) |
| **T** — *delivered 2.34.0 (#622), §3.5* | Say loudly when the CA key is on the node | the dev posture keeps the fleet CA's private key on every node (`auto_cert_dir`'s `ca-key.pem`, `mycelium-core/src/tls.rs`), so one compromised host yields every node's identity and revocation (row Q) cannot recover it; today this is only the guarantee `id.ca_key_off_node` resolving unmet, one line among the start-time report, worded for member removal alone. Promises: at `start()` a node that holds the CA key and runs no profile, or a profile other than `dev`, logs one prominent warning naming the consequence (a single host compromise compromises every identity in the fleet; key revocation does not recover it) and the remedy (`mycelium tls issue` on a separate CA host, then `[tls] cert_pem` + `key_pem` on the node — `docs/operations/cert-rotation.md`); the `id.ca_key_off_node` guarantee text names identity compromise and revocation, not only removal (catalogue regenerated); `dev` states the trade-off rather than warning; the quick-start and production-readiness pages say which posture they set up; a test pins the warning on and off | — |

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
| **F** (wire and crypto MINOR) | `deprecations.md` (close the #585/#587 windows — #587 opened none, §3.5 F); the threat model's Boundary A; `what-is-proven.md`'s KV `SignedData` residual |
| **every row** | the CHANGELOG entry, a dated wiki `.log/` entry, and any `what-is-proven.md` row the change moves between tables |

The next `doc-coverage` run after each MINOR checks this table: a page still saying *not built* for a delivered row is a
calibration entry.

**P2's row, checked at the cut (2026-10-11):** the decision record's *What is enforced today* (§3) carries a
*Since P2* block — the electorate requirement, identity pinning and freezing, nothing resizing an electorate group, the
cross-group refusal, the profile — and its §4.1 table, which still read *not built* for P2, now reads *delivered in
2.32.0, without migration* (fixed at the cut); `what-is-proven.md` moved "Agreement across an electorate change" out of
the third table, adding the gated row to the first and "Changing an electorate's members — not built" to the third
(its *unreleased* marker moved to 2.32.0 at the cut); `threat-model.md` §7 names the electorate group and
`consensus_require_electorate` as the supported profile; guide 04 § *Discovery is not an electorate* states the
requirement and the frozen membership; the FAQ's electorate answer likewise; `runtime-invariants.md` § Layer III has
the *Since P2* paragraph — all in #601 except the two markers fixed here.

**C1/C2's row, checked at the cut (2026-10-11):** the decision record `consensus-electorate.md` (*Roles and outputs
decay* — "rows C1, C2 — **delivered**, PR #600", and its §4.1 table); philosophy's "A protocol, not a service" paragraph
("post-360 plan rows C1 and C2, delivered" — no *not built* left for them; P2's *not built* stayed, correctly, until #607 struck it once the migration had landed, #606); guide 04
(`elect_leader` "**Leased** (30 s by default, since 2.32.0)", `elect_leader_with`, `release_leadership`);
`runtime-invariants.md` § *Acceptor memory* ("shrinks to a node-owned floor once its decision is over", C2's exact
condition, the code named) — all done in #600.

**P1's row, checked at the cut (2026-10-10):** README's default-posture sentence (§ *Security*: "off loopback it refuses
to start without a credential, unless `gateway_allow_unauthenticated` says otherwise") and `positioning.md`'s ("a
non-loopback one refuses to start without a credential unless `gateway_allow_unauthenticated` is set") — done in #598;
`production-readiness.md` (rev 3's `gw.exposed_closed`, the refusal naming `http_addr`, the opt-in) — done, its
*unreleased* markers moved to 2.32.0 at the cut; `configuration.md` (`http_addr`, `gateway_auth_token`,
`gateway_allow_unauthenticated`, both token tables) — done, likewise re-marked; the guarantee catalogue golden
(`docs/reference/guarantee-catalogue.md` and `.json` carry `gw.exposed_closed`; regenerated and gated by the catalogue
test) — done. Every row: the CHANGELOG entries and dated wiki `.log/` entries landed with each PR.

### 3.5 · Delivered in 2.32.0 and 2.33.0 — each promise beside its evidence

Read against the merged code on `main` at `6b675720`, not against the PR descriptions The 2.33.0 rows (P2's migration, Φ, J, P5, I, H, E, F, Q) were re-read at the cut against `main` at `b5e07a9d` (private `b6d6faa`) by reviewers who wrote none of the PRs: every cited test and function exists; the wording fixes they found (a miscount, a mis-cited counter, `sys/revoked/`, the #587 window that never opened, three stale limits) are made in place.

**G — SDKs (#595, with #583 underneath).**
- *Python whole-second timeouts* — `mycelium-py/src/mycelium/_pool.py` `whole_seconds` (rounds up, never below 1,
  refuses negative/non-finite/past-`u64` by name); `mycelium-py/tests/test_sdk_edges.py`
  `test_a_mesh_timeout_is_sent_in_whole_seconds_rounded_up`, `test_wiki_ingest_sends_whole_seconds`,
  `test_a_timeout_the_gateway_cannot_read_is_refused_by_name`, `test_a_lease_or_ttl_is_sent_in_whole_seconds`.
- *A non-truncating tuple `take`* — `test_a_tuple_take_does_not_truncate_its_park` (Python);
  `mycelium-ts/tests/sdk_edges.test.ts` "tuple take(%p) parks for %p whole seconds" (TypeScript).
- *Path segments escaped* — `test_a_caller_supplied_value_stays_one_path_segment`,
  `test_a_segment_that_cannot_travel_is_refused_before_any_request`, `test_update_prompt_escapes_its_segments`;
  the TypeScript "keeps %p one path segment" and "a server-issued handle or guard id is escaped too";
  `langgraph-checkpoint-mycelium/tests/test_blob_path.py` (the blob id that redirected the bearer, dot and
  non-string ids read as `corrupt`).
- *TypeScript `SupersededError`* — `mycelium-ts/src/agent.ts` `export class SupersededError`; "%s that lost throws
  SupersededError" and its plant "%s: other refusals are not SupersededError".
- *One `scatterGather` default, documented* — `mycelium-ts/src/agent.ts` `min_ok: options.minOk ?? 1` with 10 s, its doc
  comment, both READMEs; "scatterGather waits for one reply by default…" and the Python
  `test_scatter_gather_waits_for_one_reply_by_default`.
- *SSE reconnect designed with a resume point, or recorded as not built* — **recorded as not built**: guide 10
  § (line 355, "A dropped stream is not resumed — not built: the gateway has no resume point"),
  `mycelium-py/README.md` and `mycelium-ts/README.md` say the same and what each stream loses. The resume point
  itself is gateway work (row E).

**P3 — `#![deny(unsafe_code)]` in `mycelium-core` (#596).**
- *The attribute in core* — `mycelium-core/src/lib.rs:14`; also every companion library (`-sim`, `-blackboard`,
  `-wiki`, `-wasm-host`, `-agentfacts`, `-reason`, `-guardrails`, `-effects`, `-commitment`, the coop library);
  the root crate and `-tuple-space` already had it.
- *The one production site replaced by `zeroize`* — `mycelium-core/src/erasure.rs` (`dek.zeroize()`, no
  `write_volatile` left); `destroy_wipes_the_key_it_removes`, and `install_key_wipes_the_key_it_replaces` for the
  replaced-key path the PR found.
- *Test-only `set_var` sites scoped* — the workspace's only `#[allow(unsafe_code)]`s are
  `mycelium-core/src/config.rs` `set_test_env` (inside `#[cfg(test)] mod tests`, takes the `env_test_lock()` guard)
  and one statement in `mycelium-wasm-host/src/stem.rs`'s test module, each with its `SAFETY:` comment
  (`grep -rn 'allow(unsafe_code)' --include='*.rs'`). Outside the promise and stated: `examples/conway.rs` still uses
  `unsafe`.

**C — Companion WALs (#597).**
- *Compaction is crash-durable (temp file and directory fsynced)* — tuple space
  `compaction_syncs_the_directory_after_the_rename`, blackboard `compaction_syncs_the_temp_file_and_the_directory`;
  and beyond the promise, compaction folds the log under the WAL lock
  (`a_compaction_between_append_and_apply_keeps_the_acknowledged_put` / `_post`) and keeps the id high-water mark
  (`a_restart_never_reuses_an_acked_id`).
- *A torn or undecodable record poisons the writer and is refused at open rather than silently dropping every later
  record* — a failed append poisons (`a_failed_append_never_strands_a_later_acknowledged_put` / `_post`; on a
  secondary, `a_poisoned_mirror_repairs_and_keeps_every_later_record`); a whole frame that does not decode, with data
  after it, refuses the open with the file untouched (`a_corrupt_middle_record_refuses_the_open_and_leaves_the_file`);
  a frame the file ends inside is still a crash's torn tail and is truncated (`a_valid_kind_followed_by_zeros_is_a_torn_tail`).
  **Not built, stated in the CHANGELOG:** a length prefix corrupted to run past the end of the file still reads as a
  torn tail (the format has no checksum), and there is no `on_unreadable = "quarantine"` counterpart.
- *One owner per file* — `mycelium::OwnershipLock` on `<wal>.lock` (`mycelium-tuple-space/src/store.rs`,
  `mycelium-blackboard/src/wal.rs`); `a_second_owner_of_the_wal_is_refused` in both crates.

**D — WASM execution limits (#599).**
- *Epoch interruption bounds a guest call* — `mycelium-wasm-host/src/host.rs` `cfg.epoch_interruption(true)`,
  `DEFAULT_CALL_DEADLINE` (5 s), `with_call_deadline`, `[hosts].call_deadline_ms`;
  `mycelium-wasm-host/tests/e2e.rs` `a_guest_call_past_its_deadline_is_stopped_by_name`.
- *Guest code runs off the runtime's worker threads* — `runtime.rs` (`spawn_blocking` around each call, and install's
  compile, instantiation and `describe`); `provisioner.rs`
  `a_long_guest_call_does_not_block_another_task_on_a_current_thread_runtime`.
- *A trap does not recompile the component on every call* — a trapped instance is replaced from the install's compiled
  component (`WasmHost::compiles()`); `a_trapping_guest_called_repeatedly_compiles_once`.

**A — Locks and leases, with C1 and C2 (#600; design record `docs/design/lock-lifecycle.md`).** The interleaving tests
withhold real frames with the test-only `test-frame-filter` (enabled in CI's default lib run and in `make check-full`).
- *A released lock whose tombstone was garbage-collected is not re-committed to its old holder* — the decision record
  ends by a release marker no record write can remove (`src/consensus_life.rs`); `row01_a_released_lock_is_not_recommitted_after_collection`
  (`src/agent/lock_lifecycle_tests.rs`), and `growth_a_thousand_acquisitions_leave_constant_keys`.
- *A reopened lease does not adopt the expired holder's value before the decided floor arrives* —
  `row02_a_reopened_lease_does_not_adopt_the_expired_value`; an adopter re-commits the holder's window, never its own
  (`row22_an_adoption_keeps_the_holders_lease`); a legacy value adopted in flight is wrapped in a lease
  (`f1_a_legacy_released_permanent_is_not_resurrected`).
- *A late COMMIT does not re-stamp a released lock fleet-wide* — `CommitTerm` carries the whole envelope and an upgraded
  learner ignores the legacy `Commit`; `row03_a_late_commit_does_not_revive_a_released_lock`,
  `row07_a_legacy_restamp_of_committed_is_not_read`, `row23_a_late_commit_does_not_displace_a_permanent_decision`.
  **Stated, not closed:** an *older* reader still reads the legacy keys the old way until it is upgraded.
- **C1** *`elect_leader` is lease-based by default with a release path (permanence stays available, opt-in)* —
  `DEFAULT_LEADER_LEASE` (30 s), `release_leadership` (`src/agent/consensus_handle.rs`), `elect_leader_with(group,
  LeaderTerm::Permanent)`; gateway `ttl_secs` / `permanent` and `DELETE /gateway/overlay/elect/{group}`;
  `c1_elect_leader_is_leased_by_default_with_a_release_path`, `c1_a_dead_leaders_lease_lapses_and_a_new_election_succeeds`,
  `row08_a_release_ends_every_renewal_of_its_lineage`. **Not built:** an SDK release verb (row G's follow-up).
- **C2** *acceptor state is collected once its decision or lease is over, without weakening the promise a live slot
  depends on* — `run_acceptor_collector` / `collect_finished` / `shrink_acceptor` (`src/consensus.rs`);
  `c2_acceptor_state_shrinks_to_a_floor_once_its_decision_is_over`, `c2_the_shrink_rechecks_the_promise_inside_its_compare_and_set`,
  `c2_collection_is_bounded_per_pass`, `row27_collection_keeps_a_node_owned_floor`. **Not built:** collection on a node
  that proposes without a listener.

**B — Core resource bounds, as narrowed by the maintainer on 2026-10-11 (#602).**
- *A handshake and idle timeout on the gossip accept path, and read-progress bounds with a 1 KiB/s floor* —
  `handshake_timeout_ms`, `inbound_idle_timeout_secs`, `peer_read_stall_timeout_ms`, `peer_min_rate_bytes_per_sec`
  (`mycelium-core/src/config.rs`, `stall.rs` `StallGuard`); `idle_sockets_that_never_speak_are_closed_and_a_new_peer_connects`,
  `a_one_byte_trickle_is_closed_by_the_rate_floor`, `one_byte_then_silence_is_closed_with_the_floor_off`,
  `a_frame_that_stalls_mid_body_is_closed_and_counted_apart`, `a_large_first_frame_trickling_steadily_is_read_past_the_handshake_bound`
  (`src/resource_bounds_tests.rs`); `a_header_claiming_a_large_frame_does_not_reserve_it_upfront` (`framing.rs`).
- *The accept path is bounded against silent and slow connections; an attacker that keeps talking can hold the slots it
  obtained from free permits, mitigated by mTLS and an opt-in per-source cap* — the residual is stated on
  `max_connections` in `configuration.md`, in the CHANGELOG and on `what-is-proven.md`'s third table; the cap:
  `max_connections_per_source`, `the_per_source_cap_closes_a_second_connection_from_one_address`. The mTLS mitigation is
  the existing handshake under `handshake_timeout_ms`; no new test asserts a non-member's handshake failing in time.
- *Connect, write and flush timeouts on the outbound writer* — `WriterTiming` (`mycelium-core/src/writer.rs`: connect +
  TLS under `handshake_timeout_ms`, writes and flush under `peer_write_stall_timeout_ms`, 1 140 s, from the measured
  760 s worst chunk, `worst_case_chunk_apply_with_an_fsync_wal`, `#[ignore]`d, a measurement);
  `a_stalled_write_times_out_and_the_queue_drains`, `a_peer_that_never_reads_does_not_block_eviction` / `…_shutdown`,
  `a_slow_reader_that_keeps_reading_receives_a_large_frame`. **No dedicated test** for the connect timeout itself
  (code: `writer.rs`, the `Connect … timed out` branch).
- *No unbounded pile of anti-entropy replies behind a stalled peer* — one reply in flight per peer;
  `a_peer_that_never_reads_gets_one_anti_entropy_reply_at_a_time`.
- *One stalled SSE or serve subscriber cannot veto admission of its kind node-wide* — `fill_ratio` is the least-full
  work-bearing subscriber, SSE routes are taps (`mycelium-core/src/signal.rs`); `a_stalled_subscriber_does_not_stop_admission_for_another`,
  `a_kind_whose_every_worker_is_full_sheds_while_a_tap_reads`, `a_fast_tap_does_not_hide_a_saturated_worker`.
- *The signal log is bounded per sender-chosen kind* — 4096 kinds, 1024 senders per kind; `the_signal_log_stays_bounded_under_random_kinds`,
  `a_kind_first_seen_after_a_flood_is_still_tracked`, `a_query_pin_expires`; its `sys/quorum/` twin:
  `a_random_kind_flood_writes_no_evidence`, `one_kinds_evidence_is_bounded_by_the_sender_cap` (a rate, not a total —
  the keys are not collected, `deprecations.md` §30).
- *A stopped agent leaks no task context (`RpcRequestRx` ends at shutdown)* — `a_stopped_agent_frees_its_task_context`
  (`src/agent/rpc.rs`).
- **Removed in review, not built:** connection preemption and WAL group commit (rounds 3–5); a write-side rate floor
  (deliberately — the residual is on `what-is-proven.md`).

**P2 — A consensus electorate is a governed group, without migration (#601; decision record §8).**
- *Safety-sensitive consensus requires a governed group* — `consensus_require_electorate` refuses a proposal flagged
  `safety_sensitive`, or in `lock/`, `leader/`, `consistent/`, `capauthz/`, `electorate/` or a commitment award,
  outside an electorate group: `ConsensusResult::ElectorateNotGoverned`, 403 `electorate_not_governed`
  (`src/agent/electorate.rs` `SAFETY_SLOT_FAMILIES`, `src/consensus.rs`); `a_safety_sensitive_proposal_is_decided_only_by_an_electorate_group`,
  `the_exclusive_slot_families_are_safety_sensitive_by_definition`, `an_exclusive_outcome_at_the_gateway_needs_an_electorate_group`.
- *Whose membership moves only through a governance route* — **in 2.32.0, as decided, it did not move at all** (since #606 it changes only by a migration, each step a governance decision at `POST /gateway/govern/electorate`, `govern:write`): a genesis
  (`declare_electorate`, `POST /gateway/govern/electorate`, `govern:write`) is decided by every member it names, and a
  changed roster is `MembershipFrozen` (`409 membership_frozen`); `a_swap_without_a_migration_is_refused` (asserts
  `MembershipFrozen`). An undecided genesis is superseded only by the live roster
  (`an_undecided_genesis_is_superseded_by_one_naming_the_live_roster`, `a_genesis_supersedes_only_the_epoch_before_it_with_a_majority_of_it`).
  **Delivered after 2.32.0: changing an electorate's members by a migration that terminates** — declare the successor
  naming the group, retire the group into it, complete the carry — and the default moving by retirement
  (`docs/design/consensus-electorate.md` §8.3; `a_value_chosen_in_a_retired_group_is_carried_by_its_successor` and
  the sixteen tests beside it in `src/lib_tests.rs`, § *P2's migration*, among them `a_drop_needs_a_retirement_elsewhere_and_a_forced_drop_that_races_a_retirement_is_a_detected_conflict`; `after_the_completion_the_successor_survives_the_predecessors_departure_and_its_own_restart`
  is the acceptance test). **Not built:** disaster recovery of a group that lost its majority.
- *The secure profile checks it* — `cons.safety_profile` (rev 2), required by `secure-single-domain` **rev 4**;
  `the_consensus_safety_profile_resolves_on_the_electorate_requirement`, `the_secure_profiles_required_set_is_pinned`.
- *The electorate is named as a group, never as node identities* — `ElectorateDecl { group, epoch, members, … }`
  decided on `electorate/{group}/{epoch}`; the members are the vote filter, pinned by identity: a roster that differs
  is `ElectorateMismatch` (`electorate_roster_mismatches`); `votes_from_outside_the_electorate_are_not_counted`,
  `an_electorate_is_established_not_inferred`, certificates `a_forged_electorate_record_is_not_adopted`,
  `an_electorate_certificate_verifies_under_tls`.
- *Versioned electorates with joint-consensus transitions recorded as a later plan, not built here* — recorded (decision
  record §4.2, "not built, not scheduled").
- *Tension (a), governed is not fixed* — resolved by freezing; the governor and the emergent watcher leave an electorate
  group alone, `/gateway/mesh/group` refuses it 403 `governed_group`: `the_governor_and_the_watcher_leave_an_electorate_group_alone`.
- *Tension (b), trust slices as the name* — decided: the member set is the vote filter, a trust slice may narrow it and no
  longer names it (decision record §3).
- *The gateway-only boundary (an embedded `join_group` or `grp/` write)* — **detected, not prevented**: such a write
  makes the roster differ from the member set, and every proposal to the group is refused `ElectorateMismatch` and
  counted. Layer I still accepts the write (detection, not prevention).
- *The exclusive default set once* — `a_second_default_is_refused_by_name`, `two_defaults_fail_closed`,
  `a_local_electorate_setting_that_disagrees_with_the_fleet_is_refused` — and since #606 moved only by its holder's retirement (`passes_default`; `a_second_live_default_is_refused_by_name`); cluster-scoped exclusive slots fail closed while
  the default cannot be named: `a_tagged_cluster_proposal_for_an_exclusive_slot_fails_closed`; a cross-group proposal
  decides neither: `a_cross_group_proposal_does_not_decide_for_an_electorate_group`, `…_an_exclusive_slot`.
  `leader/{g}` and `electorate/{g}/…` only under `Group(g)`: `slot_names_group` at the door and the acceptor
  (`src/consensus.rs`) — **no test proposes `leader/{g}` under another group**; the scoping is code-only.
- **Stated limits:** two defaults declared concurrently within gossip lag stopped the exclusive verbs fleet-wide with no
  escape in 2.32.0 (since #606 the escape is to merge them into one successor, `two_defaults_are_merged_into_one_successor`); a lost majority stops the group (for the default, the cluster-scoped exclusive verbs); genesis
  assumes the members agree on the roster; certificates are unauthenticated without `[tls]`.

**Φ — Philosophy: "protocol, not service" (#594 the first paragraph; #607 its full statement; docs only).**
- *A paragraph in the corrected-litmus section stating §2's line* — `docs/philosophy.md` § *The corrected litmus*, the
  blockquote **A protocol, not a service**: "a *protocol* that every member of a group runs — whichever nodes are in
  the group run it, roles form per ballot, the coalition dissolves when the decision completes, its state is ordinary
  keys and signals, and Layers I and II know nothing of it. It is never a *service* that a named node provides" (§2's
  two bullets, and D1).
- *The deployment rule that no shape may make a named node set the place agreement happens* — the same paragraph,
  bold: "no shape — a profile, a configuration default, a unit file, a governance route, a document — may make a named
  node set the place where agreement happens", with how an electorate is named instead (a group, members fixed at
  genesis, changed only by a migration that terminates) and what 2.32.0 enforces of it (P2, C1, C2) and what came
  after (the migration, #606). No *not built* remains in the paragraph: C1/C2's was struck in #600 and P2's here, the
  migration having landed.
- *The wiki's runtime-invariants page cites it* — `docs/wiki/dev/architecture/runtime-invariants.md` § *Layer III
  posture*, the paragraph **Discovery is not an electorate; consensus is a protocol, not a service**: one sentence
  restating the rule with a relative link to `philosophy.md#the-corrected-litmus`.
- **Stated, not a promise of this row:** the rule is a document's; the code that keeps it is P2's, closed above.

**P1 — Refuse an exposed, unauthenticated gateway (#598).**
- *`start()` refuses by name when the gateway binds a non-loopback address with no credential model, unless an explicit
  insecure opt-in is set* — `src/agent/lifecycle.rs` (`GatewayExposure::Refused` → `InvalidField { field: "http_addr" }`,
  naming the credentials and the opt-in), `guarantee::gateway_exposure` / `gateway_credential_model`;
  `src/lib_tests.rs` `an_exposed_gateway_with_no_credential_refuses_to_start` (`0.0.0.0` and `::`, and blank tokens
  refused at `validate()`: `mycelium-core/src/config.rs` `a_blank_gateway_token_is_refused_in_every_form`,
  `src/agent/http/http_tests.rs` `resolve_token_never_matches_an_empty_presented_bearer`).
- *The opt-in warns once at start* — `lifecycle.rs` (`GatewayExposure::Waived` → one `tracing::warn!` in `start()`);
  the opt-in starting and classifying as `Waived` is tested
  (`an_exposed_gateway_starts_with_a_credential_on_loopback_or_with_the_opt_in`); **the warning itself is not asserted
  by a test.**
- *The guarantee report and the secure profile reflect it* — `gw.exposed_closed` (`guarantee.rs`), required by
  `secure-single-domain` **rev 3**; `the_secure_profiles_required_set_is_pinned`,
  `the_secure_profile_names_the_unauthenticated_opt_in`, and the report states in the start test above.
- *Loopback development is unchanged* — the `127.0.0.1` case of the start test (`not_applicable`) and
  `loopback_is_127_slash_8_and_colon_colon_1_and_nothing_else`.

**J — Hygiene (#608, released in 2.33.0).**
- *The sim-seam gate's test-skip bug (the one the KV gate fixed in #591)* — `scripts/check-sim-seams.sh` `count_in`
  takes `check-kv-namespaces.sh`'s brace-counting skip (exactly the item the attribute applies to, at any
  indentation); `scripts/test-check-sim-seams.sh` plants the shapes — `skip_oneline_mod`, `skip_indented_fn`,
  `skip_oneline_body` seen failing on the old gate (counted 0, 1, 0 for 1, 2, 1), `skip_more_attrs` a guard that passes
  on every version, `skip_whole_module` the control. **The review round (findings 1–3):** the first cut ended a skip
  only at a `;` or a body, so a `#[cfg(test)]` on a comma-terminated element hid the next item — live in
  `mycelium-core/src/signal.rs:400–401` (`trim_hook`, #602), which hid all of production `impl SignalLog` (`:671`,
  `:673`, `:693`); the skip also ends at a trailing `,` or an unbalanced `}` and strips block comments and raw strings.
  Plants `skip_field`, `skip_last_field`, `skip_enum_variant`, `skip_match_arm`, `skip_literal_field`,
  `skip_block_comment`, `skip_raw_string`, each seen failing on the first cut (counted 0 for 1); the same six shapes
  in `check-kv-namespaces.sh --self-test`, each seen unnamed before its filter took the fix (latent there). The
  remaining fail-open inputs are stated in both headers. **Follow-up, not built:** a not-yet-started skip ends at the
  first trailing `,`, so a test item whose header spans lines (multi-line parameters, `where`, generics, `let (a,`, a
  const array, `use a::{b,`) is partly scanned as production — a false failure, never a hidden site (one dormant
  instance, `mycelium-core/src/persistence.rs:558–574`); the fix is to balance `()`/`[]`/`<>` across an unstarted
  header before honouring the `,`. **What it caught:** the `#[cfg(test)]` on `ConsensusEngine::shrink_acceptor_for_test`
  (`src/consensus.rs:1592`, #600) hid the five timer sites after it to the `impl`'s close (`:4154`) — the gate
  counted 0 against a baseline of 5 since 2.32.0 and said only *progress*. Baseline rows moved: `capability_ops.rs`
  2→0, `intent.rs` 1→0 (the `now_ms` unification), `http.rs` 15→14 (stale on `main`). `signal.rs` stays 11: the old
  gate's 8 on `main` was the `trim_hook` field hiding three sites since #602, not sites removed.
- *Every feature and bench linted or built by some CI step* — the table is in the PR; the uncovered ones (`otel`,
  `gateway_overhead`; `metrics` on tuple-space, blackboard, effects, wiki; guardrails `gateway`, `metrics-export`;
  reason `compliance`; the coop examples' `metrics`, a `cargo check` in the coop-smoke job) are each on an existing job's line (`.github/workflows/ci.yml`; the two root lines
  also in `make check-full`). **What it caught:** the wiki's `metrics` lint found `mycelium` built with `metrics` and without `gateway` failing `-D warnings` — `mod emergent`'s allowance (`not(any(gateway, metrics))`, `src/agent/mod.rs:106`) lifts under `metrics` alone and the six explain items only `/gateway/explain` reads were dead; each now carries `cfg_attr(not(gateway), allow(dead_code))` and `cargo clippy --lib --no-default-features --features metrics` is a named line (embedded job, `make check`). `mycelium-core/test-support`: the root crate's dev-dependency, every root test step;
  `throughput`: the embedded job's `cargo check --all-targets --no-default-features`. **Not built:** a check that
  derives the table from the manifests and fails when a feature loses its step.
- *The two CI jobs on `stable` pinned* — `effects` and `commitment` → `dtolnay/rust-toolchain@1.96.0`, the
  version `rust-toolchain.toml` and the other seventeen pin; `testing.md`'s toolchain paragraph counts them.
- *Unreferenced public items deprecated, not removed* — nineteen items (eighteen names; `with_max_tokens` is two methods), each with the whole-tree `grep -rnw` in the PR,
  `#[deprecated(since = "2.33.0")]`, `deprecations.md` §37; nothing removed. The sweep found twenty items (nineteen names); the review
  found the private companion calling `AeEvidence::audit_outcome`, which is therefore not deprecated (the private repo
  was then searched for all nineteen names: no other use). `Preflight::is_inert`
  (`src/agent/http.rs`, now `src/agent/http/authz.rs`) is the twentieth, left for row P5's split; row E then deprecated it (§38).
- *Only the duplicates that disagree are unified* — `hex_line`: the wasm-host gateway's copy trimmed, the
  catalogue's did not (its caller did); one `pub(crate)` trimming decoder in `catalog.rs`, pinned by
  `hex_line_ignores_edge_whitespace_and_refuses_everything_else`. `now_ms`: `capability_ops.rs` (`now_ms`,
  `age_now_ms`), `intent.rs`, tuple-space `store.rs`, agentfacts `crdt.rs`/`lib.rs` read `SystemTime::now()`;
  `federation/edge.rs` read the seam; every one reads `sim_seam::wall_now_ms` now. Pinned structurally for the two
  `src/` files only (their baseline rows at 0); the tuple-space and agentfacts copies are outside the gate's scan and
  **pinned by no gate**; `every_now_ms_is_the_seams_wall_clock` checks the clock's unit and agreement but cannot tell
  an old copy from a new one outside `sim`. Under `sim` each read is now a recorded kernel choice (an older recording
  touching these paths would diverge; the checked-in corpus is core-only and unaffected). The agreeing copies in
  `examples/` and `tests/` are left alone.

**P5 — `src/agent/http.rs` split by route family (#610).** Read against the diff, not the description:
a line-map verifier (its script and output are in the PR body) maps every original line to its new file and compares them.
- *Behaviour-preserving* — the split is by **line range** from the original: every moved line is byte-identical
  (the verifier's output is in the PR body); the only token added inside moved code is `pub(super)` on 153
  column-0 items, plus `pub(super)` on `KvKeyQuery::key` (shared by `kv.rs` and the overlay's `consistent/get`).
  The non-moved lines are listed one by one in the PR: `mod.rs`'s module declarations, the sibling-module imports
  that keep `super::x` resolving one level down, the family re-imports and the unchanged re-exports; three
  `include_str!` paths in `http_tests.rs` one directory deeper; and the governance-route enumeration test reading
  `mod.rs` whole (it holds no test module) instead of slicing `http.rs` at its test module. No route added,
  removed or renamed (the router moved as one block), no scope changed (`required_scope` moved as one block).
- *One shared authorisation-and-evidence layer every family goes through* — `src/agent/http/authz.rs`: the
  `gateway_auth` middleware (applied once to `/gateway` and once to the gated node routes in `mod.rs`, exactly as
  before), `required_scope`, `ae_preflight` / `ae_record` / `ae_record_execution`, `refuse_protected_kind`,
  `refuse_unobservable_kind`, `refuse_protected_key`, `refuse_owned_stream`, `dispatch_refused`. Every family
  file reaches them through `use super::*;` — no family carries a copy.
- *The cross-door tests unchanged and green* — `test_c7_the_bypass_matrix_no_door_runs_revoked_work`
  (`src/lib_tests.rs`; the test untouched — the PR changed only two doc-comment paths in that file), `every_listed_public_path_is_mounted_and_public_and_gated_paths_are_not` (the
  public-surface check; `mod.rs`'s comments still cite it as `the_public_surface_is_exactly_the_documented_list`, a name
  no test has had since #587 — left byte-identical here, fixed in row E),
  `required_scope_table_maps_families_and_denies_by_default`, `scope_admits_exact_and_wildcard_only`,
  `every_governance_write_route_audits_its_change`, `node_level_routes_honour_scoped_tokens`, the `gateway_caller_tests`
  and `ae_seam_tests` modules — all moved verbatim (de-indented) and run under
  `cargo test --lib --features tls,metrics,a2a,llm`, `--features compliance` and
  `--no-default-features --features gateway` (results in the PR).
- *The lock-order table still complete* — `HttpCtx`'s three `Mutex` fields (`gateway_caps`, `lock_guards`,
  `served_rpcs`) are rows 15, 16 and 55 of `docs/wiki/dev/concurrency/lock-order.md`, each now naming its file; no
  `Mutex`/`RwLock` was added.
- *`required_scope` still complete* — the table moved as one block (`authz.rs`); `required_scope_table_maps_families_and_denies_by_default`
  and the public-surface test hold it to the router.
- *Every gate that greps the path re-pointed* — `scripts/sim-seams-baseline.txt` (`15 src/agent/http.rs` → four
  rows summing to 14, the count the single file had on the day: the 15 was one site stale before this change);
  `scripts/kv-namespaces-nonkeys.txt`'s three notes; the wiki-lint skill's
  `grep '.route("/govern/' src/agent/http/mod.rs`; `scripts/check-test-inventory.py` walks the new `#[path]`
  test module (a planted unrunnable test in `http_tests.rs` was reported by file); `check-sim-seams.sh` and
  `check-kv-namespaces.sh` exempt the test files by their `*_tests.rs` names, which is why the module `tests`
  lives in `http_tests.rs` under `#[path]`.

**I — the private repository's items (private PR #30, merged `b6d6faa`, 2026-10-12; two independent review rounds).**
Each promise and what delivers it:
- *"RA5 correction chains and competing corrections"* → `mycelium-ra/src/reconcile.rs`: a usage record's key is its
  identity (a readable prefix plus the sha256 of the whole record, citation included), so a self-citation or a cycle
  needs a hash fixed point; the usage in force is worked out by citation, so a chain resolves to its tip in every
  arrival order; `Refusal::CompetingCorrections` names two corrections of one record (equal amounts included) and the
  larger stands; a dangling correction can raise the cost, never lower it; an unmeasured correction supersedes
  nothing; `StubConsumer` refuses a second correction and counts a byte-identical redelivery as a duplicate. Tests,
  each seen failing first: chain, competing, self-citation, two-record cycle, a chain revisiting an amount, equal
  amounts, unmeasured, redelivery. Stated: the key is a wire contract on serde_json's bytes and the exact field set; a
  transport must not re-stamp `at_ms`; a dangling record keeps raising the cost after it is itself corrected.
- *"attempt keys carry the operation id"* → `AttemptGuard` keys on `(operation_id, attempt_id)` in reserve, settle,
  release and restore (two tests, seen failing first); substrate-minted attempt ids already embed the operation
  (`mycelium-core/src/receipt.rs`), so only caller-chosen ids could collide; the AE correlator reports such a
  collision `Finding::Incoherent { field: "operation" }` rather than merging it (`two_operations_sharing_a_caller_chosen_attempt_id_are_reported_not_merged_silently`, a characterisation test — it passed on the unchanged code).
- *"a lock-order table"* → the private `docs/wiki/dev/concurrency/lock-order.md` (five rows, one chain), gated by
  `scripts/check-lock-table.py` matching `Type::field`, tuple fields, aliases and `use … as` names, with a self-test.
  Stated: a struct with nested generics in its header is not parsed.
- *"the outbox directory fsynced at create"* → `Outbox::open`, and `ExporterStore::open` too, sync the new directory
  and its ancestors through an `fsync_dir` seam (two tests, seen failing first). The tests pin the calls; behaviour
  under real power loss is **not built**.
- *"Cedar policies schema-validated at load"* → **delivered narrower: a vocabulary check, not schema validation.**
  `check_vocabulary` refuses a policy naming an action outside the catalogue, or an entity or `is`-type neither a
  request nor the store can supply (ancestors of stored entities count as supplied), as
  `BuildError::UnknownVocabulary`. A Cedar schema type-check of `context.*` is **not built** (the context's types are
  undeclared, and open records need an experimental Cedar feature); template `when` literals are **not built**.
- *"the CI pin check refuses `[patch]`, `rev =` and multi-line tables"* → `scripts/check-pin.py` parses every manifest,
  the cargo config and the lockfile, and recognises the substrate by its git URL whatever the crate's name; its
  self-test refuses every staged bypass, the three named ones among them (the old grep step passed all three). A build directory is skipped
  only when git does not track its `CACHEDIR.TAG`.

**H — Companions (#609; read against the merged code).** The review's claims, each sized against the code:
- *The commitment deadline judged on the log's clock, not the offerer's* — **held, and the first fix was wrong**.
  `plan_award_from` filtered on `o.offered_at_ms <= deadline_ms`, a field the offerer writes. #609's first head judged
  on `hlc::physical_ms` of the offer's log key; the review (comment 6117910561, finding 1) showed that segment is
  offerer-written too — `kv().set("log/cn/closed/offers/0000000000010000/mallory", …)` won an award a minute late, and
  the drift clamp applies to `Hlc::observe`, not stored keys. **No clock of the log is offerer-proof, so the promise is
  delivered as the declarer's close** (`mycelium-commitment/src/lib.rs`: `Close`, `ClosedOffer`, `close`, `close_of`,
  `verify_close`, `choose_award`): the declarer closes at or after the deadline on its own clock, naming every offer
  its node holds by log position and SHA-256; an award counts only named offers, and none is made before a close
  (`NotClosed`). **Round 2** (the review's confirming pass): one LWW key could not hold both a forged and a genuine
  close — a member's unsigned close written first stopped the declarer closing, and an overwrite was permanent. Closes
  now go on a stream `cn/{r}/closes`; the checked path takes the earliest close verifying under the pinned declarer
  (`close_of_verified`), `close` refuses `AlreadyClosed` only for a close verifying under its own key (a local read),
  the unverified path (`close_of`) requires the announcement's declarer, deadline and digest and a date at or after
  the deadline, a close binds its announcement, an award records its close (`close_sha256`), and an oversize close is
  `CloseTooLarge` before writing and read back after (`CloseNotWritten`). Seen failing on 9d4da849:
  `a_forged_close_written_first_does_not_stop_the_declarers_signed_close` (`AlreadyClosed`),
  `an_overwritten_close_is_recovered_by_the_declarer_closing_again` (`AlreadyClosed`),
  `on_the_unverified_path_a_close_dated_before_the_deadline_or_naming_another_declarer_is_refused` (the early forgery awarded),
  `an_oversize_close_is_refused_by_name_not_reported_as_written` (`Ok`, nothing written);
  `an_award_names_its_close_and_a_close_binds_its_announcement` is new API. **Round 3:** the round-2 recovery
  (re-close after an overwrite) recomputed from everything held and admitted a late offer, and "earliest" was by
  stream key, which any member writes. Recovery now **re-asserts** — `close` re-appends the close the handle made,
  byte for byte (`ContractNet::minted`, lock-order row 59), `reassert_close(&kept)` after a restart — and closes are
  ordered by the signed `closed_at_ms`, then digest. Seen failing on d90c2d51:
  `a_reclose_after_an_overwrite_never_admits_an_offer_the_first_close_did_not` (`a-mallory`),
  `a_second_signed_close_at_a_backdated_key_does_not_change_the_award` (`a-mallory`);
  `a_restarted_declarer_reasserts_the_close_it_kept` is new API. **Residual:** a declarer that restarts without
  keeping its close and is overwritten everywhere re-decides (the handle cannot tell an overwritten close from none);
  a declarer that signs two different closes has equivocated — the earlier-dated counts, both are on the stream; a
  forged unsigned close dated at or after the deadline still steers the unverified path; and (the round-3 review,
  stated rather than fixed) a close names offers by log position and hash, and the award re-reads them from the stream — so a member who deletes or rewrites another participant's named offer removes it from the candidates, and the award does not show it (the close and `close_sha256` are untouched). **Follow-up, not built:** carry each named offer's signed bytes inside the close and award from that copy. Tests (round 1, each first seen failing on
  461da69f): `a_backdated_log_key_does_not_make_a_late_offer_a_candidate`
  (the review's key loses; the offer that arrived before the close wins), `an_award_before_the_requirement_is_closed_is_refused`;
  and `no_offers_and_late_offers_are_visible_states_not_awards` pins the close's own refusals (before the deadline,
  not the declarer, closed once). **The residual**: the declarer decides which offers arrived in time — a late close
  admits late offers, an omitting one excludes; a signed close shows which offers it counted and makes that
  attributable, but `closed_at_ms` is the declarer's own claim, not a measurement. The close is unsigned on the plain
  path, like the head, and the plain path is steerable by any member that can append to the close stream. Finding 6 (HLC contagion) no longer applies: no HLC decides lateness.
- *`award_of` verifies before it reports* — **partial, stated**. `award_of` reports only a record whose
  `requirement` and `operation_id` name this requirement (`award_of_reports_only_a_record_that_names_this_requirement_and_its_operation`,
  seen failing on 2933073f). It cannot verify a signature without a key, so verification is
  `award_of_verified(requirement, declarer, resolve)` with the declarer **pinned by the caller** — the first head read
  it from the writable head and fell in two writes (finding 4;
  `award_of_verified_checks_the_declarer_the_caller_pins_not_the_writable_head`, seen failing on 461da69f).
  `plan_award_verified` decides `AlreadyAwarded` only from a verified award, and
  `commit_award_linearizable_verified` reports a slot holding an unproven award as `UnprovenAwardHolds`
  (`the_checked_path_needs_a_signed_close_and_is_not_blocked_by_a_forged_award`, new API); a slot holding another
  requirement's record is `SlotHoldsAnotherRecord`, not `Encoding` (`a_slot_holding_another_requirements_award_says_so`,
  seen failing on 461da69f). **Not built:** on the unverified path (`award_of`, `plan_award_from`, `award`,
  `commit_award_linearizable`) **any member that can write `cn/{r}/award` can block an award** — stated in their docs;
  a linearizable slot a forgery takes first stays taken. Sibling (finding 3): `offers`/`offers_verified`/`reports`/
  `assessments` read only records naming their requirement
  (`an_offer_report_or_assessment_naming_another_requirement_is_not_read_as_this_ones`, seen failing on 461da69f).
- *The reason blob store bounded* — **narrower than stated**: a per-blob ceiling existed (`MAX_BLOB_BYTES`, 8 MiB, at
  `FsBlobStore::put` and the gateway's 413); the **total** did not. `mycelium-reason/src/blob.rs`:
  `DEFAULT_BLOB_QUOTA_BYTES` (1 GiB), `open_with_quota`, `quota_bytes`/`used_bytes`, the reservation **per digest**
  before the write (`begin_write`/`end_write` over `account`, lock-order row 58 — the first head reserved per put and
  grew N×len under racing puts of one blob, finding 2: `concurrent_puts_of_one_digest_reserve_it_once` seen failing
  3/3 on 461da69f, the account at 38.8–50.9 MB against 16.8 MB on disk), released on every failure path
  (`a_failed_write_releases_its_reservation` — a pin; it held on 461da69f too), `.tmp-*` removed at open
  (`an_abandoned_temp_file_is_removed_at_open`, seen failing), each file at least one 4 KiB block
  (`BLOB_FILE_MIN_COST`, round 2 — an 8-byte blob cost 8 bytes of quota and a block, an inode and an account entry:
  `a_tiny_blob_costs_a_block`, seen failing on 9d4da849 at 8), `ErrorKind::QuotaExceeded`; `src/http.rs`
  `gw_blob_put` → 507 `quota_exceeded`. A file added or deleted by hand is credited at the next open (stated).
  Doors enumerated (`grep -rn "\.put(" mycelium-reason/src`): the gateway PUT, `MeshBlobStore::put`, the mesh
  write-back in `MeshBlobStore::fetch` (logged, bytes still returned). Tests
  `a_store_refuses_a_put_past_its_quota_and_counts_what_it_holds` (unit),
  `the_blob_door_answers_507_when_the_store_is_at_its_quota` (gateway) — new API, so seen failing as a missing method.
  **Not built:** eviction.
- *Caller-chosen `run_id` validated* — **held, and wider than a path segment**: `run_id` is read from a client at
  three doors (`grep -rn run_id mycelium-reason/src src mycelium-py/src mycelium-ts/src mycelium-wasm-host/src
  mycelium-effects/src`: `GET /gateway/reason/trace/{run_id}`, `POST /gateway/reason/route` body,
  `POST /gateway/reason/v1/chat/completions` body; the Rust API `TraceRecorder::new` and `replay`; nothing in
  `src/`, the wasm host, effects or the SDKs reads one) and used as a **log-stream segment** `reason/{run_id}/{node}`,
  never a filename — so `victim/x` wrote under `log/reason/victim/` and `replay("victim")` carried the injected event
  (`a_run_id_with_a_slash_cannot_write_into_another_runs_trace`, seen failing first with two events, one mallory's);
  `GET …/trace/victim%2Fx` answered 200 (`every_run_id_door_refuses_an_id_that_is_not_one_bounded_segment`, seen
  failing first). Now `validate_run_id` at every door (400 `bad_run_id`; `TraceRecorder::new` → `Result`), and
  `replay` reads only `{node}/{hlc}/{salt}` under the run, so a raw append under a deeper id (a member bypassing
  the recorder) is skipped — the read side closed as well as the write side. A `..` segment is refused at the door
  (proved with a raw request: a conforming client resolves `%2E%2E` before sending). **Validated, not owned**
  (finding 8): any `llm:invoke` caller can record into another tenant's valid run id — stated in the docs, not in this
  row's promise.

**E — the gateway remainder (#613, released in 2.33.0).** The row's cell runs several items together; read as six promises
(the cell's sentence "the log-group subscribe keeps the core's `(hlc, key)` tiebreak the mailbox deletes…" is two,
and its trailing "and saves the group offset only after the entry is sent" belongs to the log group, as row G found
it). Each test below was run on the code before its fix and failed with the line quoted; the fail-first runs are in the
PR.
- *Client-supplied timer periods (`interval_secs`) bounded* — `src/agent/http/caps.rs` `timer_secs` (absent →
  default; a whole number in `1..=MAX_TIMER_PERIOD_SECS` = 3600; else 400 `timer_out_of_range` naming the field) on
  both routes that read `interval_secs` and `lease_secs` (`gw_cap_advertise`, `gw_units_declare`);
  `NodeCapabilityConfig::validate()` holds a unit file's `ttl_secs` to the same range (`src/capability_config.rs`).
  Tests: `client_timer_periods_are_bounded_at_both_routes` (*was* `left: 200 right: 400 — capability/advertise
  interval_secs=1000000000: past the bound`), `a_unit_files_ttl_is_bounded_by_validate` (*was* `ttl_secs = 0` accepted).
  On the way: a gateway advert now gossips a `CapEntry` carrying its period (`a_gateway_advert_declares_the_period_it_refreshes_at`,
  *was* a bare `Capability` that `CapEntry::decode` refused); both SDKs send `interval_secs` as whole seconds
  (`test_a_lease_or_ttl_is_sent_in_whole_seconds[…interval_secs]`, `advertiseCapability.intervalSecs sends 1.5 as 2`,
  *was* `1.5` sent as given). The other gateway timers were already clamped (`timeout_secs`, `ttl_secs`, `timeout_ms`).
- *The log-group subscribe keeps the core's `(hlc, key)` tiebreak* — `mycelium-core/src/log_cursor.rs`
  (`LogPosition`, `rows_after`, `read_position`, `record`), used by the gateway (`src/agent/http/overlay.rs`
  `log_group_events`) and in process (`KvHandle::subscribe_log_group`); `log/scan` and `log/subscribe` sort the same
  way. Tests: `the_log_group_keeps_the_cores_hlc_key_tiebreak` (*was* `left: ["b", "c"] right: ["a", "b", "c"]`),
  `the_in_process_log_group_keeps_the_tiebreak` (*was* `left: ["a", "c"]`), and `log_cursor`'s two unit tests (the
  order; the cursor read only when at least as far as the legacy offset, which is still written for older nodes).
- *The mailbox deletes an entry only after the client has it, not on entering the 256-slot channel* —
  `src/agent/mailbox.rs` `open_mailbox_acked` / `MailboxAck` / `Sink::Acked`, acked in `mailbox_events`'s `map` as the
  stream yields (`src/agent/http/mesh.rs`). "The client has it" is stated as *handed to the HTTP connection*: written
  to the socket is not received, and guide 10 and both SDK READMEs say what a drop still loses. Test:
  `a_dropped_mailbox_stream_loses_only_what_it_handed_on` (*was* `left: 0 right: 9 — only the event the stream handed
  on is deleted`). **Not changed:** the in-process `ServiceHandle::open_mailbox`, which hands a plain `mpsc::Receiver`
  and still deletes on enqueue (the module doc says so).
- *Prompt keys cannot alias across `ns`/`name`* — `src/agent/prompt.rs` `prompt_key` (an empty part or a `/` →
  `PromptSkillError::InvalidName`), at every door that builds the key: the three `/gateway/prompts/{ns}/{name}` routes
  and `llm/call` (400 `invalid_prompt_name`), `llm/stream` (an error event), `LlmHandle`'s six verbs. Tests:
  `prompt_keys_cannot_alias_across_ns_and_name` (*was* `left: 200 right: 400 — the other pair never reads the first
  pair's template (PUT answered 200 OK)`), `in_process_prompt_names_cannot_alias` (*was* `update_prompt("a/b", "c")`
  accepted).
- *The log-group subscribe saves the group offset only after the entry is sent* — the gateway records in the stream's
  `map` after the channel, behind a `take_while` that stops a consumer that lost the claim; the core records after
  `tx.send` succeeds and exits when idle with its receiver gone. Tests:
  `a_dropped_log_group_stream_saves_only_what_it_handed_on` (*was* `left: Some("0000000000000104") right:
  Some("0000000000000100")`), `the_in_process_log_group_records_only_after_the_send` (*was* `left:
  Some(b"0000000000000100") right: None`).
- *The caller envelope binds freshness* — `src/agent/gateway_caller.rs` `verify`: issued more than
  `CALLER_CONTEXT_MAX_AGE_MS` (300 s) plus `max_clock_drift_ms` ago or more than `max_clock_drift_ms` ahead →
  `CallerError::Stale`; one verifier for every receive path. Test: `a_stale_or_future_envelope_is_refused` (*was*
  `left: Ok(Some(1791791309573)) right: Err("stale") — tls=false: two hours old`). **Not built:** a nonce cache, so a
  replay inside the window is not refused.
- *Left for this row by P5* — `mod.rs` cites `every_listed_public_path_is_mounted_and_public_and_gated_paths_are_not`;
  `GatewayPreflight::is_inert` `#[deprecated(since = "2.33.0")]` (no reference in this repository or
  `mycelium-private`, grepped); the SSE resume point stays *not built* — the guide's and READMEs' loss sentences now
  match what a drop loses.

- *#613's adversarial review, answered in one round* (each code fix with a test seen failing first on `80218501`):
  1. `max_clock_drift_ms = 0` is *no drift bound* for the envelope too — no future bound, the age bound the 300 s
     alone (`a_zero_drift_bound_bounds_only_the_age`, *was* `left: Err("stale") right: Ok(true)` for a gateway 50 ms
     ahead).
  2. Two mailbox streams of one kind share one in-flight set per `(node, kind)` (`TaskCtx::mailbox_in_flight`, held
     weakly; an un-acked event released on drop wakes the others) —
     `two_mailbox_streams_for_one_kind_never_hand_out_one_event_twice` (*was* stream B handed `AQ==`).
  3. The legacy offset is written one below the entry's HLC, so a reader with the offset and not yet the cursor
     re-delivers rather than skips; `record()`'s doc says what holds only on the writing node —
     `an_offset_that_arrives_before_its_cursor_never_skips_a_sibling` (*was* `position ThroughHlc(512)`).
  4. Every `prompts/` key builder goes through `helpers::prompt_kv_key` (`a2a.rs`, `llm.rs` included).
  5. `validate()` refuses a `[[serve]]` name that is empty or holds a `/` — `a_serve_name_that_cannot_be_a_prompt_skill_is_refused_at_load`
     (*was* accepted).
  6. The two `cap/` cases are §3.6's follow-ups, not this row's.
  7. §38 and the constant's doc state that the 300 s window is the gateway's: an in-process caller behind a backlog
     longer than that is refused at dequeue.
  8. "Never clamped" narrowed to the two routes and the unit file; lock/elect `ttl_secs` and `rpc/call`'s
     `timeout_secs` still clamp; both SDKs refuse a period past 3600 locally (*was* sent; 4 py and 4 ts tests).
  9. The `Stale` message's lost line continuation, the test doc's constant name, the `CONSUMER_LOG` doc.

**F — Wire and crypto, one MINOR with a window (#611, delivered in 2.33.0).** Read against the branch's code.
- *A TLS node refuses unsigned `Data` frames* — `mycelium-core/src/connection.rs`, the `Data` arm: under a TLS
  identity and `kv_require_signed_frames` the frame is dropped and counted (`kv_unsigned_refused`; `GET /stats` `kv_unsigned_frames_refused`, `mycelium_kv_unsigned_frames_total{outcome="refused"}`);
  `a_tls_node_requiring_signed_frames_refuses_an_unsigned_data_frame` (seen applying the forged key on the unfixed
  arm). **Not unconditional in this release** — the refusal is the setting, default off, which is the window
  (next bullet but one); the unconditional refusal is row F2's, *not built*. The twenty-three agent write
  sites that sent bare `Data` frames sign (`framing::local_kv_wire_msg`; `scripts/check-signed-dispatch.sh` in
  `make check` and CI, with `--self-test`); `an_agent_write_reaches_a_peer_that_requires_signed_frames` (seen
  failing with the refusal built and the sites unrouted). **Not covered:** anti-entropy entries carry no author.
- *The removal check reads a peer's key structurally, not by byte-scanning the certificate* —
  `mycelium-core/src/tls.rs` `ed25519_key_from_cert_der` (`webpki::EndEntityCert::subject_public_key_info`, compared
  with RFC 8410's header), used by `refuse_if_removed` and `stream.rs` `peer_ed25519_key`;
  `a_planted_key_pattern_in_the_serial_does_not_fool_the_structural_read`,
  `a_removed_member_hiding_its_key_behind_a_planted_pattern_is_still_refused`,
  `a_non_ed25519_certificate_has_no_ed25519_key_whatever_its_serial_carries` — all three seen failing on the scan.
- *KV `SignedData` is domain-tagged* — `framing::kv_signing_message` (`mycelium.kv/update/1 ‖ u32 len ‖ canonical`),
  signed in `make_kv_wire_msg`, verified in `connection.rs`'s `SignedData` arm;
  `a_kv_signature_is_over_the_tagged_message_not_the_bare_bytes`, `a_tagged_kv_signature_is_accepted_at_the_connection`,
  `a_signature_under_another_tag_is_not_a_kv_signature` (seen failing first).
- *Each change carries a mixed-fleet window and an upgrade note in the 2.30.0 shape* — the bare KV signature
  accepted and counted (`kv_untagged_signatures`), the unsigned frame accepted and counted
  (`kv_unsigned_frames_accepted`; `an_unsigned_data_frame_is_applied_and_counted_during_the_window`), both on
  `GET /stats` and as metrics; `deprecations.md` §35 states each direction ("an upgraded node's KV writes reach a
  2.32 peer only by anti-entropy until that peer is upgraded"; "an upgraded node requiring signed frames drops a
  2.32 peer's agent writes until that peer is upgraded"); CHANGELOG `[Unreleased]` § Security. The certificate read
  is local and has no window. The window counters are asserted on `/stats` (`the_row_f_window_counters_are_on_gateway_stats`).
- *The #585 and #587 windows close in the same MINOR* — §23 (#585: the identity proof and the consensus signature)
  is closed: `helpers::validate_and_merge_identity` and `consensus::verify_consensus_signature` accept the tagged
  form only, `SignatureForm`/`sign_payload_as` removed, both counters removed;
  `a_bare_identity_proof_is_refused_now_the_window_is_closed`, `a_bare_proof_signed_by_the_victim_is_refused_and_flagged`,
  `a_consensus_signature_verifies_only_in_the_tagged_form`, `an_acceptor_answers_only_a_tagged_request` (seen
  failing against the allowance). **#587 opened no window** (its commit, CHANGELOG entry and §25 accept nothing
  for one release; the consensus-signature tag the brief attributed to it is #585's second half) — so nothing
  was closed for it.
- *#611's review, fixed before merge* — (1) a signed frame's signer bound to its sender:
  `connection.rs` `SignedData` arm, `kv_signer_mismatch`; `a_signed_frame_whose_signer_is_not_its_sender_is_refused`
  (seen applying V's own `sys/role/` key and an app key under M's signature). (2) the quorum-evidence write
  signed and the gate narrowed to marked receive paths, any spelling, with a three-plant self-test (connection.rs,
  a new agent file, a `use`-spelled `Data(`); `quorum_evidence_reaches_a_peer_that_requires_signed_frames` (seen:
  A never held B's evidence; the 6081c9e6 gate called a tree with all three plants clean). (4) the flip is row F2;
  `mesh.signed_writes` in the guarantee report, not in `secure-single-domain`. (6) the removal check fails closed
  on a certificate with no Ed25519 key; `a_certificate_without_an_ed25519_key_is_refused_by_the_removal_check`
  (seen `Ok(())`). (8) `a_bare_2_32_kv_signature_is_accepted_and_counted_during_the_window`,
  `a_gateway_kv_delete_reaches_a_peer_that_requires_signed_frames` (seen failing with `kv_write` toggled bare),
  `the_row_f_window_counters_are_on_gateway_stats` — the counters are per node now (`TransportBounds`). (9)
  **The second review** reproduced impersonation and silencing through that round's identity exemption (it trusted
  `sender`); the exemption is now a self-authenticating sealed record only (`connection::self_authenticating_identity`,
  counted `kv_identity_exempt_frames`), a node publishes the sealed record first unsigned and its legacy pair signed,
  and an identity tombstone removes no keys — `an_unsigned_identity_forgery_cannot_make_a_member_sign_as_another`,
  `an_identity_tombstone_does_not_erase_a_members_keys` (both postures),
  `only_a_self_authenticating_sealed_identity_is_exempt_from_the_refusal`, all seen failing on 59d86368; the gate
  skips only an inline test module (fourth plant, seen passing the old gate); the per-frame warnings are rate-limited
  (`a_warning_gate_lets_one_through_per_interval_and_counts_the_rest`, a new helper, no fail-first run). **What
  remains with `require_identity_proofs` off:** another member's identity is writable under the writer's own
  signature, detected not prevented. **The third review:** N4 — a replayed pre-rotation sealed record replaced the
  current one; an exempt record must now contain every key held or stored for the node, a stale one refused and counted
  (`kv_identity_stale_refused`; `a_replayed_pre_rotation_sealed_record_does_not_replace_the_current_one`, the
  reviewer's test, seen failing on 884f310f); first sighting stays trust on first use. L1 — the gate blanks string and
  char literals before counting braces (fifth plant, seen passing the old gate; a multi-line string can still miscount,
  stated). L2 — stated in §35. N5 — pre-existing, recorded as row Q, `revoke_key`'s doc corrected. **CI's co-op
  demo 04** exposed a shutdown race the signed retraction made reliable (a provider's capability tombstone, sent as
  its handle dropped during shutdown, never left); graceful shutdown now drains queued frames before signalling —
  `a_capability_retracted_at_shutdown_reaches_the_peers`, seen failing first; the co-op suite 13/13 locally.
  `start()` warns when `kv_require_signed_frames` is inert (`kv_require_signed_frames_without_tls_is_inert`); the
  warning's emission itself is not asserted. (3, 5, 7) answered in the docs: the refusal is bypassable through
  anti-entropy; upgraded nodes sign tagged at once (the 2.32.0 shape) and reach 2.32 peers by anti-entropy only;
  revocation events join the untagged list.
- *Documentation follow-through* — `deprecations.md` §35 and the §23 closure note; `docs/threat-model.md` Boundary A;
  `what-is-proven.md`'s KV residual moved to the CI table (role claims and audit records remain in the third);
  `configuration.md`'s `kv_require_signed_frames` row; the identity-authentication and federated-domains records
  amended; wiki `dev/security.md`; `docs/wiki/dev/.log/2026-10-12-f-wire-crypto.md`.

**F2 — The refusal unconditional (#624, released in 2.34.0).** Read against the branch's code; the doors enumerated first
(`grep -rn 'WireMessage::Data\b\|WireMessage::SignedData\|StateResponse\|apply_inbound('` over `mycelium-core/src`
and `src` — one `Data` arm, one `SignedData` arm, one `StateResponse` arm in `connection.rs`, the codec, the fuzz
harness's decode-only `wire_frame_apply`; `grep -rn 'kv_require_signed_frames\|KV_REQUIRE_SIGNED_FRAMES'` — the field,
`Default`, the env override, `kv_require_signed_frames_is_inert`, the `start()` warning, the guarantee, the
`Data` arm, the tests, the docs; `grep -rn 'kv_unsigned_accepted\|kv_untagged_signatures\|mycelium_kv_'` — the two
`TransportBounds` fields, two `metrics::counter!` sites, `GET /stats`, the tests; no SDK, dashboard or alert reads
either counter).
- *A TLS node refuses an unsigned `Data` frame whatever `kv_require_signed_frames` says* — `connection.rs`, the
  `Data` arm: under a TLS identity every unsigned frame but the exempt ones is dropped and counted
  (`kv_unsigned_refused`), the setting read nowhere; `an_unsigned_data_frame_is_refused_whatever_the_setting_says`
  (seen applying the forged key on 4251d50a); a node without `[tls]` unchanged
  (`a_node_without_tls_still_applies_an_unsigned_frame`, passes on both — not a regression test).
- *… and a bare KV `SignedData` signature* — the `SignedData` arm verifies the tagged form only; a signature that
  verifies bare is counted (`kv_untagged_refused`, `GET /stats` `kv_untagged_signatures_refused`,
  `mycelium_kv_untagged_refused_total`) and named in a rate-limited warning; `a_bare_2_32_kv_signature_is_refused`
  (seen applying the key on 4251d50a).
- *The setting retired or kept as a no-op, stated* — **retired as a no-op**: `#[deprecated(since = "2.34.0")]`, still
  parsed from TOML and `GOSSIP_KV_REQUIRE_SIGNED_FRAMES`, `kv_require_signed_frames_is_inert` true wherever it is set
  and `start()` warns then (`kv_require_signed_frames_is_inert_wherever_it_is_set`, seen failing on 4251d50a: "set with
  [tls]" read as enforcing; the warning's emission itself is not asserted); a node that still sets it refuses the same
  (`a_tls_node_that_sets_the_retired_setting_refuses_an_unsigned_data_frame`). An explicit `false` is not an opt-out
  (stated, `deprecations.md` §40).
- *The window counters stop rising or are removed, stated in `deprecations.md`* — **stop rising**: both
  `TransportBounds` fields kept and reported on `GET /stats` at zero (nothing raises them; the API-shape rule), their
  metrics no longer emitted; asserted at zero in the two refusal tests, and still reported on `/stats` (`the_row_f_window_counters_are_on_gateway_stats`)
  (`the_row_f_window_counters_are_on_gateway_stats`); §40 and the §35 closure note.
- *`mesh.signed_writes` resolves `enforced` under `[tls]`* — rev 2, on `[tls]` alone, names the bare signature;
  `mesh_signed_writes_is_enforced_under_tls_by_default` (seen `not_configured` on 4251d50a). *Considered for
  `secure-single-domain`* — **added**, rev 5: it resolves on exactly what `mesh.tls` does, so rev 5 admits every node rev
  4 admitted (`the_secure_profiles_required_set_is_pinned`, the two refusal tests' revision).
- *The identity and certificate exemptions keep passing* — `only_a_self_authenticating_sealed_identity_is_exempt_from_the_refusal`
  now runs at the default (seen failing on 4251d50a: all five refused frames applied); the certificate record's
  exemption is exercised by row Q's multi-node tests, whose fleets no longer set the flag; the whole `lib_tests`
  module (263 tests, the multi-node set among them) 12× green.
- *Every agent write still reaches a peer* — `an_agent_write_reaches_a_peer_that_requires_signed_frames`,
  `quorum_evidence_reaches_a_peer_that_requires_signed_frames`, `a_gateway_kv_delete_reaches_a_peer_that_requires_signed_frames`,
  `a_capability_retracted_at_shutdown_reaches_the_peers` at the default; the co-op suite locally.
- *#624's review, four low findings* — (1) `advertise_locality()` ran before the TLS install, so its frame left
  unsigned: moved after it (`lifecycle.rs`); the only earlier writes are the two exempt records. A two-node test was
  written and run on 6d97464b and after the move: the frame reached B by gossip in neither (dispatched before any
  connection), so it was not kept — **not deterministic, stated**. (2) a refused frame consumes its nonce —
  pre-existing, stated in §40 and Boundary A, code unchanged. (3) `configuration.md`'s `profile` row says rev 5, 20 ids.
  (4) the bare-signature warning has its own per-connection gate; unsigned refusals stay at `debug`, reason in §40.
- *The anti-entropy residual closed or restated* — **restated, not built**: closing it needs a `SyncEntry` that carries
  its originator's signature — a wire change (v13) with its own window; restated in §40, the threat model's Boundary A,
  `what-is-proven.md`'s third table and the config field's doc.

**Q — Revocation reaches every verifier (#616, released in 2.33.0; decision record `docs/design/revocation-authority.md`
rev 3).** #616's first head (`e5c2c27e`) wired every verifier to one answer and was right about that; its adversarial
review found the *authority* behind it wrong (any held key revoked any other, for ever — a poisoned key, a first
sighting after a restart or any leaked retired key silenced a node, and a restart bypassed the chaining fix). Two design
reviews later (rev 1 narrowed; rev 2 the reviewer's minimal sound set; rev 3 its three named changes, plus links cut as
unproduced), the row closes as follows. The verifiers were enumerated with
`grep -rn "peer_keys\|known_verifying_keys\|revoked_key_set\|verify_bytes" src mycelium-core/src mycelium-*/src` and
`grep -rln "verify_bytes\|\.verify(&"`; the PR body has the table.
- *Filtered from the `peer_keys`-based KV `SignedData` verifier* — `connection.rs`'s arm verifies against
  `CoreCtx::trusted_keys` (an anchored node's anchored, unrevoked keys; a legacy node's retained set);
  `a_kv_write_signed_by_a_revoked_key_is_refused_once_the_revocation_is_seen` (seen failing on 7c967ba3).
- *From identity-proof chaining* — `validate_and_merge_identity_with_revocations` (a revoked signer chains nothing), and
  for an anchored node no chained key verifies unless a certificate anchors it;
  `an_identity_record_chained_through_a_revoked_key_is_not_merged` (7c967ba3) and
  `a_fork_from_a_revoked_key_is_not_trusted_after_the_verifier_restarts` (the review's restart bypass; seen failing on
  e5c2c27e).
- *From the unsigned identity exemption* — `self_authenticating_identity` refuses a sealed record whose signer the node
  revoked or, for an anchored node, has no standing; `a_sealed_record_signed_by_a_revoked_key_is_not_exempt` (7c967ba3).
- *Not only consensus and `known_verifying_keys`* — consensus (`decode_verify`, its raw-KV fallback removed),
  `known_verifying_keys`, caller envelopes, knowledge keys and `mycelium-agentfacts` read `trusted_keys`;
  agentfacts' `a_field_signed_by_a_key_written_into_the_raw_identity_does_not_verify` (seen failing on e5c2c27e);
  `a_tls_only_node_honours_a_revocation` (a `tls` build; passes on e5c2c27e too — the review's finding 7 was coverage).
- *A test that a `SignedData` signed by a revoked key is refused at a peer that has seen the revocation* — the first
  bullet's, and across the verifier's restart `a_revocation_still_filters_after_the_verifier_restarts`, and with the
  revocation hidden while the verifier was down `a_revocation_hidden_while_the_verifier_was_down_still_counts` (the
  journal; seen failing on e5c2c27e).
- *`revoke_key`'s doc made true* — `src/agent/revocation.rs`'s module doc states standing, who may revoke, the records,
  order, restart and hiding; `rotate_identity_on_compromise` and `revoke_identity_key` state what they rely on and do
  not cover.
- **Who may revoke (the review's findings 2 and 3).** Standing only from certificate records
  (`sys/identity-cert/{node}/{key}`, a CA-issued certificate naming the node by URI, issuance in the serial, bounded by
  `wall_now_ms` plus the skew — `identity_cert_future`); an anchored node trusts exactly its anchored keys; a revocation
  of P counts only when signed by an anchored key issued after P's. `a_poisoned_identity_key_neither_revokes_nor_signs_for_an_anchored_member`,
  `a_first_sighted_key_after_a_restart_cannot_revoke_an_anchored_member`, `a_retired_key_cannot_revoke_its_successor_and_rotation_continues`
  (each seen failing on e5c2c27e); `a_certificate_issued_in_the_future_outranks_nothing`, `a_legacy_ip_only_certificate_anchors_nothing`
  (guards on the new path). Revocations keyed `(P, S)`, self-contained, the v1 bytes first (`a_v2_record_is_a_v1_record_followed_by_a_trailer_both_readers_decode`);
  owner republish and the tripwire (`an_owner_republishes_its_overwritten_certificate_record`, seen failing on e5c2c27e).
  `deprecations.md` §39; guarantee `id.revocation_authority`.
- **The final review's round** (`38da0472`; named fixes only, each seen failing there): history verifies under a
  verify-only held-minus-revoked set (`an_audit_chain_sealed_under_a_key_without_a_certificate_record_survives_restarts`);
  first-issuance ranking, counted-stays-counted and the start refusal (`a_revoked_key_restored_at_its_node_does_not_regain_trust_at_a_peer`,
  `a_node_whose_key_pem_names_a_retired_key_refuses_to_start`); the fold's cost bounded before checks
  (`junk_revocation_records_keep_a_fold_bounded`); the clock-step floor (`a_rotation_after_the_clock_steps_back_still_outranks_its_predecessor`);
  adoption only under the node's own keys (`an_owner_does_not_adopt_a_revocation_planted_through_its_raw_identity`);
  idempotent revocation (`revoking_one_key_twice_writes_one_record`); `compute` merges (inspection — a race no test
  reproduces); the three test labels corrected. **The confirming review's N1–N2** (seen failing on 39cfb571):
  provisional revocations of unanchored keys and no signer the node counted as revoked
  (`a_thiefs_revocation_judged_before_the_successors_certificate_does_not_lock_in`), the own certificate anchoring
  first at start (`a_planted_record_does_not_refuse_a_start_whose_own_certificate_record_is_missing`), the start
  history from the node's own records (`a_raw_identity_value_a_member_writes_does_not_enter_the_owners_history`);
  N3 (the certificate cache clears rather than evicting) and N4 (a CA holder can refuse a node's start) stated.
  **The N1/N2 check's NEW-1 and NEW-2** (seen failing on 4908ecd3): audit recovery never seals over an existing record,
  reporting the unverified prefix (`an_audit_genesis_survives_an_upgrade_after_an_old_version_rotation`); the
  revoked-signer rule dropped (`two_compromise_rotations_learned_out_of_order_leave_the_oldest_key_revoked`).
  **The safety net's review, H1/M1/L1/L2** (seen failing on 81b70a2e): strict key/body reading, skip-occupied sealing
  with `checked_add`, the audit prefixes self-owned, segment-by-segment verification, a real unverified count, pruning
  that keeps unverified records (`a_forged_high_seq_record_neither_pushes_nor_wraps_the_numbering`,
  `a_far_away_unverified_record_is_reported_not_followed`, `an_audit_record_whose_key_and_body_disagree_is_ignored`,
  `a_peers_write_into_a_nodes_audit_stream_trips_the_tripwire`, `one_planted_record_does_not_leave_the_rest_of_the_stream_unverifiable`).
  **The H1/M1 review's N-A–N-D**: `verify_linked` public (`a_stream_with_a_skipped_slot_verifies_under_verify_linked`),
  the tripwire test controlled, the seal never refused by planted records, the free slot found outside the lock (`a_planted_run_of_any_length_leaves_the_node_sealing_and_is_reported`), kept
  records' removal documented, and the checkpoint-boundary residual fixed (case (c) of
  `recover_chain_head_anchors_at_a_checkpoint_or_reports_the_gap`).
- **Not built, stated:** no protection against a holder of the fleet CA key (every node in the development posture);
  the secure posture cannot rotate (recovery is re-issue only); a node with a pre-row-Q certificate has no standing
  until re-issued, and keys it revoked before the upgrade verify again at upgraded nodes until then; a retired key that
  never had a URI-SAN certificate stops verifying *live* signatures for a re-anchored node (history — the audit chain —
  verifies under the held-minus-revoked set, the final review's round); unsigned
  `Data` and anti-entropy; ~~the mTLS handshake does not refuse a revoked key~~ (delivered by the follow-up, §3.6 under
  Q); a
  node without persistence forgets a revocation hidden while it was down until the owner republishes; the fold walks
  every node with records on each change (bounded per node, not per-node incremental).

**S — Identity proofs on by default (#623, released in 2.34.0).** Read against the branch's code. The doors were
enumerated with `git grep -n "require_identity_proofs\|resolve_identity_record\|validate_and_merge_identity\|merge_peer_keys\|kv_ns::IDENTITY"`
over `src mycelium-core/src mycelium-*/src`: a node's keys are learned from gossip at two sites, `prewarm_peer_keys`
and `start_identity_watcher`, both through `identity_resync` → `resolve_identity_record` →
`validate_and_merge_identity_with_revocations`, each reading the one config value; besides them a key enters
`peer_keys` only at the handshake anchor (`record_peer_anchor`, CA-validated); `self_authenticating_identity` (the
unsigned exemption) and the own-history build read no flag; `mycelium-agentfacts` reads the raw value for order only,
filtered by trusted keys. The config value has three sources — `Default`, serde, the environment — each tested.
**#623's review found a writer the grep missed** (it searched `kv_ns::IDENTITY`, not `format!("sys/identity/{}")`):
`rotate_identity` built the new history from the raw `sys/identity/{self}` value and signed it with the trusted prior
key into the sealed record, laundering a planted key into C's authenticated history at every peer, with proofs on.
Now built from `revocation::own_history` — `rotation_does_not_launder_a_poisoned_bare_identity_into_the_history`
(seen failing on 1e0a648e: "LAUNDERED: after C rotated, A's poisoned key is in C's history set at B"; its legacy-
certificate twin is a guard, since a rotation re-anchors the node). Re-grep (`"sys/identity/`, `format!("sys/identity/{`,
`kv_ns::IDENTITY`): `lifecycle.rs` start path — writes only, history from `own_history_at_start`; the identity watcher
and prewarm — gated by the flag; `mod.rs` rotation — fixed; `mycelium-agentfacts` `crdt.rs:161` — order only, filtered
by trusted keys; `connection::self_authenticating_identity` (the unsigned exemption's `Stale` check, `connection.rs:308`) — reads the stored bare list only as keys
a new record must *contain*, so it grants nothing, but a member's planted bare list makes a genuine unsigned sealed
record count as `Stale` at gossip (refused; it still arrives by anti-entropy) — a liveness residual since 2.33.0,
read from the code and not reproduced, recorded, not changed here; the rest are tests, examples' wait conditions and the tripwire's prefix list.
- *The 2.14.0 attempt re-examined first* — `8b588c68` (#368) flipped it; `7cb3b872` (#372) reverted it for the
  Docker suite's `S12 leader election … Nodes disagree on leader`; `12a69598` (#373) corrected the diagnosis (the flag
  is inert without TLS and that suite runs none) and shipped the sealed record, the precondition the revert named; the
  S12 log resolved the failure the same day as an election with a quorum of one (S12 joined no group), closed by
  #377. Flipping the default and running every lib suite, the core suite, the co-op demos and `make check` found no
  failure the flag caused (the PR body lists each run).
- *Defaults to `true`* — `GossipConfig::default()`; `the_default_requires_identity_proofs` (seen failing on 4251d50a).
  **Two defects the flip exposed:** the field's field-level `#[serde(default)]` resolved a TOML file that never named
  it to `bool`'s `false` — `a_config_file_that_never_names_identity_proofs_requires_them`, seen failing on 4251d50a
  and again with only `Default` flipped; and `GOSSIP_REQUIRE_IDENTITY_PROOFS` read any value but four as `false`, so a
  typo disabled it — strict now, `the_identity_proofs_setting_parses_strictly_from_the_environment` (seen failing).
- *A bare key list without a proof is refused everywhere a node's keys are learned* — the two gossip sites share one
  function; `a_default_configuration_rejects_an_unsigned_identity_entry` (seen failing on 4251d50a).
- *Closing the residual rows F and Q state* — `by_default_a_poisoned_identity_key_does_not_reach_the_history_set`
  (seen failing on 4251d50a: "HISTORY: … is in C's history set at B" in a `tls` build, "FORGED: A's audit record signed
  by its poisoned key verifies as C's history at B" under `compliance`) and
  `by_default_a_poisoned_identity_key_does_not_sign_for_a_legacy_certificate_node` (C on an IP-only certificate; seen
  failing: "IMPERSONATION: by default, a key A wrote into C's identity signs as C at B").
- *A mixed-fleet window in the 2.30.0 shape, `false` an explicit opt-out* — `deprecations.md` §41.
  **The boundary (the PR's first statement, "before 2.14.0", corrected by #623's review, finding 2):** since 2.33.0
  (row F) only a tagged proof is accepted, in either posture (`helpers::identity_proof_message`), so a 2.3.0–2.31.x
  peer is not learned from gossip with or without the flag. Row S cuts off nobody 2.33.0 did not already cut off
  except a node from **before 2.3.0** (no proof at all), the one node `false` re-admits — as the cell said. A direct
  connection still anchors any peer's certificate key. The strict environment parse refuses an empty value, as
  `GOSSIP_GATEWAY_ALLOW_UNAUTHENTICATED` does; §41 names the refused spellings (finding 3).
  `an_explicit_false_still_opts_out_of_identity_proofs` (a guard; passes on 4251d50a too). A TLS-less node: inert,
  no warning, no refusal; the report names `[tls]` as missing.
- *`id.proofs_required` resolves `enforced` by default* — `id_proofs_required_resolves_enforced_by_default` (seen
  failing on 4251d50a); its `needs` text changed, catalogue regenerated.
- *Documentation* — CHANGELOG; `deprecations.md` §41 and its table row; `configuration.md`; guide 09, 23, 24;
  `cert-rotation.md`, `production-readiness.md`, `shared-responsibility-matrix.md`, `companions.md`, `audit.md`,
  `observability.md`; `docs/threat-model.md` Boundary A; `what-is-proven.md` (the residual moved from the third table
  to the CI table); the design records that cited the default; wiki `dev/security.md`, `dev/operations.md`,
  `dev/companions/commitment.md`; `docs/wiki/dev/.log/2026-10-13-s-proofs-default.md`.
- *Finding 4* — `examples/identity_one_record.rs`'s "opt-in under test" comment corrected.
- *The confirming check's low finding* — rotation signed the sealed record with the prior key and never re-signed it,
  so on a legacy-certificate node a second rotation's `own_history` ignored that record and dropped the original key.
  `rotate_identity` (and so `rotate_identity_on_compromise`, which calls it) now re-signs and republishes the record
  and proof with the new key after the cut-over, keeping the prior-key record first for the chain;
  `a_double_rotation_keeps_the_original_key_in_the_published_history` (seen failing on 0f9d417b: "k0 dropped: a
  legacy-certificate node's history lost its original key after two rotations (2 keys)"). The start path's loss
  after one rotation and a restart has the same cause and the same fix (`own_history_at_start` now finds a record
  signed by the current key) but no test: restarting a legacy-certificate node after a rotation means reconfiguring
  its pre-issued certificate, which the test helpers do not do.
- **Not built, stated:** first sighting of a node never seen stays trust on first use (anchors close it); a pre-2.3.0
  peer is not learned from gossip; a planted bare list can make a genuine unsigned sealed record `Stale` at gossip
  (`connection::self_authenticating_identity`, anti-entropy still delivers it); an un-upgraded 2.33 peer keeps proofs off until it is upgraded or configured, so
  the residual stays open *at it*; anti-entropy entries carry no author.

**T — Say loudly when the CA key is on the node (#622, released in 2.34.0).** Read against the merged code at the
2.34.0 cut by an independent reviewer. The doors enumerated first (`git grep -n 'log_report\|ca_key_on_node_warning\|CA_KEY_ON_NODE_WARNING\|id.ca_key_off_node'`):
one emission site, `guarantee::log_report`, called once from `start()` (`src/agent/lifecycle.rs:192`) after the
report is resolved and before the profile check (so a `secure-single-domain` node that will refuse still logs it
first); no other caller.
- *At `start()` a node that holds the CA key and runs no profile, or a profile other than `dev`, logs one prominent
  warning* — `guarantee::ca_key_on_node_warning` (`src/agent/guarantee.rs:542`) fires when `id.ca_key_off_node`
  resolves `not_configured` for a reason other than missing `[tls]`: no `cert_pem` (so `start()` re-signs with the CA
  key), `ca-key.pem` in `auto_cert_dir`, or no CA yet (`start()` is about to mint one here) (`guarantee.rs:829-835`).
  Silent only when the profile is explicitly selected as `dev` (an unset profile resolves to `dev` unselected and
  warns). `log_report` emits it once per `start()` at `WARN`, first in the report block (`guarantee.rs:551-553`).
- *… naming the consequence and the remedy* — `CA_KEY_ON_NODE_WARNING` (`guarantee.rs:534`): one compromised host
  can mint a certificate for every node and key revocation cannot recover a stolen identity; issue node certificates
  on a separate CA host with `mycelium tls issue` and give the node `[tls] cert_pem` + `key_pem`
  (`docs/operations/cert-rotation.md`).
- *The `id.ca_key_off_node` guarantee text names identity compromise and revocation, not only removal (catalogue
  regenerated)* — `guarantee.rs:821-823`; `docs/reference/guarantee-catalogue.md:20`, `.json:119-128`, gated by
  `the_checked_in_guarantee_catalogue_is_current`.
- *`dev` states the trade-off rather than warning* — the warning is suppressed under an explicit `dev`; the log then
  carries only the generic `profile dev — nothing required; this is not a production profile` (`guarantee.rs:562`,
  which predates T) and `id.ca_key_off_node`'s `not_configured` line. The trade-off is stated in the documentation
  (`docs/guide/09-security.md:96-100`). *A dev-specific log line stating the trade-off: not built.*
- *The quick-start and production-readiness pages say which posture they set up* — there is no page named
  quick-start; the TLS quick-start is guide 09's snippet, which names `TlsConfig::default()` the development posture
  and the off-node certificate the production one (`docs/guide/09-security.md:96-100`); production-readiness names
  the warning and when `dev` silences it (`docs/operations/production-readiness.md:68`). The deployment page's
  "TLS / identity" snippet, which sets up the CA-key-on-node posture, says so since the 2.34.0 cut
  (`docs/operations/deployment.md`).
- *A test pins the warning on and off* — `a_node_holding_the_ca_key_is_warned_unless_it_chose_dev`
  (`guarantee.rs:1100`, seen failing with the check stubbed out): no profile with a CA key → warned, the text naming
  "every node" and "mycelium tls issue"; `secure-single-domain` → warned; explicit `dev` → none; no `[tls]` → none.
  It tests the predicate, not the emission: that `log_report` logs at `WARN` is not asserted, and the quiet case of
  a pre-issued certificate with no CA key on the node is not asserted.
- **Not built, stated:** the CA key stays on every node in the development posture (the warning says so; it does
  not move the key); no dev-specific log line for the trade-off; the warning's emission is not asserted by a test.

### 3.6 · State of play and the next iteration (2026-10-12)

Where the work stands at the end of 2026-10-12, so the next session starts here rather than from a conversation.

**In flight:** nothing. Every PR in the batch is merged — E as #613, F as #611 (each after its independent reviews;
CI on F also caught a co-op demo regression its own changes caused, fixed in the PR by draining outbound
queues before the shutdown signal), and N, O, P recorded by #614 with the `a2a-sdk` probe's evidence
(`docs/plans/external/2026-10-12-a2a-sdk-probe/`).

**Delivered in v2.34.0 (2026-10-16):** F2 (#624 — the unsigned-frame and bare-signature refusal unconditional, `kv_require_signed_frames` retired, `secure-single-domain` rev 5; §3.5), T (#622), S (#623 — identity proofs required by default; §3.5); the wasmtime 49.0.3 / rustls 0.23.46 advisories (#625).

**Not started:** K (policy over history — decide §5's fleet question first), L (credentials attached at egress), M
(activation commands confined), N (A2A at the current specification — independent of E and F, and it carries a defect
worth fixing on its own: the agent card names the gossip port), O and P (after N), and §5's inference-routing question. (Q, revocation at every verifier, is delivered — #616, §3.5; S, identity proofs on by default, is delivered in 2.34.0 — #623, §3.5.)

**Follow-ups recorded as not built in delivered rows (each in its §3.5 block):**
- **H:** carry each named offer's signed bytes inside the close and award from that copy (today a member who deletes a
  named offer steers the checked path); a declarer that restarts without keeping its close can re-decide.
- **J:** the seam and KV gates end an unstarted skip at the first trailing `,`, so a test item with a multi-line header
  is partly scanned as production (fails safe) — balance `()`/`[]`/`<>` across the header.
- **I (private):** a Cedar schema type-check of `context.*` and template `when` literals; behaviour under real power
  loss behind the fsync pins.
- **E:** a nonce cache for the caller envelope (a replay inside the freshness window is
  accepted); an acknowledged in-process mailbox; and the SSE resume point (recorded since row G). **Fixed (2026-10-16):**
  the two `cap/` cases outside the row (#613's review, finding 6) — a `/` or an empty `ns`/`name`, which built an advert
  no reader parses, and `locality`/`self`, which overwrote the node's own locality entry — are refused at every door
  that writes a `cap/`, `req/` or `gcap/` key: `capability/advertise` (400 `invalid_capability_name`), the unit-file
  loader (`units/declare` and node config; a group name holding a `/` too), and in process `advertise_capability`,
  `declare_requirement` and `define_capability_group`, which write nothing and warn; `[[serve]]` and
  `register_prompt_skill` refuse the locality pair; and the emergent-group projector skips a gossiped `cap-group/`
  definition whose names fail the check (`capability::capability_name_problem`;
  `a_capability_name_that_is_not_a_key_segment_is_refused_at_every_door`,
  `the_group_projector_skips_a_definition_whose_names_are_not_key_segments`; `deprecations.md` §43). Not checked at
  load: a catalogue manifest's `provides` (it installs; its advert is refused in process).
- **F:** the shutdown drain is best effort (it reads queue emptiness, so a frame between a shard queue and a writer
  queue can be missed). **Fixed (2026-10-16):** the pre-existing gap #611's review found — keys the shutdown sweep did
  not cover were tombstoned only when the signal woke their tasks, after the drain, so a live handle's entry was never
  retracted at its peers — now covers `svc/{kind}/{self}`, `tools/{name}/{self}` and, found by enumerating every
  soft-state task, `gcap/{group}/{ns}/{name}/{self}` (`shutdown_with_timeout`;
  `soft_state_with_a_live_handle_at_shutdown_is_retracted_at_the_peers`, seen failing on all three). Not swept, as
  before: `cap-group/{name}` (ownership not readable from the key), `grp/` membership entries and `sys/health/{self}`.
  **The re-assert residual (#627's review, finding 1) is closed (2026-10-16):** shutdown closes
  `CoreCtx::soft_state_gate` before the sweep, so no task re-writes a swept key between the sweep and the signal
  (`a_fast_reassert_does_not_undo_the_shutdown_sweep`, every round failing before on `svc/` and `cap/`;
  `deprecations.md` §42). Only `run_kv_persist_task`'s path and `write_opacity_key` are shown by a test; the
  emergent-group and load-opacity writes are gated by the same call and not separately tested. Also
  stated in F's §3.5 block: first sighting is trust on first use; with identity proofs off another member's identity
  is writable under the writer's own signature; anti-entropy entries carry no author; role claims, audit records and
  revocation events are signed untagged.
- **B:** connection preemption and WAL group commit, removed in review.
- **Q:** ~~the mTLS handshake does not refuse a revoked key's certificate~~ — *delivered after 2.34.0, unreleased:*
  both sides of the handshake refuse a certificate whose key is anchored for its owner and revoked by it — the node
  its URI SAN names, or for a URI-less certificate the node holding the key anchored (`refuse_if_revoked`,
  `mycelium-core/src/tls.rs`; `a_revoked_keys_certificate_is_refused_at_a_peers_handshake`,
  `a_revoked_keys_certificate_is_refused_at_the_handshake_both_ways`,
  `a_revoked_key_is_refused_behind_a_certificate_that_names_no_node`), and the mesh resumes no TLS session — found on
  the way: a resumed session re-ran no verifier, so a removed member could reconnect on one (C5, since 2.15.0;
  `a_removed_member_cannot_reconnect_on_a_resumed_session`). *Not built:* a connection already open when the revocation
  is counted is not closed; a key that never had a certificate record naming its node is not refused at the
  handshake; a provisional revocation refuses no handshake (deliberately: a rotated node must connect). Still open: revocation authority rests on the CA key's custody (the development posture gives every node the CA key); the fold is
  not per-node incremental.

**Sources compared on 2026-10-12** (rows K–P and the open question came from these):
[Strands Box and Dogwood](https://aws.amazon.com/blogs/opensource/introducing-strands-box-ai-agent-sandboxes-powered-by-dogwood/)
(policy over an agent's history; credentials attached at egress; OS containment) ·
[NVIDIA OpenShell](https://docs.nvidia.com/openshell/latest/about/overview.html) (credential placeholders; fixed versus
live policy; the privacy router) ·
[Strands Agents 1.0](https://aws.amazon.com/blogs/opensource/introducing-strands-agents-1-0-production-ready-multi-agent-orchestration-made-simple/)
(`SessionManager`, swarms and graphs, A2A — the framework rows N–P put on Mycelium).

**Release:** v2.33.0 is cut on 2026-10-13 (branch `release/v2.33.0`; tagged only on the maintainer's word) with the rows merged
since v2.32.0 — P2's migration #606 (with #604, #605), Φ, P5, J, H, E, F and Q, and the private row I on its pin;
upgrade notes `deprecations.md` §34–§39. **v2.34.0 is cut on 2026-10-16** (branch `release/v2.34.0`; tagged only on the
maintainer's word) with F2, S and T, the follow-ups #627–#629 and #625; upgrade notes §40–§43. Recorded by F2 as not
built: an authenticated anti-entropy entry (a wire change).

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
- **Inference routing as policy (from OpenShell's privacy router, 2026-10-12).** `mycelium-reason` routes a model
  call by capacity and reservation; nothing decides whether a request *may* leave the domain, or to which model, by the
  data it carries. The façade already runs the action preflight, so it is the natural enforcement point. Decide whether
  this becomes a row (the evaluator sees the route's destination and a data classification) or stays out of scope.
- **Row K: what does "history" mean in a fleet?** The evidence journal is node-local, so a history-conditioned rule
  counts what *this* enforcement point saw. A fleet-wide count needs either the consensus path (an agreed counter —
  exact, slower, unavailable without a quorum) or a gossiped tally that detects an overrun after the fact (detection,
  not prevention, the Layer I law). Decide before K is built, and say which on the rule.

## 6 · Recorded limits that stay limits

Single-decree safety rather than a proof of the whole protocol; DNS rebinding outside the egress claim; a member that
*signs* consensus messages at the ballot ceiling; a forgery that reaches a node only through anti-entropy before it
holds its own record.
