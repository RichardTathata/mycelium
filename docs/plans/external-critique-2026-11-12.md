# The third-party critique of 2026-11-12 — verification and the plan that answers it

**Status:** **agreed with the maintainer on 2026-11-12**, rev 0.4 (rev 0.4, 2026-11-13: CR13's second step gains a second candidate shape, local non-admission with forwarding unchanged, to be weighed with the counter's data; D16 unchanged; rev 0.3, 2026-11-12, records the maintainer's
answers to all sixteen decisions of §5, each as recommended, and marks every row of §4 *buildable*, *after wave
1b* or *decision recorded, no row*; rev 0.2 took the independent review of this plan's PR, #773: the use-marker
row rewritten — a KV record alone cannot carry it; the lease margin and the fenced write restated; the drift
refusal given its receiver-clock, replay and ordering cases and moved to a decision; the WAL checksum's downgrade
case; catalogue rollback's limits; four inventory rows that had no plan row; §6 split into what is wrong and what
is understated; every choice the review found inside a row moved to §5). **The order is agreed (D11):** CR1 now
(delivered: PR #774 and the log read doors' follow-up, PR #777); **inside wave 1b**, CR2's first PR (the use marker), CR13's counter and CR12; CR3–CR11 and
everything else **after wave 1b**, with CR10 (the model) startable in parallel. A row is *buildable* only where
its text needs no further decision; each row's **State** line says where one remains.

The critique is vendored at
[`external/2026-11-12-360-critique.md`](external/2026-11-12-360-critique.md), its text unchanged: the reviewer's
summary, then the reviewer's reply to the project's first response (one marked block in its header is the
project's, not the reviewer's). **The reviewer's full per-layer document was offered and has not
been received**, so every finding here is the summary's one line, with whatever `file:line` the summary gave. Where
the summary gives no location, the verification found its own, and says how. The project's first response, which
the reply answers item by item, was not kept as a document; its six-item list is given in the vendored file's
header, marked as the project's.

The reviewer states the method: six independent code readers, each layer's lead finding confirmed against the code,
nothing built or run. This verification is the same kind: every finding was read against `main` at `156ca329`
(after #771 and #772) under the [verification policy](../wiki/dev/testing/verification-policy.md), rule 1 — the
entry points enumerated, the grep given, each cited line re-read by the author of this plan after three read-only
agents found it. **Nothing was built or run.** A verdict of *confirmed* means *the code reads that way*; no
behaviour was observed, and §7 lists what that leaves open.

## 1 · What the critique found, and what the verification found

The critique makes one structural claim, a table of per-layer findings, six points on the philosophy, a set of
counts, and an ordered list of code areas. In short:

- **The root cause is right, and it is already written down.** "Any member can write any key, and the upper layers
  keep security state in that KV" is the owner-signed rule adopted today
  ([`owner-signed-records.md`](../wiki/dev/owner-signed-records.md), #772), whose inventory is the verification of
  that section: 37 rows whose status includes *Does not hold*, 15 at *Partly*, 2 at *Meets*
  (`grep -c` over the page's tables). This plan refers to those rows and does not re-derive
  them.
- **Every per-layer finding that was checked reads true in the code.** None is false. Several are narrower than
  stated, several are limits the repository already states (the reviewer says so; the table says which), and
  three — the far-future stamp, the lease's skew bound, reply forgery — are stated only in a code comment, a
  design note or an upgrade note, where an adopter does not look.
- **Three findings are not confirmed:** self-install is not on by default in code (a stem installs only with a
  `[hosts]` table and a trusted publisher) — though the reviewer is right that it is the headline (P5); `TaskCtx`
  was not shown to be the cause of the `too_many_arguments` allows (16 of 57 are outside the root crate; N13); and
  the upgrade notes do state the two conditional protections as conditional (P3).
- **The counts are right.** Of the seventeen rows of §2.4, two differ in what was counted (clippy invocations,
  review-history comments), one states a cause that was not shown (N13), and the rest match within a few per cent.
- **The critique's "Sound" column was not verified** (§7).
- **The reviewer's reply is right about the two conditional protections** (`gateway_caller.rs:1348`, `:1405`–`1410`),
  and the v2.40.0 upgrade notes already say so (§2.3). No release note is corrected by this PR.

What the verification found that the critique did not:

- **The far-future stamp is not an anti-entropy property.** A live `Data` or `SignedData` frame takes the same
  path (`connection.rs:1060`, `:1254`): the clock is clamped, the entry is stored at its own stamp. A fix at the
  anti-entropy arm alone would leave the live door open. And one such write to `cap/{victim}/…` de-advertises
  the victim for good: the capability freshness check quarantines the entry (`src/capability.rs:565`–`566`) and
  no refresh by the owner can out-stamp it.
- **A presence-bearing record cannot be made safe in the KV alone.** While any member can out-stamp the owner, a
  tombstone or a garbage value planted *before* the genuine record wins LWW, and the door's own write reports
  success (`src/agent/state_machine.rs:981`–`983` discards a receipt whose application rung is `Superseded`).
  The #773 review found this against the first draft of CR2; the row now rests on a node-local durable record.
- **A forged SWIM `Dead` composes with the live-peer quorum.** It removes a peer from `peers`
  (`mycelium-core/src/swim.rs:208`–`210`), and the default quorum is `peers.len() + 1`
  (`src/agent/consensus_handle.rs:336`): on a default `[tls]` fleet a non-member with UDP reach may be able to
  shrink two views until each holds a local majority, with no partition. Inferred, not run; a live TCP peer may
  be re-added, so it may need repeating (§7).
- **A WAL checksum is an on-disk format change**, not the "no wire change" item the reply calls cheap. The WAL has
  no magic and no version byte to discriminate on, and the golden fixture rule is *a format change adds a
  directory; it never edits one* (`tests/fixtures/persistence/README.md:14`).
- **The catalogue ranks candidates by two unsigned fields** (`provisioner.rs:1147`), so a replayed line can also
  be made to win selection; and a `Provisioner` built in code with no `require_provenance` call accepts unsigned
  lines (`provisioner.rs:537`–`539`) — the refusal lives in the stem, not in the type.
- **The log read doors** have the same hole as the KV read doors: `OWNED_LOG_STREAMS` is consulted on append and
  compact only (`overlay.rs:549`, `:617`), not on scan or either subscribe (`overlay.rs:587`, `:646`, `:714`).
- **A signal's nonce is burned before admission** as a KV frame's is, in one set keyed by the nonce alone
  (`seen.rs:35`–`36`). The threat model states this for KV frames and says anti-entropy repairs it; nothing
  repairs a suppressed signal.

## 2 · Finding by finding

Verdicts: **confirmed** · **confirmed, narrower** (what holds is stated) · **already stated** (a limit the
repository states, cited) · **not confirmed** (what the code does) · **not verified** (why). *Row* names the §4
row that answers it, or the decision in §3 or §5.

### 2.1 · The root cause

| | Critique | Verdict | Evidence | Row |
|---|---|---|---|---|
| A1 | The grant use marker is a gossip key; a member can tombstone it to re-arm a grant; two gateways can admit the same grant concurrently | **confirmed** by #772 | [inventory](../wiki/dev/owner-signed-records.md), *Grants* table, `grant/…/used/{door}`: *Does not hold — open*; also `consumed`, `revoked` and the request record. `what-is-proven.md` § *Not yet shown*, first row | CR2 |
| A2 | The audit chain has no signed head or external anchor; tail truncation is undetectable | **confirmed** by #772 | inventory, *Audit* table, *The audit chain's head*: a truncated or deleted chain verifies `Ok` (`src/agent/audit.rs:917`). Each record is signed and verified (*Meets*, per record) | CR3 |
| A3 | `cap/`, `req/`, `prov-shed/` are unauthenticated; one member can trigger installs, suppress healing or attract calls | **confirmed, narrower** by #772 | inventory, *Capabilities* table. Forged demand installs only a catalogued, provenance-checked artifact within budget and self-election (`provisioner.rs:1065`); `filter_protected` (H3) exists with no caller | CR4 |
| A4 | A caller envelope is trusted if signed by any key of the sending node, so any member can assert any principal or scope | **confirmed, narrower** — the reviewer's own reply narrows it: true for a principal with no key (`token:`, `oidc:` without exchange), not for a certificate holder's signed call (I1), a signed grant decision (I3) or an exchanged token (I5) | inventory, *Identity* table, the key-less principal row and the bare-`sender` row; `what-is-proven.md` § *Not yet shown* | CR5 |
| A5 | The fix needs no Layer I law: owner-signed records verified at read | **agreed and adopted** — the rule, #772 | `owner-signed-records.md` § *It does not contradict "detection, not prevention"* | — |

### 2.2 · Layer by layer

| | Critique | Verdict | Evidence, and how it was found | Row |
|---|---|---|---|---|
| **Layer I** | | | | |
| L1 | Anti-entropy applies unsigned entries (`connection.rs:843-868`) | **already stated** | `what-is-proven.md` [*Anti-entropy entries authenticated*](../operations/what-is-proven.md#anti-entropy-entries-authenticated) ( "carries no author and no signature and is applied by LWW"); `threat-model.md:85`–`88`; inventory § *Layer I* | §5 D8 |
| L2 | …with unbounded timestamps, so a `u64::MAX` stamp pins a key forever | **confirmed, wider** — and stated only in a module comment. `hlc.observe(entry.timestamp)` (`mycelium-core/src/connection.rs:852`) clamps the **receiver's clock** to wall + `max_clock_drift_ms` (default 5 min; `hlc.rs:283`–`285`, `config.rs:2147`), then the entry is applied with `timestamp: entry.timestamp` unchanged (`connection.rs:862`). LWW is `incoming_ts > curr.timestamp` with no bound (`store.rs:591`–`594`). A local write stamps about wall-now, so it never beats `u64::MAX`; only a tombstone or a greater value at exactly that stamp does (`store.rs:595`–`598`). The tombstone sweep collects only stamps below its cutoff (`store.rs:967`), so a poisoned tombstone is never collected. **Wider:** live `Data` and `SignedData` frames take the same path (`connection.rs:1060`, `:1254`), and WAL replay applies a stored stamp as it is (`src/agent/lifecycle.rs:502`–`511`). **Worse above Layer I:** a far-future write to `cap/{victim}/…` fails `CapEntry::is_fresh` (`src/capability.rs:565`–`566`: quarantined when more than three refresh intervals ahead) and beats every refresh by the owner, so one write de-advertises a victim permanently. The clamp is a rate-limited `warn!` with no counter. `hlc.rs:76`–`83` says so: "the poisoned entry would win LWW against local rewrites until the wall clock catches up", and "Full inbound rejection of out-of-bound updates (store-level quarantine) is a candidate for the next wire-policy pass." Not in `threat-model.md` or `what-is-proven.md` (`grep -n -i "far-future\|drift\|u64::MAX"`) | found: `grep -n "hlc.observe" mycelium-core/src/connection.rs`; `grep -n "observe\|drift" mycelium-core/src/hlc.rs`; `sed -n 591,598p mycelium-core/src/store.rs` | CR13 |
| L3 | Deleted keys can resurrect after about 50 minutes | **confirmed**; the number is exact from defaults, and a floor. Tombstones older than `default_ttl × propagation_window_secs × 10` seconds are removed outright (`src/agent/tasks.rs:1029`–`1036`; defaults 5 and 60, `config.rs:2095`, `:2098` → 3 000 s; a `default_ttl` of 0 auto-sizes upward with the fleet, `config.rs:2267`–`2268`), by a sweep on a tick of at least 60 s (`tasks.rs:1014`). After that a returning node's live entry meets no tombstone and is inserted; the `StateResponse` arm has no horizon check (`connection.rs:843`–`871`). Needs a node that held the value and missed the tombstone for longer than that. Stated generically in `docs/design/data-lifecycle-and-erasure.md:22`–`23` ("anti-entropy can resurrect a value…") and `docs/operations/data-erasure.md:6`, with no number; not in `threat-model.md` or `what-is-proven.md` (`grep -n -i resurrect`). The knob is `propagation_window_secs` | found: `grep -rn sweep_stale_tombstones src mycelium-core/src` | CR17 |
| L4 | No checksum on WAL records | **confirmed.** A record is `[u32 LE length][payload]` (`mycelium-core/src/persistence.rs:1222`–`1234`); replay sees a zero or oversized length, a file ending inside a record, and a payload that fails to decrypt or decode (`persistence.rs:597`–`624`). A flipped bit that still decodes is replayed. `snapshot.bin` has none either. **Narrower in effect:** torn and corrupt are told apart and a corrupt record refuses the start or quarantines; a configured at-rest cipher makes a decrypt failure corrupt. The three other logs say so themselves: `src/agent/journal.rs:61` ("there is no checksum on disk"), `mycelium-tuple-space/src/store.rs:1669`, `mycelium-blackboard/src/wal.rs:560` ("for want of a checksum") | found: `git grep -n -i "crc\|checksum" -- mycelium-core/src/persistence.rs src/agent/journal.rs mycelium-tuple-space/src mycelium-blackboard/src` | CR14 |
| L5 | SWIM is unauthenticated UDP even under TLS | **confirmed; already stated** for the federation profile only. Any datagram that decodes is acted on (`mycelium-core/src/swim.rs:356`–`368`); a `Dead` update removes the peer from `peers` and evicts its writer (`swim.rs:208`–`210`); on by default (`config.rs:2120`). **It composes with L11:** the default quorum is `peers.len() + 1`, so forged `Dead` updates shrink the view a lock's quorum is sized from (inferred, not run). The code says it: `config.rs:307`–`308` ("unauthenticated UDP … signs nothing"), `swim_membership.rs:103`–`105`. Stated at `threat-model.md:252` (off in the enforced profile) and `shared-responsibility-matrix.md:44` ("plaintext, unauthenticated"). **Not stated:** that on an ordinary `[tls]` fleet a host that reaches the UDP port, member or not, can evict a peer and shrink a quorum. An injected address still has to pass the mTLS handshake | found: `grep -n "sign\|verify\|mac" mycelium-core/src/swim.rs` (no authentication) | §5 D7 |
| **Layer II** | | | | |
| L6 | Signal frames are unsigned (`framing.rs:193-204`) | **confirmed** at the frame: `WireMessage::Signal` has no signature field (`mycelium-core/src/framing.rs:193`–`204`), and the receive arm checks only that the sender is not removed (`connection.rs:881`) — the bare-`sender` row of the #772 inventory. **Narrower:** payloads that decide carry their own signatures — consensus messages, the gateway caller envelope, grants and possession proofs | inventory, *The bare RPC's `sender`* | §5 D6 |
| L7 | …so RPC replies are forgeable | **confirmed, narrower.** A reply is matched on an 8-byte nonce and `expected == sig.sender` (`src/agent/mod.rs:1520`–`1521`); the sender is emitter-written. A member that **saw the request** can answer as the target, first reply wins. One that did not would need the nonce, which is `fastrand::u64` (`src/agent/rpc.rs:451`) — not designed as a secret, the same generator whose outputs travel as frame nonces (`mycelium-core/src/framing.rs:479`); unassessed, and no weight is put on it. With `group_aware_forwarding` (default on) an Individual frame goes direct when a route exists (`src/agent/tasks.rs:368`–`370`), so a relay sees a request only on the flood fallback — **which is the common path in a fleet of any size**: forwarding targets are capped at `2·⌈log2 N⌉`, floor 8 (`mycelium-core/src/config.rs:967`–`988`), so above about nine nodes most unpinned pairs have no direct route. Stated in code (`src/agent/mod.rs:596`–`599`) and in `docs/guide/deprecations.md:539`–`540`; not in the threat model. No reply authentication exists | found: `grep -n "rpc_reply_sender\|pending.get" src/agent/mod.rs` | §5 D6 |
| L8 | …and mandates travel in clear past hop TLS, readable by every relay | **confirmed** as said ("in clear"): TLS is per hop and a relaying member reads what it relays — and by L7's fan-out cap, relays are the common path. Context, not a rebuttal: a relayed mandate is readable and not usable, since a grant is signed by its authority and the possession proof by the holder over this call's operation, resource and arguments (`src/agent/gateway_caller.rs:226`–`231`). The philosophy now says a boundary is not a confidentiality mechanism (`docs/philosophy.md:13`, #771); the threat model has no line on relay confidentiality | — | §5 D6 |
| L9 | Quorum-evidence writes amplify each signal by N | **confirmed, narrower; already stated.** Each node that admits a signal of a kind it works on or recently asked about writes `sys/quorum/{kind}/{sender}` and gossips it (`connection.rs:944`–`955`); gated by `has_worker`/`is_pinned` (`mycelium-core/src/signal.rs:1191`), at most once a second per key per node (`signal.rs:870`–`876`), bounded per kind and per second. An Individual signal is one write; a Cluster signal with a worker on every node is up to N. `threat-model.md:205`–`206` states the bound; `deprecations.md:706`–`711` says it "bounds a rate, not a total" and the keys are not collected | — | CR17 (stated, not built) |
| L10 | Nonce is marked seen before validation | **already stated** for KV frames (`threat-model.md:89`–`92`: "A refused frame still consumes its nonce … recovered by the next anti-entropy exchange"). **Confirmed, and not stated, for signals:** the nonce is marked after the removed-sender check and before admission (`connection.rs:884`–`886`, `:898`), in one set shared with KV frames and keyed by the nonce alone (`mycelium-core/src/seen.rs:35`–`36`). A suppressed signal has no repair | found: `grep -n mark_and_check mycelium-core/src/*.rs` | CR17 |
| **Layer III** | | | | |
| L11 | Default quorum is sized from the local live-peer view, so a 3\|2 partition can commit two lock holders (`consensus_handle.rs:336`) | **confirmed for the default; already stated.** `n_nodes = peers.len() + 1` (`src/agent/consensus_handle.rs:336`), quorum `n/2 + 1` (`src/agent/helpers.rs:113`–`115`); SWIM removes a dead peer from `peers` (`swim.rs:209`). `consensus_require_electorate` defaults to `false` (`config.rs:2179`); off, a safety-sensitive proposal outside an electorate is **counted and let through** (`src/consensus.rs:3323`–`3330`, `mycelium_consensus_ungoverned_safety_total`); on, it is refused `ElectorateNotGoverned`. `distributed_lock` and `consistent_set` fall to `cluster_propose` when no exclusive-default electorate is declared (`consensus_handle.rs:507`). `secure-single-domain` rev 4 requires the flag. Stated: `threat-model.md` §7 ("two non-empty, disjoint views can each satisfy a locally computed majority … **Not claimed**"); `what-is-proven.md` scopes its claim to "With `consensus_require_electorate`". The reviewer's point stands as written: the profile is answered, the default is not. The 3\|2 outcome is a reading, not a run | found: `git grep -n consensus_require_electorate -- 'src/*.rs' 'mycelium-core/src/*.rs'` | CR9, §5 D1 |
| L12 | No model or linearizability checker exists | **confirmed.** `mycelium-sim` is a record-and-replay harness (its one test checks a replay diverges on a changed WAL byte); `loom-spike` has four models of small concurrency idioms (`once_guard`, `unique_id`, `publish`, `bounded_append`), none of consensus; `proptest` blocks exist in the core and none in `src/consensus.rs`, `consensus_life.rs` or `electorate.rs`. No stateright, shuttle, turmoil, madsim or porcupine in any manifest or source; no `.tla`. The code claims no more: "single-decree agreement on a stable roster, not linearizability" (`consensus_handle.rs:447`) | found: `git grep -il "stateright\|shuttle::\|turmoil\|madsim\|porcupine\|knossos" -- '*.rs' '*.toml'` (empty); `git ls-files '*.tla'` (empty) | CR10 |
| L13 | Leases have no skew margin | **confirmed; stated in the design note only.** A reader ends a lease on its own wall clock, `wall_now_ms > exp` (`src/consensus_life.rs:275`), against the proposer's wall-clock expiry (`src/consensus.rs:1394`); the holder's deadline is `started + ttl` with nothing subtracted (`consensus_handle.rs:822`). `docs/design/lock-lifecycle.md:316`–`320` states it: "another holder can then be granted while the first is still inside its own deadline … nothing in the substrate knows δ". Not in `threat-model.md` or `what-is-proven.md` (`grep -n -i skew`) | — | CR11 |
| L14 | Nothing checks the fencing token | **confirmed, by design; already stated.** A token is issued and returned (`src/agent/overlay_consistent.rs:193`–`196`); no library write path checks one — `consistent_set` writes by plain `kv_set` after the commit, and the only resource that refuses a stale token is `examples/distributed_lock.rs`. `threat-model.md:596`: "the effect — fenced **at the resource** with the commit's token"; `docs/guide/04-consensus.md` § fencing says the same. The contract is the application's | found: `git grep -n -i fenc -- 'src/*.rs' 'mycelium-*/src/*.rs'` | CR11 |
| **Layer IV** | | | | |
| L15 | Catalogue signatures carry no version or expiry, so any member can roll the fleet back (`catalog.rs:154-168`) | **confirmed, narrower.** The signed message is a domain tag, the entry-format version, kind, artifact hash, the two requirements and the capability bytes (`mycelium-wasm-host/src/catalog.rs:154`–`168`): no sequence, issue time, expiry or revocation. Withdrawal is a plain tombstone, so a withdrawn line any member re-writes verifies (`provisioner.rs:537`–`539`). **Narrower only in that no single write swaps a running install** — none was found on the catalogue path. It is reachable by composition: a member forces the next install by shedding a band with a forged `prov-shed` (A3), or waits for a probe failure, which withdraws and "this round reinstalls" (`provisioner.rs:947`–`967`); the replayed line is then chosen. **Wider:** candidates are ranked by `(size_bytes, est_install_secs)` (`provisioner.rs:1147`), both outside the signature, so the replayer can make its line win. Already in the #772 inventory (`installable/…`: *Partly*) | found: `grep -n "provenance_ok\|sort_by_key" mycelium-wasm-host/src/provisioner.rs` | CR7 |
| L16 | Native activation is unconfined unless declared | **confirmed; already stated**, and it is the documented default: `mycelium-wasm-host/src/activation.rs:9`–`12`, `docs/reference/unit-file.md:363`–`365` ("**By default it is unconfined**"). The command comes from the stem's own unit file, not from a gossiped line, so a member or a publisher cannot supply one. Declared confinement fails closed. Not in the threat model | — | CR8 |
| L17 | Stems stage blobs before checking provenance | **confirmed.** The ticker stages every catalogue entry of a declared kind by its size hint (`mycelium-wasm-host/src/stem.rs:505`–`517`); `provenance_ok` runs later, inside the provision round. The hint is unsigned; the ceiling is the hint or `max_stage_bytes` (`http_source.rs:437`); the staged bytes must hash to the id. Already in the #772 inventory, same row | — | CR7 |
| **Layer V** | | | | |
| L18 | Protected kinds are checked by hand at 9 sites, preflight at about 10 | **confirmed, narrower.** Nine gateway handler sites (`mesh.rs:116`, `:171`, `:503`, `:656`, `:750`; `signals.rs:22`, `:72`, `:142`; `overlay.rs:886`), plus three outside the gateway (`provider_enforcement.rs:238`, `exchanged_token.rs:51`, `mycelium-wasm-host/src/confine.rs:100`). Eight handler calls of `ae_preflight` (`mcp_bridge.rs:163`; `a2a.rs:873`, `:1263`; `federation_routes.rs:113`; `grants.rs:350`, `:826`; `llm_bridge.rs:170`, `:311`), the public wrapper (`authz.rs:808`), the provider (`provider_enforcement.rs:280`) and the exposure door (`door.rs:208`). **Narrower:** each site calls one shared predicate (`is_protected_kind`, `src/agent/mod.rs:258`) and one decision function; the *call* is what is hand-wired. There is no single door function and no typed extractor. What bounds it today: the scope layer is central and deny-by-default; the C7 bypass matrix (`src/lib_tests.rs`, `test_c7_the_bypass_matrix_no_door_runs_revoked_work`) and `scripts/check-causal-doors.py` enumerate doors from hand-kept lists, not from the router | found: `git grep -n "refuse_protected_kind(\|refuse_unobservable_kind(\|refuse_unservable_kind(" -- 'src/agent/http/*.rs' ':!*_tests.rs'`; `git grep -n "ae_preflight(" -- 'src/*.rs' ':!*_tests.rs'` | CR6 |
| L19 | Raw KV reads have no namespace guard (`kv.rs:13-25`), so `kv:read` reads grants and audit | **confirmed** on `main`: `gw_kv_get` reads the store with no guard (`src/agent/http/kv.rs:13`–`17`); `refuse_protected_key` is called on writes only. `docs/operations/rbac.md:71` says "application keys only" of both verbs, which overclaims for `GET`. **Wider:** the log scan and subscribe doors (`overlay.rs:587`, `:646`, `:714`) call no guard | — | CR1 (delivered) |
| L20 | Default TLS keeps the CA key on every node | **already stated.** True of the default `[tls]` mode (`cert_pem: None` re-signs with the CA key, `mycelium-core/src/config.rs:187`–`190`, `tls.rs:534`–`535`), warned at start unless the profile is `dev` (`src/agent/guarantee.rs:598`), stated at `threat-model.md:486`–`487`, `:547` and `what-is-proven.md` [*Recovery from a compromised fleet CA key*](../operations/what-is-proven.md#recovery-from-compromised-fleet-ca-key); the off-node mode is proven (`what-is-proven.md` — *A TLS node starts without the fleet CA's private key*) and required by `secure-single-domain` | — | none |

### 2.3 · The two conditional protections, and the defaults

| | Critique | Verdict | Evidence | Row |
|---|---|---|---|---|
| P1 | The exchanged token is required for an `oidc:` principal only where the provider sets `exchange_audience`; otherwise the check returns `Ok(None)` (`gateway_caller.rs:1348-1353`) | **confirmed.** `src/agent/gateway_caller.rs:1348`: `…filter(|v| v.config().exchange_audience.is_some()) else { return Ok(None) }`. Narrower still: where set, it is required for protected kinds (`exchanged_token.rs:51`); a build without `compliance` verifies nothing (`:1340`–`1345`) | **The upgrade notes say so.** `CHANGELOG.md` 2.40.0, §96: "a provider *can* verify … and one that does refuses"; "verifies iff `exchange_audience` is set". `deprecations.md` §96: "**The trigger is `exchange_audience`.**" `what-is-proven.md` (*A caller envelope's principal is the client's*) names the unset case as open | none |
| P2 | An unsigned call for a certificate holder is counted, not refused (`gateway_caller.rs:1405-1409`) | **confirmed.** `gateway_caller.rs:1405`–`1410`: `ctx.unsigned_actions.note_at_provider(req.kind())`, and the call is served | **The upgrade notes say so.** `CHANGELOG.md` 2.40.0, §87: "counted, not yet refused"; `deprecations.md` §87 title and body ("A later release will refuse an unsigned call"); `identity-and-lifecycle.md` §9 O3 | O3, in its plan |
| P3 | "The upgrade notes should say so until the refusals land" | **not confirmed as a defect in the notes** — they do. The absolute wording is in two one-line summaries: the CHANGELOG's lead sentence for 2.40.0 and `CLAUDE.md`'s release paragraph ("a client's own `cs`/`cn` signature checked at the door and the provider"; "verified at the provider"). Both are summaries above notes that state the condition. This PR changes neither (its brief limits `CLAUDE.md` to one line); the maintainer may want *where configured* in the paragraph at the next release | — | §5 D10 |
| P4 | "Token set" reads as "secured" when no evaluator, provider enforcement or journal is attached. Warn at start | **confirmed, narrower.** No warning names that case. What exists: a `warn!` when `/a2a` is mounted with no evaluator (`src/agent/lifecycle.rs:296`–`299`); a `warn!` for the `dev` profile, which is the default (`guarantee.rs:631`); a `warn!` when an exposure policy is set with no evaluator (`src/agent/mod.rs:1807`–`1811`); the startup guarantee report listing the missing evaluator as `NotConfigured`, at info (`guarantee.rs:1025`); a non-loopback gateway with no credential refused | — | CR15 |
| P5 | Make self-install opt-in, not the headline default | **not confirmed in code; right about the headline.** It is opt-in. The library has no provisioning field and "never auto-provisions" (`mycelium-wasm-host/src/provisioner.rs:5`–`6`); the `mycelium` binary has no provisioner; a stem runs one only with a `[hosts]` table (`stem.rs:352`–`353`) and refuses an empty `trusted_publishers` unless `accept_unsigned` is set, which warns (`stem.rs:442`–`461`). `docs/positioning.md` says "It is not true of every node". What is fair, and conceded: the hero sentence leads with it and the README's five-step path has the stem fleet as step 3 (`README.md:38`). One real gap in the other direction: a `Provisioner` built in code accepts unsigned lines until `require_provenance` is called (`provisioner.rs:537`–`539`) | — | CR7 |

### 2.4 · Language and structure: the counts

Each counted at `156ca329`, with the command.

| | Reviewer | Counted | Command | Verdict |
|---|---|---|---|---|
| N1 | 2,253 `cfg(feature)` lines | **2,346** in all tracked `.rs`; 2,137 in `src/` and `mycelium-core/src/` | `git grep -n 'cfg(.*feature *=' -- '*.rs' \| wc -l` | confirmed (the reviewer's scope is unknown; both figures bracket it) |
| N2 | 11 clippy invocations | **15** in `make check`; **37** in `ci.yml` | `sed -n 163,190p Makefile \| grep -c "cargo clippy"`; `grep -c "cargo clippy" .github/workflows/ci.yml` | confirmed, understated. 11 is the root crate's and the core's lines in `make check` |
| N3 | two extra crates that exist only to test feature-off builds | **2**: `mycelium-gateway-free-tests`, `mycelium-tls-free-tests` | `ls -d mycelium-*-free-tests` | confirmed; their manifests say why (Cargo unifies dev-dependency features) |
| N4 | `GossipConfig` has 84 public fields and is not `#[non_exhaustive]` | **84**; not `#[non_exhaustive]`; `#[serde(default)]` and a `Default` impl exist | `awk` over `mycelium-core/src/config.rs:1016`–`1847` counting `^    pub name:`; `git grep -n non_exhaustive -- mycelium-core/src/config.rs` (empty) | confirmed |
| N5 | 98 deprecation sections | **95** numbered `##` sections, numbered to 98 (no `## 7.`–`## 9.` heading) | `grep -c '^## [0-9]' docs/guide/deprecations.md` | confirmed |
| N6 | struct-literal breaks in MINOR releases | v2.40.0's own notes: "`GossipConfig` gained `gateway_max_connections`, …" | `CLAUDE.md` release paragraph; `deprecations.md` § *Will the compiler tell me?* | confirmed |
| N7 | SDKs hand-written against no OpenAPI schema | no OpenAPI or Swagger file, no `utoipa`; no generator in either SDK. Cross-language pins that do exist: golden vectors and `declaration.schema.json` (the unit file, not the gateway) | `find . -iname '*openapi*' -o -iname '*swagger*'`; `git grep -il "openapi\|utoipa"` | confirmed |
| N8 | 76 routes in the main router, 86 overall | **76** in `src/agent/http/mod.rs`; **86** in `src/`; 113 with the companions' routers | `grep -c "\.route(" src/agent/http/mod.rs`; `git grep -n "\.route(" -- 'src/*.rs' ':!*_tests.rs' \| wc -l` | confirmed |
| N9 | four separate append-log implementations | **4**: `mycelium-core/src/persistence.rs` (+ `writer.rs`), `src/agent/journal.rs`, `mycelium-tuple-space/src/store.rs`, `mycelium-blackboard/src/wal.rs`; none checksummed, three say so (L4) | the grep at L4 | confirmed. `journal.rs` already backs seven consumers (the 2026-10-31 plan, F7) |
| N10 | `src/consensus.rs` 5.5k lines (6,503 with tests), `propose_inner` 414 | **6,503** lines; first `#[cfg(test)]` at 4,984; `propose_inner` 1825–2238 = **414** | `wc -l`; `grep -n "fn propose_inner"`; `awk` to the closing brace | confirmed |
| N11 | `handle_connection` is a single 929-line function | 367–1295 = **929** | `grep -n "fn handle_connection" mycelium-core/src/connection.rs`; `awk` to the closing `}` | confirmed |
| N12 | `TaskCtx`: 90 fields and 59 cfg attributes | **90** fields, **61** `#[cfg` lines (`src/agent/mod.rs:696`–`1093`) | `awk` over the struct | confirmed |
| N13 | …the cause of 56 `too_many_arguments` allows | **57** allows. **The cause was not shown:** 16 are outside the root crate, 13 of them in `mycelium-core` (`persistence.rs` 6, `writer.rs` 3, `swim.rs` 2, `kv_persist.rs` 1, `connection.rs` 1), whose `CoreCtx` is `TaskCtx`'s lower half (`mycelium-core/src/context.rs:111`–`113`), so the crate boundary alone proves little; the largest clusters are `gateway_caller.rs` 7 and `consensus.rs` 6. Whether grouping `TaskCtx` would remove them is what CR21 measures | `git grep -n too_many_arguments -- '*.rs' \| awk -F: '{print $1}' \| sort \| uniq -c` | count confirmed; cause **not confirmed** |
| N14 | `src/lib_tests.rs`: 18k lines in one file | **18,466** | `wc -l` | confirmed |
| N15 | about 650 fixed sleeps in tests | **770** `sleep(` calls in test-named files and `tests/` directories (214 in `lib_tests.rs`); 1,015 in all `.rs`. A grep cannot tell a fixed sleep from a poll loop's interval | `git grep -n "sleep(" -- '*.rs'`, split by path | confirmed in order of magnitude; *fixed* **not verified** |
| N16 | a retry wrapper referenced 30 times in CI | **30** lines name it; **27** are invocations of `scripts/ci-retest.sh`. It re-runs only failed tests, once, annotates a pass-on-retry, and under `CI_RETEST_STRICT=1` fails it | `grep -c "ci-retest" .github/workflows/ci.yml` | confirmed; the wrapper is stricter than "retry" suggests |
| N17 | roughly 15–20% of comments narrate review history; several are stale | **7.9%** of 3,290 comment blocks in `src/`, `mycelium-core/src/` and `mycelium-wasm-host/src/` (tests excluded) carry a PR number, "review", "round N" or "Run-N". In a systematic sample of 40 blocks, 6 carry a plan-row or date tag and 1 narrates what the code used to do instead of the invariant. *Stale* was not assessed | blocks = runs of consecutive `//` lines; every 83rd block read | **confirmed, narrower**: about 8% by marker, 15% if every plan-row tag counts |

### 2.5 · The philosophy

| | Critique | Verdict or position |
|---|---|---|
| Ph1 | Signals and Boundaries is no longer what ships; restate as "flood to discover and broadcast, route what is addressed" | **The maintainer's position (§3 M2):** the principle is unchanged. The reviewer's narrower point — the page claimed more than the code for Individual routing and Group sampling — was accepted, and #771 reworded the page (§6.3); what it now says matches the code (`docs/philosophy.md:11`–`21`; `tasks.rs:368`–`370`). The second half — signed signals, end-to-end encryption — is §5 D6 |
| Ph2 | "Consensus is a protocol the substrate is unaware of" leaves no authoritative membership; make the electorate mandatory; model the acceptor; openraft | L11 and L12. Mandatory electorate: CR9, §5 D1. The model: taken, CR10. openraft: a counterfactual, agreed by both sides (§3 M5) |
| Ph3 | "Detection, not prevention" is right for availability and wrong as a security posture | **agreed and adopted** as the owner-signed rule (#772) |
| Ph4 | "Library, not platform" no longer describes the product | **decided (§3 M3):** it stays, described as a library that also ships a reference node; the positioning to say plainly that Python and TypeScript users run it as a sidecar. Today only `docs/philosophy.md:194` says "sidecar" (`grep -n -i sidecar docs/positioning.md README.md`: none). CR16 |
| Ph5 | Cut the lineage to Holland, Promise Theory, the litmus tests and Properties 8 and 9 | **decided (§3 M4):** the maintainer's call; not planned |
| Ph6 | Defaults: warn at start; self-install opt-in | P4 (confirmed, narrower; CR15) and P5 (not confirmed) |

## 3 · Already decided, already done, in progress

Recorded, not proposed. None of these is a finding to act on in §4 beyond what is stated.

- **M1 · The owner-signed rule is written** (#772, 2026-11-12): the rule, its four conditions and the inventory.
  It is the verification of §2.1 and of the unsigned-record parts of §2.2. The reply's second addition ("write
  item 2 as a rule") is done.
- **M2 · The forwarding statements are reworded** (#771, 2026-11-12). The maintainer's position: forwarding is
  unconditional on admission and on a route; a direct send is an optimisation over the flood; a boundary is not a
  confidentiality mechanism. The page says so.
- **M3 · "Library, not platform" stays**, described honestly: a library that also ships a reference node. The
  positioning is to say plainly that Python and TypeScript users run it as a sidecar (CR16).
- **M4 · Trimming the philosophy's intellectual lineage is the maintainer's call and is not planned.**
- **M5 · openraft is a counterfactual; the model-checking suggestion is taken** (CR10).
- **M6 · The KV read guard is being built** on branch `kv-read-guard` by another agent (no pull request open when
  this was written). Decided: sensitive key names hidden from the listing, every owned key's value refused, the
  log read doors as a follow-up. The reply's first addition ("do item 1 today") is in hand. This plan does not
  touch that work; CR1 records it.
- **The order** was, at rev 0.2, agreed for two things only — CR1 and the rule, now — and proposed for the rest
  (the six items of the project's first response, listed in the vendored file's header, which the reviewer's
  reply called right). **Since 2026-11-12 it is agreed (D11):** CR1 now; CR2's first PR, CR13's counter and CR12
  inside wave 1b; the rest after it; CR10 in parallel.

## 4 · The rows

Each row: one PR in its own worktree, every fix written fail-first against a planted fault, an independent
adversarial review before merge, a feature page updated where the feature changes (rule 5), and for every record
row the forged-record test the owner-signed rule's condition (d) asks for. Sizes are rough: **S** up to a day,
**M** a few days, **L** a week or more, counted in PRs where it is more than one. The order is the **agreed**
order (§5 D11, decided 2026-11-12); tier 1 is the security work, tier 2 the smaller Layer I items, tier 3 the structure.

Each row carries a **State** line, as of 2026-11-12. *Buildable* means the row's text needs no further
decision; *after wave 1b* is D11's schedule, and says separately whether the text is ready; *decision recorded,
no row* means §5 answered the question and nothing is built from it.

Two standing requirements of every row that reads a clock or keeps reader memory: wall time is read through
`mycelium_core::sim_seam::wall_now_ms` (CR3, CR7's expiry, CR11, CR13), and a new `Mutex` or `RwLock` field for a
reader's memory (CR2, CR3, CR7) adds its row to the lock-order table.

A row that moves an inventory row to *Meets* cites the verifying code and the forged-record test on
`owner-signed-records.md` in the same PR.

### Tier 1

#### CR1 · The KV read guard, then the log read doors *(L19)* — delivered

**State (2026-11-12):** **delivered** — the guard is PR #774, merged (`deprecations.md` §99); the log read doors
follow-up is PR #777 (`deprecations.md` §101). The follow-up's exit, promise by promise:
- *"a `consensus:read` token is refused a scan and both subscribes of `cn/…`, `wiki/…` and `reason/…`"* —
  `a_consensus_read_token_reads_application_streams_only_at_every_log_door` (`compliance`; a `"*"` token too),
  and one test per door: `log_scan_refuses_an_owned_stream_and_reads_an_application_stream`,
  `log_subscribe_refuses_an_owned_stream_before_any_event`,
  `log_group_subscribe_refuses_an_owned_stream_before_the_claim` (`src/agent/http/log_read_tests.rs`);
- *"each seen admitted first"* — with `refuse_owned_stream_read` planted to refuse nothing, the four door tests
  fail on a 200 (the PR lists the counts);
- *"an application stream still reads"* — the same tests: `events` is scanned, subscribed and group-subscribed,
  an entry delivered at each.
- **Added by the review of #777** (the maintainer's decisions, 2026-11-12): the key *names* under `log/cn/`,
  `log/wiki/`, `log/reason/` and their `clog/` twins are hidden from `GET /gateway/kv/keys`, derived from
  `OWNED_LOG_STREAMS` — `the_raw_listing_hides_a_component_streams_key_names`; and a `log/{name}/…` family in
  `src/lib.rs`'s ownership table must be a row of that table or a stated application stream —
  `every_log_stream_in_the_table_is_classified_for_the_log_routes` (what it does not gate is in the table's doc).
- **Not built, a follow-up PR, surveyed first:** `GET /consensus/{slot}` still returns a component-owned slot's
  value under `consensus:read` — a commitment's award (`cn/{requirement}/award`) and a consumer group's claim.
  This row does not close it.

**What:** as decided (M6): on the raw KV read routes, an owned key's value is refused and sensitive key names are
hidden from the listing. **Follow-up, one PR:** `gw_overlay_log_scan`, `gw_overlay_log_subscribe` and
`gw_overlay_log_group_subscribe` (`src/agent/http/overlay.rs:587`, `:646`, `:714`) refuse an owned stream as the
append and compact doors do; `docs/operations/rbac.md:71` says what `kv:read` reads.

**Exit:** the other agent's, for the guard. For the follow-up: a `consensus:read` token is refused a scan and both
subscribes of `cn/…`, `wiki/…` and `reason/…`, each seen admitted first; an application stream still reads.

**Does not:** stop a member — a member reads the store; this is a door for an HTTP client. It is not the
owner-signed rule and satisfies none of its rows.

**Size:** S (the follow-up).

#### CR2 · Grant records a member cannot re-arm *(A1; inventory: `used/{door}`, `consumed`, `revoked`, the request, `issued`, `checkpoint`)*

**State (2026-11-12):** **the first PR (the use marker and its release) is *buildable*, inside wave 1b** (D11); concurrent admission at two gateways is stated, not prevented (D12). PRs 2–4 are *after wave 1b*, their text ready.

**What:** in this order, each its own PR:

1. **The use marker and its release.** A KV record alone cannot carry this, signed or not: while any member can
   out-stamp the owner, a tombstone planted at the predictable key `grant/{holder}/{term}/used/{point}`
   (`src/agent/http/authz.rs:1436`; `{point}` is the enforcement point's name, not a node) *before* the first use
   wins LWW, the door's own marker write still returns `Ok` (`write_owned_kv_durable` discards the receipt,
   `src/agent/state_machine.rs:981`–`983`; `kv_set_requiring_sync` returns `Ok` with the application rung it was
   given, `mycelium-core/src/ops.rs:495`–`511`), and no verified marker ever exists for a reader to remember. So:
   - **The door keeps the use in a node-local durable record** (its journal), read before the KV. A restart
     reads it back; a member's KV write cannot touch it.
   - **At the marker key, anything other than *never written* reads *used*:** a verified marker, a value that
     does not verify, or a tombstone. The marker itself is signed by the door's node, its body naming the grant,
     the door and the use.
   - **Only a signed release bound to that use reads unused** — signed by the same door, naming the use it
     releases (not only the door), so an earlier genuine release cannot be replayed over a later use. The gateway
     today tombstones the marker itself when a call is refused before dispatch (`authz.rs:716`); that becomes
     the signed release.
   - **The door checks its own marker write's receipt** and refuses the call, by name, if the write was
     `Superseded`.
2. **`consumed` and `revoked`**, the same shape: signed by the agent's node and by the revoker; a planted
   `revoked` no longer stops a real revocation, because the route reads a verified marker only.
3. **The request record** (`grant/{agent}/{request}`): the supervisor signs what it read, and the agent's honour
   window comes from the signed grant, not from the unsigned request's `expires_at_ms`.
4. **`issued` and `checkpoint`, the *Partly* conditions:** the scope compared with the path; a restarted node does
   not take an older genuine checkpoint over a newer one.

**Exit:** per record, the forged-record test. For the use marker: a member's tombstone of a used marker leaves the
grant used at a second gateway and at the same gateway after a restart; **a tombstone planted before the first
use** refuses the call (the receipt) and reads used; **a tombstone delivered to a second gateway before the
marker** reads used there; **a garbage overwrite at a higher stamp** reads used; **a replay of an earlier genuine
release over a later use** releases nothing; a forged release under another node's key releases
nothing. For the others: a planted `revoked` does not answer `already_revoked`; a replayed sequence-1 checkpoint does not revive a
revoked grant on a restarted node. Each seen failing first. The inventory rows move to *Meets*, and
`what-is-proven.md`'s first *Not yet shown* row moves up.

**Does not:** make two gateways' concurrent admission of one grant impossible — that is a consensus claim per
use, a separate decision (§5 D12). **Does not survive the tombstone horizon at a second gateway:** a marker's
tombstone or the marker itself is all a second gateway has, and once a planted tombstone is collected (3 000 s at
the defaults, L3) the key reads *never written* there again; a grant valid longer than that horizon can be
re-armed at a gateway that never recorded the use itself. Stated. No bound is built: D12 decided concurrent
admission is stated, and the honour window is not shortened by this row. A member can also *burn* a grant by planting a tombstone before its use: a refusal, which
is availability and outside the rule. Does not change the grant's wire form to the client. I4's collector
(`identity-and-lifecycle.md`, row I4) must also collect signed markers and releases; the row says so when it is
written.

**Format:** a KV **value** change for each marker (today a timestamp or `b"1"`), read for one release beside the
old form, counted — an upgrade note and a mixed-version rule (verifiers before writers), as I3's digest had. A new
journal stream for the door's own uses: additive on disk, with a lock-order row for its reader. During the mixed
release a pre-dispatch refusal at an un-upgraded gateway — which releases by tombstone
(`src/agent/http/authz.rs:716`) — spends the grant at upgraded doors: the safe side, stated in the upgrade note. No wire-frame
change, no change to an existing on-disk format.

**Size:** L — four PRs; the first is M to L and is the one that closes the open row.

#### CR3 · An audit chain whose truncation is detected *(A2; inventory: the chain's head, `sys/audit-checkpoint/`)*

**State (2026-11-12):** *after wave 1b*; the text is ready.

**What:** a signed head — the node signs `(node, newest seq, newest hash)` on each append or each checkpoint
interval, under its own domain tag — kept where a member's KV write cannot lower it: in the node's own journal,
and gossiped as a record a verifier compares by sequence so an older genuine head does not replace a newer one.
`verify` reports a chain shorter than the newest head it has seen as *truncated*, and an absent chain with a known
head as *missing*, not `Ok`. The checkpoint's `{seq}` is compared with its body, and a skipped checkpoint is
counted.

**Exit:** a chain whose newest records are tombstoned verifies as *truncated* on a peer that saw the head and on
the owner after a restart; a wholly deleted chain is *missing*; a replayed older head is ignored and counted; a
forged checkpoint under another key is refused and counted. Each seen failing first.

**Does not:** provide an external anchor — a fleet whose every node is rewritten together has no witness. That is
an exporter's job (`AuditSink` is a mirror that drops when full) and is stated, not built. Does not detect
truncation by a verifier that never saw a head.

**Format:** a new KV record and a new journal stream; additive. No wire or on-disk format change to existing
files.

**Size:** M.

#### CR4 · `cap/`, `req/` and `prov-shed` advertisements carry their node's signature *(A3; inventory: `cap/{node}/…`, `req/…`, `cap/{node}/prov-shed/…`)*

**State (2026-11-12):** *after wave 1b*. Its first PR is the measurement, and the signing PRs are not buildable until that measurement is in the row.

**What:** an advertisement's value carries its node's signature over `(node, ns, name, body)`; the resolvers
(`resolve_for_caller`, `resolve_filter_at`), the demand snapshot and the shed ranking read verified records only,
and count the rest. `filter_protected` (H3) is given its caller, so an unbacked advertisement in a protected
namespace does not resolve — Boundary H's H3, built as a primitive and not in force.

**Exit:** a member's `cap/{victim}/…` does not resolve and does not attract a call; a forged `req/` installs
nothing and a tombstone of real demand is re-asserted and does not suppress an install past one round; a forged
`prov-shed` neither holds a band above its ceiling nor sheds it below; each counted where `/stats` shows it.
`what-is-proven.md`'s H3 row moves up.

**Does not:** cover `tools/`, `skills/`, `prompts/`, `cap-group/`·`gcap/`, `mailbox/`, `comp/` or the application
prefixes — CR5b. Does not authenticate anti-entropy; the signature is in the value, so it survives a state reply.

**Format:** a KV **value** change on the busiest soft-state namespace: every advertisement grows by a signature
and a verifier's check. One release of reading both forms, counted. **This row needs a measurement first** —
advertisement refresh rate × fleet size against the signing and verifying cost — and the measurement is its first
PR. No wire-frame change.

**Size:** L — a measurement, then two or three PRs.

#### CR5 · The remaining *Does not hold* and *Partly* rows, grouped

**State (2026-11-12):** *after wave 1b*, and **not yet rows**: each group is written as a row once CR2–CR4 show the cost. CR5f is written **with I2's row** (wave 1b). CR5a's signed-`sender` part waits on D6's measurement; anti-entropy stays unsigned (D8).

Not one row to build; six groups (CR5a–CR5f) to be written as rows once CR2–CR4 show the pattern's cost. Every *Does not
hold*, *Partly* and *Bounded, not detected* row of the inventory is named in CR2–CR5 or CR7 (checked by grep for
each record's prefix against this file).

- **CR5a · Caller identity** — `sys/caller-context/` (unsigned marker), `sys/caller-context-id/` (a tombstone
  fails open), the key-less principal, the bare RPC `sender`. Depends on §5 D6 (signed signals) and on O3; the
  marker rows can go first. *Format:* value changes; D6 would be a wire change.
- **CR5b · What a model is told and where a call goes** — `tools/`, `skills/`, `prompts/`, `cap-group/`·`gcap/`,
  `mailbox/`, `wiki/…/proposal`, `comp/`, `ckpt/`·`ckptw/`·`strands/`. After CR4, same shape.
- **CR5c · Consensus records** — `consensus/life/…`, the legacy `committed`/`lease` reading, the acceptor's
  memory (`sys/consensus-accepted/`) and the ballot and decided floors, `consensus/trust/`, `sys/topology-override/`, `grp/`, `sys/quorum/`, `sys/load/` under
  `count_opaque_as_absent`, the self-certified electorate genesis. **After CR10**: these are the records the model
  should say are safe to sign before anyone changes the acceptor. *Format:* value changes, and the acceptor's
  durable record is an on-disk format.
- **CR5d · Governance and policy** — `sys/capauthz/` (a tombstone removes the role requirement),
  `sys/govern/membership/`, `sys/govern/fleet`·`sys/config/` (no range at read), `sys/role/` currency,
  `sys/membership/removed/` across a restart, `sys/identity-cert/{self}` as its owner reads it; the two
  *Bounded, not detected* rows `sys/govern/timing` (name its detector) and `cap/{node}/locality/self`.
- **CR5f · The identity records' *Partly* conditions** — `sys/identity-signed/{node}`,
  `sys/identity-cert/{node}/{key-hex}` as verifiers read it, `sys/revocation/…` and `sys/client-cert/…`: signed
  and verified today, each short of *Meets* on counting a failure or on a forged-record test for a tombstone.
  **These are the records I2 (CA succession) touches**, so this group is written with I2's row, not after it.
- **CR5e · The tally, exposure, and the companions** — `sys/ae-tally/`, `sys/ae-exposure/` (both *stated*),
  `sched/…/last`, `cn/` heads and logs, `cn/{requirement}/award` (the unverified `award_of` path), `facts/` key binding, `log/reason/` (untraced: trace first).

**Exit, for the group:** every inventory row has a plan row or a sentence saying it is accepted as a limit, by
name, on `what-is-proven.md`.

**Size:** not estimated until CR2–CR4 are measured.

#### CR6 · One door function for authorization *(L18; the critique's code area 1)*

**State (2026-11-12):** *after wave 1b*; the text is ready. The route table derived from the router is the one D4 asks for before any schema.

**What:** one function that owns, for a governed call, the sequence every door repeats by hand — the protected
kind refusal, the companion-kind gate, the preflight, the grant use, the dispatch, the release on a refusal before
dispatch, and the evidence record — and a typed proof of authorization (the reviewer's `Authorized<Scope>`) that a
dispatch function requires, so a door that skips the sequence does not compile. The nine protected-kind sites and
the eight preflight sites of L18 become callers of it. The door list the C7 matrix and `check-causal-doors.py`
keep by hand is derived from the router, so a new route is in the matrix or fails the gate.

**Exit:** a planted door that dispatches without the proof fails to compile (a `trybuild`-style test, or the
dispatch function's signature shown private to the door module); the bypass matrix enumerates routes from the
router and a planted unregistered route fails it by name; the existing door tests pass unchanged — the row changes
no decision, only who makes the call.

**Does not:** change any scope, any refusal code, or what any door decides. Does not cover the provider side
(`provider_enforcement::check` stays its own door, with the same proof type if it fits). Not a rewrite of
`authz.rs`.

**Size:** L — one PR for the function and the type with two doors moved, then the rest by family.

#### CR7 · Catalogue currency, and provenance before staging *(L15, L17, P5; inventory: `installable/…`)*

**State (2026-11-12):** *after wave 1b*; the text is ready, its choices decided: a revocation refuses new installs and does not stop a running one (D9); a v1 line is refused after one release of both forms, and the library `Provisioner` refuses unsigned lines by default in that release (D13).

**What:**

1. **Provenance before staging**: the stem's staging loop (`stem.rs:505`–`517`) filters by `provenance_ok` before
   it stages, and `size_bytes` — which sets the staging ceiling and the install budget — moves inside the signed
   message, or is replaced by the signed `requires.disk_bytes` as the ceiling.
2. **Ranking on signed fields**: candidates for one capability are ordered by what the publisher signed, not by
   the two unsigned hints (`provisioner.rs:1147`).
3. **Version, expiry and revocation**: a line's signed message gains a publisher sequence per `(ns, name)` and an
   expiry; a reader holds the highest sequence it has verified per publisher and name — **durably, in a
   node-local record, or a restart forgets it** — and refuses a lower one (rollback) and an expired one; a publisher-signed revocation of an artifact hash is honoured before install.
   Withdrawal becomes a signed record, so a tombstone is not the only way to withdraw and a replay of the
   withdrawn line loses.
4. **The library type refuses by default** (a change of a library default: §5 D13, decided yes): a `Provisioner` with no
   trusted publisher installs nothing unless unsigned lines are accepted by name, as the stem already requires.

**Exit:** a line never signed by a trusted publisher is not staged (a counter, and no bytes on disk); a withdrawn
line re-written by a member is refused as *superseded* and counted; an expired line is not installed; a forged low
`size_bytes` does not win selection; `Provisioner::new` with no publishers installs nothing; a restarted reader
still refuses the lower sequence; a publisher-signed revocation stops an install; a `-proposed-v2` line carries
the same currency. Each seen failing first. The inventory row moves to *Meets*.

**Does not:** adopt TUF. The reviewer's "TUF-style" is the shape (version, expiry, revocation), and this row takes
those three; root rotation, delegated targets and threshold signing are not built and are stated. Does not
replace a running install on a revocation — whether a revoked artifact is stopped was §5 D9 (decided: it is not). **Does not protect a
reader that never saw the newer line:** a newly joined stem — the headline case — holds no sequence and accepts a
replayed, unexpired line until its expiry; expiry is the only bound there, so the row's value depends on publishers
choosing short ones. Does not stop the composition of L15 by itself: a member can still force a re-install (CR4
closes the shed).

**Format:** the entry's encoding gains fields (sequence, expiry, and `size_bytes` moving under the signature) — a
KV **value** change with a mixed-version rule (readers before publishers) — under a new signed-message domain
(`mycelium-installable-v2`), and the proposal domain `mycelium-installable-proposed-v1`
(`mycelium-wasm-host/src/catalog.rs:160`) gains its `-v2` with it, so a proposed line and its reviewer's acceptance
carry the same currency. A new node-local record for the highest sequence seen: additive on disk. Whether and when
a v1 line stops verifying was §5 D13 (decided: after one release of both). No wire-frame change.

**Size:** M — two PRs (1, 2 and 4; then 3).

#### CR8 · An unconfined activation is said aloud *(L16)*

**State (2026-11-12):** *after wave 1b*; the text is ready.

**What:** the stem warns at start, not at info, for each activation with no `[activation.confine]`; the threat
model gains the line the unit-file reference already has. Whether confinement becomes required is not proposed:
the command is the operator's own, from a reviewed file.

**Exit:** the warning's text pinned by a test; `threat-model.md` states it.

**Does not:** confine anything by default.

**Size:** S.

#### CR9 · The exclusive verbs refuse without an electorate, in every profile *(L11; Ph2)*

**State (2026-11-12):** *after wave 1b*, and **not buildable as written**: D1 is decided — a deprecation cycle — but the single-node quickstart's answer (a declared electorate of one, `init` declaring it, or a named `dev` allowance) is to be written into this row before its first PR. The first release turns the existing counter into a start-time and per-call warning naming the release that refuses; the refusal is the release after.

**What:** a safety-sensitive proposal — the families of `SAFETY_SLOT_FAMILIES` (`src/agent/electorate.rs:74`:
`lock/`, `leader/`, `consistent/`, `capauthz/`, the electorate slots) — outside an electorate group is refused
`ElectorateNotGoverned` whatever `consensus_require_electorate` says. Today the flag's off arm counts it and lets
it through (`src/consensus.rs:3328`–`3330`). The single-node case (a fleet of one, the quickstart) needs an
answer: a declared electorate of one, declared for the caller by `init`, or an explicit `dev` allowance by name.

**Exit:** on `GossipConfig::default()`, `distributed_lock`, `elect_leader` and `consistent_set` return the refusal
with no electorate declared, in the Rust API and at each gateway route and SDK verb that reaches them (enumerated
first: rule 1); the quickstart, every example and both SDKs' suites pass with an electorate declared or the
allowance named; the two-sided partition that today is a reading becomes a test — five nodes, a 3\|2 split, the
minority's lock refused or timed out, never granted.

**Does not:** change the non-exclusive verbs — `cluster_propose` and `group_propose` without `safety_sensitive`
stay "coordination that says what it means". Does not build disaster recovery for a lost majority.

**How it ships was §5 D1, decided 2026-11-12: a deprecation cycle** — the counter that exists today becomes a
start-time and per-call warning naming the release that refuses, then the refusal. The single-node quickstart's
answer is still to be written here (the State line above).

**Size:** M for the code; the examples and SDKs are most of it.

#### CR10 · A model of the acceptor *(L12; Ph2; M5)*

**State (2026-11-12):** ***buildable*, startable in parallel with wave 1b** (it touches no shipped code). The tool is Stateright (D2). The lease area models CR11's margin as designed; until CR11 lands, the `what-is-proven.md` row says that area checks the design, not the code.

**What:** a model, checked in CI, of the single-decree acceptor with the three things the hand-written
interleavings cover one case at a time: **slot reuse** after a release or a lapsed lease, the **lease's
wall-clock end** under a skew bound δ, and the **electorate migration** (declare the successor, retire, carry,
complete). Invariants: at most one value decided per slot and life; no holder granted while another is inside its
own deadline minus δ; nothing decided by a successor before the predecessor's retirement is visible. The model
is written against the decision functions the engine already keeps pure, so the model and the code share them
where the tool allows.

**Exit:** the model finds a planted fault in each of the three areas — a vote counted from outside the
electorate, a lease ended with no margin, a successor deciding early — and passes without them; it runs in CI
with a stated bound (nodes, slots, depth); `what-is-proven.md` gains a row saying what the bound is.

**Does not:** prove the implementation. A model checks the design at a bound; the link to the code is the shared
decision functions and the `row01`–`row27` tests. Not a linearizability checker over recorded histories — that is
a separate, later row if D2 chooses it.

**Tool was §5 D2, decided 2026-11-12: Stateright.**

**Size:** L.

#### CR11 · The lease's skew bound, stated where an adopter reads; a holder-side margin *(L13, L14)*

**State (2026-11-12):** *after wave 1b*. The docs half is ready. The margin and its guarantee are ready; a `secure-single-domain` revision requires the guarantee when that profile next changes for another reason, not in a revision of its own (D14). No library fenced write is built (D15).

**What:** docs first: the skew bound of `lock-lifecycle.md:316`–`320` in the threat model's §7 table and the
consensus guide, and on `what-is-proven.md` (added by this PR). Then a **holder-side margin as a guarantee**:
the holder stops a stated δ before its deadline, and a guarantee resolves *Enforced* only when δ is non-zero and
below the lease. δ is 0 unless set; where an `[authority]` table is present, its `skew_ms` (default 1 000,
`mycelium-core/src/config.rs:1872`–`1877`) is the default. A node with neither is unchanged and the guarantee
reports `NotConfigured`. The figure is **not** `max_clock_drift_ms`, which is the transport's HLC tolerance
(300 000 ms, `mycelium-core/src/hlc.rs:92`) and would lapse the default 30 s leader lease
(`src/agent/overlay_consistent.rs:143`) at the moment it is granted. Profiles require guarantees and set no
configuration, so "the secure profile sets it" is not available; whether a new revision of `secure-single-domain`
*requires* the guarantee was §5 D14 (decided: in the next revision that changes for another reason).

**Exit:** a holder with the margin reports its lock lapsed δ before a reader with a clock δ ahead would grant
another; a margin of zero, or one at or above the lease, resolves the guarantee `NotConfigured` by name; a node
with an `[authority]` table and no margin set takes its `skew_ms`; a node with neither behaves as today.

**Does not:** check a fencing token anywhere. The first draft proposed "a fenced `consistent_set`"; the review of
#773 showed it does not exist to be fenced — `consistent_set(key, value)` takes no token and is its own consensus
round, not a lock holder's write (`src/agent/consensus_handle.rs:466`–`470`) — and that "the highest token this
node has stored" is local to one node, while in the skew case the two holders are on different nodes and LWW
orders by HLC, not by token: the test could pass while the property fails. A library-provided fenced write would
be a new verb, a gateway route and both SDKs, with the token compared where the value is decided. Whether to build
that was §5 D15 (decided: not built). Fencing stays the application's contract, as the threat model states.

**Size:** S (docs, in part done here), then S to M for the margin and its guarantee.

#### CR12 · `GossipConfig` is `#[non_exhaustive]`, with a constructor *(N4, N6)*

**State (2026-11-12):** **inside wave 1b** (D3, D11), *buildable* for `GossipConfig` (D3): the next MINOR, `default()` plus assignment, no builder. The other config structs are enumerated in the PR and included only on the maintainer's yes.

**What:** `#[non_exhaustive]` on `GossipConfig` (and on the config structs that have grown in a MINOR —
`GatewayTlsConfig`, `OidcConfig`, `TlsConfig` — enumerated first), so a new field stops being a compile break for
a struct-literal caller. The construction path is `GossipConfig::default()` plus field assignment, which
`#[non_exhaustive]` still permits outside the crate, or a builder if the maintainer wants one (§5 D3). Every
example, test crate and companion that builds one with `..Default::default()` from outside `mycelium-core` is
changed in the same PR, because that syntax stops compiling for them.

**Exit:** the workspace and both feature-off crates build; an external-crate compile test shows a struct literal
is refused and `default()` plus assignment is not; the next field added in a MINOR needs no *Will the compiler
tell me?* entry.

**Does not:** reduce the 84 fields or group them.

**How it ships was §5 D3, decided 2026-11-12: the next MINOR** — it is itself a source break for every downstream struct literal, once. Wave 1b's I7
already adds a field to `GossipConfig` and says "an exhaustive literal breaks"
(`identity-and-lifecycle.md:1708`), so landing this row in the same MINOR breaks those callers once instead of
twice. The #773 review counted 114 literal sites in 71 files outside `mycelium-core`; the row's first step
re-counts them.

**Size:** M (mechanical, wide).

### Tier 2 · The smaller Layer I items, and what is said aloud

#### CR13 · An inbound entry stamped beyond the drift bound: counted, then refused *(L2)*

**State (2026-11-12):** **step 1, the counter, is *buildable*, inside wave 1b** (D11, D16). **Step 2, the refusal, is not buildable yet**: it waits for a release of counting and for the slow-clock case to have an acceptable answer (D16), and it lands default-on with a recorded carve-out in runtime-invariants, in the same PR.

**What:** in two steps, as every other refusal in this repository ships (O3, O5, O6; D1's recommendation):

1. **A counter**, where `max_clock_drift_ms > 0`: an inbound KV entry whose physical time is beyond wall + the
   bound is counted (`kv_future_stamps` on `/stats` and in metrics) at each of the three doors that take a remote
   stamp — the `StateResponse` arm (`mycelium-core/src/connection.rs:852`), `Data` (`:1060`) and `SignedData`
   (`:1254`) — and still applied. The clamp has no counter today, so nobody knows how often a healthy fleet would
   trip a refusal.
2. **The refusal**, a release later if the counter is quiet: such an entry is not applied. The check comes
   **before** `hlc.observe` and before `wal.begin_inbound()` (`connection.rs:185`), so a refused entry moves no
   clock and opens no WAL window. `apply_and_notify` is untouched, so a node's own stamped writes are not
   affected. It reads the entry's stamp and the receiver's clock: no prefix, no owner.

**A second shape for step 2, to weigh with the counter's data (the maintainer's question, 2026-11-13): local
non-admission, forwarding unchanged.** Step 2 as written does not say what happens to the frame after the entry is
not applied. The signal mesh's rule is that a node always forwards and decides locally whether to *act*
(`Boundary::admits`, `mycelium-core/src/signal.rs:140`). The same rule for a KV entry: a node whose clock puts the
stamp beyond its bound **does not apply it to its own store, counts it, and forwards the frame as it would any
other** — a live `Data` or `SignedData` frame is already forwarded by its TTL whether or not it won LWW
(`mycelium-core/src/connection.rs:1089`–`1119`), and an anti-entropy entry is never forwarded (TTL 1), so there
the two shapes are the same. It is not the `Boundary` type itself: that sees a signal's scope only, and a KV frame
never passes through it; opacity is a load hint and carries no stamp. It is the boundary's *principle* applied at
Layer I's three doors.

- *What it changes:* the framing, and with it D16's carve-out. "Admission is local; forwarding is unconditional"
  is what the design already says, so this shape may need no carve-out from *detection, not prevention* where a
  refusal needs one. Each node judges by its own clock and its own `max_clock_drift_ms`, as each holds its own
  boundary.
- *What it does not change:* the slow-clock receiver, the replay door, the node-local repair and the mixed
  configuration case below are the same under either shape. A node that will not apply an entry is still offered
  it at every anti-entropy round by a peer that holds it.
- *What it costs:* containment. A forwarded far-future entry still reaches, and pins, a node whose bound is 0 or
  whose clock is ahead; a refusal that also stops the forward would not carry it there. The trade is consistency
  with the forwarding principle against how far one poisoned entry travels.

D16's decision stands — the counter, then a refusal conditional on what the counter shows. Which of the two
shapes the second step takes is chosen when that step is written, with the counter's figures and this trade
beside it; until then neither is buildable.

**The cases the row must answer, each an exit test:**

- **A receiver whose own clock is slow** — booted before NTP, or more than the bound behind. Today it converges
  on data and only its clock is clamped; with the refusal it would refuse the whole fleet's state. The exit shows
  it refuses, counts, and is repaired by anti-entropy once its clock is right — and the row says how long that
  takes and what the node serves meanwhile. If that is not acceptable, step 2 is not built.
- **Replay** (`src/agent/lifecycle.rs:494`–`511`), the fourth door. It is not a free choice. Refusing at replay
  against the wall clock discards a node's own WAL on a bad-clock boot; keeping the entry means permanent
  divergence from the peers that refuse it — the bucket digests differ, and the entry is re-sent and re-refused
  every anti-entropy round. The row keeps at replay and needs **a repair that is node-local, not gossiped**: each node
  that holds an entry stamped beyond its bound removes it from its own store and WAL (an operator verb by key, or
  the node's own check at start), counted. A tombstone "at a stamp that wins" is not the repair: to beat a
  `u64::MAX` entry it must itself be stamped `u64::MAX` (`mycelium-core/src/store.rs:595`), so it is itself
  beyond the bound, is refused at the three doors, and is never collected (`store.rs:967`). Without the repair,
  the row states the churn.
- **Mixed configuration, not only mixed version:** a node with `max_clock_drift_ms = 0` accepts what the others
  refuse, permanently. `validate()` or the guarantee report says so; the upgrade note states both.
- A `u64::MAX`-stamped entry through each of the three doors leaves the key as it was; an entry inside the bound
  applies; with the bound at 0 nothing changes.

**Does not:** remove a poisoned entry a fleet already holds, without the node-local repair on every node that holds it. Does not authenticate the
entry. Does not stop a stamp *inside* the bound: a member can still out-stamp an owner by up to five minutes,
which is what CR2's first step had to be rewritten around. Does not change the wire.

**Size:** S for the counter; M for the refusal with its four cases.

#### CR14 · A per-record WAL checksum *(L4; the critique's code area 3, first half)*

**State (2026-11-12):** *after wave 1b*; the text is ready (the all-v1-or-all-new shape is the proposal the row's PR confirms).

**What:** each KV WAL record carries a checksum over its payload, verified at replay; a mismatch is `Corrupt` at
that offset, with the existing refuse-or-quarantine behaviour.

**The constraint, from the [contracts-receipts ADR](../design/contracts-receipts.md) §9:** every released on-disk
file must keep replaying, and `golden_fixture_replays_every_released_on_disk_format`
(`mycelium-core/src/persistence.rs:2895`) replays `tests/fixtures/persistence/fixint-v1/` in CI. The v1 WAL has no
magic and no version byte, so the new format must be distinguishable without one: **a header record a v1 reader cannot decode as an entry**.
(A new file name is not an option: a released binary would find no `wal.bin` and start empty, silently.) The row
**adds** `tests/fixtures/persistence/<new>/`, never edits `fixint-v1/`, and the reader replays both. **A
downgrade cannot refuse by name** — a released binary cannot be taught a new message. The best available is that
it meets the header as its first record and stops with its existing error, "holds a corrupt record at byte 0
with data after it" (`persistence.rs:581`–`585`), or quarantines under `on_unreadable = "quarantine"`; the row
says so in its upgrade note and tests it against the `fixint-v1` reader's code path. **The mixed file:** an
upgraded node meets a `wal.bin` of v1 records with no header. The row takes one of two shapes and says which —
the node snapshots and truncates before its first append (the startup snapshot already repairs a torn tail before
the first append), so a file is all-v1 or all-new; or the header may appear mid-file and the reader switches at
it. The first is simpler and is the proposal. Where the startup snapshot did not run or failed
(`on_unreadable = "quarantine"`, `src/agent/lifecycle.rs:586`–`590`; the `defer_snapshot` hook, `:556`–`572`),
the writer does not append new-format records behind v1 ones: it refuses, or writes v1 until the next snapshot.
An exit test covers the quarantine start.

**Exit:** a flipped bit inside a record's value that still decodes — the case nothing detects today — is
`Corrupt`; the v1 fixture replays byte for byte; the new fixture replays; an old-format WAL is read and the
startup snapshot leaves a new-format file before the first append; the v1 reader given a new file reports the
corrupt record at byte 0 and replays nothing as data; the `floor_*` pins and the acknowledgement rungs are unchanged (a checksum changes
no ack's meaning).

**Does not:** checksum the snapshot (a second, smaller PR), the journal or the companions' logs — that is CR18,
where one log is extracted. **It is an on-disk format change.** The reply's "neither needs a wire change" is true
of the wire only.

**Size:** M.

#### CR15 · The start says what a credential does not secure *(P4)*

**State (2026-11-12):** *after wave 1b*; the text is ready.

**What:** one `warn!` at start when a gateway credential is configured and no evaluator is attached, naming what
is then not checked (the seam, provider enforcement, the journal), beside the `dev`-profile warning.

**Exit:** the text pinned; silent when an evaluator is attached or `dev` was selected by name.

**Does not:** change a default.

**Size:** S.

#### CR16 · The positioning says "sidecar" *(Ph4; M3)*

**State (2026-11-12):** *after wave 1b*; the wording is the maintainer's and is given before the PR.

**What:** `docs/positioning.md` and the front doors it gates say that Python and TypeScript users run the
reference node as a sidecar and reach it over the gateway, and that the project is a library that also ships a
reference node. Through `scripts/check-positioning.sh`, which pins the hero and the sentence.

**Exit:** the gate green with the new wording on every door.

**Size:** S. Wording is the maintainer's.

#### CR17 · Limits stated where they are not *(L3, L5, L9, L10)*

**State (2026-11-12):** *after wave 1b*; the text is ready. D6 and D7 decided that the relay and SWIM limits are stated here, with the quorum consequence.

**What:** docs only, in the threat model: the tombstone horizon and the resurrection case with its number and its
knob (L3); that SWIM on an ordinary `[tls]` fleet lets a host on the UDP port evict a peer, and — until CR9 lands — shrink
the peer view a default lock's quorum is sized from (L5 with L11); that a signal's
nonce is burned before admission and nothing repairs it (L10); that a relaying member reads what it relays, and
that above about nine nodes relays are the common path for an Individual frame (L7, L8); the far-future stamp, until CR13 lands; the lease's skew bound (CR11's docs half).

**Exit:** each sentence beside the code line it states.

**Size:** S.

### Tier 3 · Structure

Lower priority; each is a refactor with no behaviour change, and each waits for the tier-1 row that touches the
same code. **State (2026-11-12), all of CR18–CR23:** *after wave 1b*, each after the row its last column names;
the two unnumbered lines are *decision recorded, no row* (D5: no feature-flag restructuring now; D4: a route
table checked against the router first, which CR6 derives, and no schema until an adopter asks for generated
clients).

| Row | What | Exit | Does not | Size |
|---|---|---|---|---|
| **CR18 · One append log** *(N9)* | Extract one checksummed, length-framed log from the four (KV WAL, `journal.rs`, tuple-space, blackboard), after CR14 fixes the format question once; crash-test it once | the four consumers on one implementation; one torn-tail and one corrupt-record suite; every existing fixture replays | merge the KV WAL's snapshot protocol into the journals | L |
| **CR19 · `consensus.rs` split** *(N10)* | `propose_inner`'s 414 lines into the state-machine steps CR10's model names; tests out of the 6,503-line file | no test changes; the model and the code share the step functions | change the protocol | L, after CR10 |
| **CR20 · `handle_connection`** *(N11)* | one function per `WireMessage` arm, the shared state in a struct | no behaviour change; the frame-filter tests pass | touch the wire | M, after CR13 |
| **CR21 · `TaskCtx`** *(N12, N13)* | group the 90 fields into per-subsystem structs behind the 61 cfg lines; remove the `too_many_arguments` allows that grouping removes, counted before and after | the allow count falls and the PR says by how many; the thirteen in `mycelium-core` are a separate, smaller change | promise zero allows | L |
| **CR22 · Test sleeps and `lib_tests.rs`** *(N14–N16)* | classify the 770 `sleep(` calls into poll intervals and fixed waits; convert fixed waits in the suites behind `ci-retest.sh` first; split `lib_tests.rs` by subject | the classification checked in as a count per file; the flake tier's annotations fall | remove the flake tier | L, continuous |
| **CR23 · Comment hygiene** *(N17)* | a comment states the invariant; the history goes to the commit and the plan. Applied to a file when a row touches it, not as a sweep; `/wiki-lint` gains a check for a comment that names a round or a review and no invariant | the marker count (7.9% today) reported at each release | a repository-wide rewrite | S, continuous |
| *(no row)* **Feature flags** *(N1–N3)* | *decision recorded, no row* (D5, 2026-11-12: not now) | — | — | — |
| *(no row)* **An OpenAPI schema for the gateway** *(N7)* | *decision recorded, no row* (D4, 2026-11-12: a route table first, in CR6) | — | — | — |

## 5 · Decisions — all sixteen decided by the maintainer, 2026-11-12

Each was the maintainer's, and each was answered on 2026-11-12 as recommended. The options and the argument are
kept as the record; the **Decided** line at the end of each is what holds. D13–D16 are choices the first
draft made inside a row; the review of #773 found them, and they are here now.

- **D1 · Is a mandatory electorate a MAJOR, or a deprecation cycle?** (CR9.) It changes what
  `GossipConfig::default()` does for `distributed_lock`, `elect_leader` and `consistent_set`: success today, a
  refusal after. *Options:* (a) a MAJOR; (b) a cycle — one MINOR in which the existing counter
  (`mycelium_consensus_ungoverned_safety_total`) becomes a start-time and per-call warning naming the release that
  refuses, then the refusal, as O3, O5 and O6 do; (c) leave the default and require it only in profiles, which is
  today. *Recommendation:* (b). It is the pattern the project already uses for a counter that becomes a refusal,
  the reviewer and the project agree on the end state, and the present behaviour is a stated limit, not a claim —
  but a lock that returns success from a minority is the kind of default that should not survive a second MINOR.
  The single-node quickstart needs its answer in the same decision.
  **Decided 2026-11-12:** (b), a deprecation cycle. The existing counter becomes a start-time and per-call warning naming the release that refuses; the refusal is the release after. The single-node quickstart is answered in CR9's row, before its first PR.
- **D2 · Which tool for the model?** (CR10.) Stateright (Rust, can share the pure decision functions, runs under
  `cargo test`) or TLA+ (better known, a separate language, no shared code). *Recommendation:* Stateright, for the
  shared functions and one toolchain; a short TLA+ spec later only if someone outside the project needs to read
  it.
  **Decided 2026-11-12:** Stateright.
- **D3 · `#[non_exhaustive]` on `GossipConfig`: when, and with what constructor?** (CR12.) It is a one-time source
  break for every downstream struct literal. *Options:* in the next MINOR with an upgrade note (the project's own
  rule lets a MINOR add fields, and each one already breaks the same callers); or held for a MAJOR.
  *Recommendation:* the next MINOR — **which is wave 1b**, so CR12 moves ahead of the rest of tier 1 in D11 — with
  `default()` plus assignment as the documented path and no builder; a builder for 84 fields is a second API to
  keep true. I7 breaks exhaustive literals in that release anyway.
  **Decided 2026-11-12:** the next MINOR (wave 1b), with `default()` plus assignment as the documented path and no builder.
- **D4 · An OpenAPI schema for the gateway?** (N7.) *Options:* write one by hand and gate it against the router;
  generate one (`utoipa` or similar, an annotation per handler on 76 routes); or keep golden vectors and the
  reviewer as the check. *Recommendation:* a hand-kept route table checked against the router first — the same
  derivation CR6 needs for the bypass matrix — and a schema only if an adopter asks for generated clients.
  **Decided 2026-11-12:** a route table checked against the router first (CR6 derives it); no schema until an adopter asks for generated clients. No row.
- **D5 · Feature flags: make consensus unconditional, move the gateway to its own crate?** (N1–N3.) The 2,346
  cfg lines and the two feature-off test crates are the cost of "embedded, pay for what you use". *Recommendation:*
  not now. Measure first which flags an adopter actually builds without; the gateway-as-a-crate move collides
  with CR6 and should follow it if it happens at all.
  **Decided 2026-11-12:** no feature-flag restructuring now; measured first (which flags an adopter builds
  without) if it is reopened. No row.
- **D6 · Signed signals, and end-to-end encryption for Individual scope?** (L6–L8; Ph1's second half.) Two
  separable things. **Signed signals** (the emitter signs a signal; a receiver verifies before it acts) closes
  reply forgery and the bare-`sender` row; it is a wire change (a new variant, as `SignedData` was) and a
  per-signal signing cost on the hot path. **End-to-end encryption** for Individual scope closes relay
  confidentiality; it needs a recipient key agreement the mesh does not have. *Recommendation:* decide signed
  signals with a measurement in hand (cost per signal at the fan-out the benches use), scoped first to RPC
  requests and replies, where the gap is concrete — forwarding targets are capped at `2·⌈log2 N⌉`, floor 8
  (`mycelium-core/src/config.rs:967`–`988`), so in a fleet above about nine nodes most Individual frames are
  relayed and a relaying member can answer for the target; state relay confidentiality as a limit in the threat model
  now (CR17) and do not plan encryption until an adopter's requirement names it. Both are wire-version decisions
  and belong with the anti-entropy one (D8).
  **Decided 2026-11-12:** measure signed RPC requests and replies first, and decide signed signals with that measurement in hand; state relay confidentiality as a limit in the threat model (CR17); no end-to-end encryption is planned.
  *The measurement has no row yet; written when taken up.*
- **D7 · SWIM authentication?** (L5.) *Options:* a MAC under a fleet-derived key on each datagram; require SWIM
  off under `secure-single-domain` as the federation profile does; or state the limit. It is more than eviction: with the default quorum sized from the peer view (L11), a forged `Dead` may shrink a
  quorum, from outside the membership. *Recommendation:* state it now with that consequence (CR17); CR9 removes
  the consequence for the exclusive verbs; make the secure profile's requirement the next step, because turning a liveness mechanism off in a
  profile is cheap and a keyed datagram is a design of its own (key distribution, rotation with the CA).
  **Decided 2026-11-12:** state the limit now, with the quorum consequence (CR17); then require SWIM off in the secure profile. A keyed datagram is not planned now; it is a design of its own.
  *The secure-profile requirement has no row yet; written when taken up.* **Proposed, pending the maintainer —
  this sentence was not put to them:** it follows D14's rule, joining the next `secure-single-domain` revision
  that changes for another reason.
- **D8 · Anti-entropy entries that carry their origin's signature?** (L1.) Already recorded as "a wire change, a
  separate decision" (`what-is-proven.md`, [*Anti-entropy entries authenticated*](../operations/what-is-proven.md#anti-entropy-entries-authenticated); the inventory § *Layer I*). The owner-signed rule is the answer
  that needs no wire change **for a record whose forgery is the risk. It is not sufficient for a presence-bearing
  record** — a use marker, a revocation, a consumed flag: the rule's condition (c) asks that deletion be as hard
  as forgery, and that cannot be met in the KV alone while any member can out-stamp the owner with a tombstone or
  a garbage value. CR2 therefore rests on a node-local durable record and on *anything but never-written reads
  used*, and still has a stated limit at the tombstone horizon. Entries that carry their origin's signature, with
  a receiver that refuses an unsigned overwrite of a signed record, would close it at the store — which is a
  Layer I law, and so a larger decision than a wire version. *Recommendation:* leave as recorded for now, with
  this limit stated; revisit with D6 if a wire version is cut for signals.
  **Decided 2026-11-12:** no signed anti-entropy entries for now; the limit for presence-bearing records stays stated. Revisited if a wire version is cut for signals.
- **D9 · Does a catalogue revocation stop a running install?** (CR7.) *Recommendation:* no in CR7 — refuse new
  installs and count the running ones — and decide stopping separately, since it is an availability decision an
  operator may not want made by a publisher's key.
  **Decided 2026-11-12:** a catalogue revocation refuses new installs and counts the running ones; it does not stop a running install.
- **D10 · The two one-line summaries of I1 and I5** (P3). The CHANGELOG's 2.40.0 lead and `CLAUDE.md`'s release
  paragraph say "checked" and "verified" without the condition the notes below them state. *Recommendation:* add
  *where configured* to both at the next release edit; not worth a release note of its own.
  **Decided 2026-11-12:** *where configured* is added to both one-line summaries **at the next release edit** — not before; neither the CHANGELOG's 2.40.0 lead nor `CLAUDE.md`'s release paragraph is edited until then.
- **D11 · Where this plan sits relative to wave 1b.** Wave 1b (I4, I2, I7) is agreed and next, and I4 and I2 each
  add security-bearing records. Only CR1 and the rule are agreed as *now*; the order of everything else is proposed. *Options:* (a) wave 1b
  first, this plan after; (b) CR1 now (in hand); CR2's first PR, CR13's counter and CR12 inside wave 1b; the rest
  after it; (c) this plan first. *Recommendation:* (b). CR1 is already being built; the use marker is an open
  contradiction of a claim on `what-is-proven.md` and should not wait a wave; CR13's first step is a counter;
  CR12 belongs in the release where I7 already breaks exhaustive literals (D3). I4 and I2 are then built to the
  rule from the start — CR5f is written with I2's row, and I4's collector learns of CR2's markers — and CR3–CR11
  follow wave 1b, with CR10 (the model) startable in parallel because it touches no shipped code.
  **Decided 2026-11-12:** (b), interleave. CR1 now (PR #774, in review). Inside wave 1b: CR2's first PR (the use marker), CR13's counter, CR12. CR3–CR11 and the rest after wave 1b; CR10 startable in parallel. Recorded in `identity-and-lifecycle.md` rev 0.19.
- **D12 · A concurrent-admission claim for a single-use grant?** (CR2's *Does not*.) Whether one grant admitted at
  two gateways at once is to be prevented (a consensus claim per use) or stated. *Recommendation:* state it, and
  offer the `FleetConsensus` scope for the grants an adopter marks as needing it.
  **Decided 2026-11-12:** concurrent admission of a single-use grant is stated, not prevented; the `FleetConsensus` scope is offered for the grants an adopter marks as needing it.
  *That offer has no row yet; written when taken up.*
- **D13 · Two choices inside CR7.** (i) Whether, and after how many releases, a v1 catalogue line stops
  verifying. (ii) Whether the library's `Provisioner` with no trusted publisher installs nothing by default — a
  change of a library default for anyone who builds one in code. *Recommendation:* (i) one release of both,
  counted, then refuse, as the project's other counters do; (ii) yes, in the same release as (i), with an
  upgrade note — the stem already refuses, and the library type should not be the weaker door.
  **Decided 2026-11-12:** (i) a v1 catalogue line is refused after one release in which both forms verify, counted; (ii) the library `Provisioner` with no trusted publisher refuses unsigned lines by default, in the same release, with an upgrade note.
- **D14 · A new revision of `secure-single-domain` that requires the lease margin?** (CR11.) A profile revision
  makes `start()` refuse on nodes that pass today. *Recommendation:* yes, in the revision that next changes for
  another reason, not a revision of its own; until then the guarantee reports `NotConfigured`.
  **Decided 2026-11-12:** the lease margin's guarantee joins the next revision of `secure-single-domain` that changes for another reason; no revision of its own.
- **D15 · A library-provided fenced write?** (CR11's *Does not*; L14.) A new verb with a token compared where the
  value is decided, a gateway route and both SDKs — or leave fencing as the application's contract, as stated.
  *Recommendation:* leave it; an example already shows the pattern, and a fenced verb is only sound if the
  comparison is part of the decision, which makes it a consensus feature, not a helper.
  **Decided 2026-11-12:** no library-provided fenced write. Fencing stays the application's contract.
- **D16 · May Layer I refuse an entry for its stamp?** (CR13.) The argument that it may: it is not a
  *higher-layer* law — it reads no prefix, no owner and no upper-layer meaning; the drift bound is Layer I's own
  contract; `hlc.rs:81`–`83` anticipates it; the size gate and the TLS unsigned-frame refusal are precedents. The
  argument that it may not, without saying so: it is **prevention at Layer I**, and the first refusal on the
  merge path that depends on receiver-local state — the receiver's clock — so the store stops being a pure
  function of the updates received, and two correct nodes can hold different stores for good. That reads against
  the hot invariant's title (*detection, not prevention*), whatever its body says about higher-layer laws.
  *Options:* (a) the counter only; (b) the counter, then the refusal, with a recorded carve-out in
  [runtime-invariants](../wiki/dev/architecture/runtime-invariants.md) beside the two that exist for forwarding;
  (c) neither, and the limit stated. *Recommendation:* (b), the refusal conditional on what the counter shows
  and on the slow-clock case having an acceptable answer — and default-on only after a release of counting,
  which is itself part of this decision.
  **Decided 2026-11-12:** (b). The counter first; the refusal default-on only after a release of counting, with a recorded carve-out in runtime-invariants, and conditional on what the counter shows and on the slow-clock case having an acceptable answer.
  **Added 2026-11-13, at the maintainer's request, not a new decision:** step 2 has a second candidate shape,
  written in CR13 — *local non-admission with forwarding unchanged*, the boundary's principle applied to a KV
  entry — which may need no carve-out and contains a poisoned entry less well. It is weighed against the refusal
  when step 2 is written.

## 6 · Where the critique is wrong or overstated, and where it is understated

Said plainly, with the code. Three lists: what does not hold as written, what holds and needed context, and one position.

### 6.1 · Wrong or overstated

- **"Almost every guarantee above Layer II … fails against one compromised or buggy mesh member."** Overstated,
  and the reviewer **narrowed** it in the reply ("should read 'the guarantees whose state is an unsigned KV record
  fail against one member'"). That narrower list is the inventory's.
- **"Make self-install opt-in, not the headline default."** Opt-in in code (P5): a stem needs a `[hosts]` table
  and a trusted publisher, or an explicit, warned waiver. The reviewer is right that it is the headline — the hero
  sentence leads with it and the README's path has the stem fleet as step 3 — and right in one place the critique
  did not name: the library `Provisioner` accepts unsigned lines until told otherwise (CR7, D13).
- **"`TaskCtx` … the cause of 56 `too_many_arguments` allows."** Not shown. Sixteen of the 57 are outside the root
  crate (N13); whether grouping the context removes the rest is CR21's measurement, not a finding.
- **"The upgrade notes should say so."** They do (P3). The absolute wording was in the project's first response
  and in two one-line summaries (D10).
- **"15–20% of comments narrate review history."** About 8% by a marker grep, 15% only if every plan-row tag
  counts (N17).
- **"Everything after authentication is by convention."** The predicate and the decision are each one function,
  and the scope layer is central and deny-by-default; what is by hand is the call (L18). The finding stands and
  CR6 answers it.

### 6.2 · True, and understated or needing context

- **"A u64::MAX stamp pins a key forever (`connection.rs:843-868`)."** True, and understated: it is not specific
  to anti-entropy, a fix there alone leaves the live frames, and above Layer I one such write de-advertises a
  capability for good (L2).
- **"Any member can roll the fleet back."** True by composition: a member who sheds a band (A3) or waits for a
  re-heal gets its replayed line chosen. Narrower only in that no single write swaps a running install (L15).
- **"RPC replies are forgeable" and "readable by every relay."** True, and the fan-out cap makes relays the
  common path above about nine nodes (L7, L8). The context: a relayed mandate is readable, not usable.
- **"Neither needs a wire change"** of the two Layer I items. True of the wire. The checksum is an on-disk format
  change against a pinned fixture with no version byte (CR14), and the drift refusal is a question about what
  Layer I may do (D16); neither is cheap in the sense the reply means.
- **"Nothing checks the fencing token."** True; by design and stated — the fence is at the resource (L14, D15).
- **"SWIM is unauthenticated UDP even under TLS."** True, and understated: with the default quorum it reaches
  consensus, not only liveness (L5, D7).
- **"11 clippy invocations."** Fifteen in `make check` and 37 in CI; the critique understates its own point (N2).

### 6.3 · A position, not a refutation

- **"Signals and Boundaries is no longer what ships."** The reviewer's narrower point — that the document claimed
  more than the code did, for Individual routing and for Group sampling (`src/agent/tasks.rs:368`–`392`,
  `:396` on) — **was accepted**: #771 changed the document. What the maintainer does not accept is that the
  principle changed (M2): unconditional means not conditional on admission or on a route, and a direct send is an
  optimisation over a flood that remains the fallback. Both are recorded.

## 7 · What was not verified

- **Nothing was built or run.** Every *confirmed* is a reading. In particular the 3\|2 double commit (L11), the
  skew double grant (L13), resurrection after 50 minutes (L3) and a pinned key (L2) are traced through the code
  and not observed. CR9, CR11 and CR13's exits make three of them tests; resurrection is stated
  (CR17), with no test proposed.
- **The reviewer's full per-layer document** has not been received; a finding it contains and the summary omits
  is not here.
- **Whether `fastrand`'s nonce is predictable** to a member that did not see a request (L7, L10).
- **Whether WAL replay re-seeds a value whose tombstone was collected** on the same node (L3): inferred from
  "replay is LWW", not traced.
- **Whether the group-scope roster drops a dead member** as the cluster-scope peer view does (L11): the cluster
  path was traced.
- **Whether any code replaces a running install** when a catalogue line changes (L15): none found; the
  provisioner was not read end to end.
- **Whether a relay can reuse an exchanged token** it reads from an envelope (L8): the binding is DPoP by design
  (#757); not re-derived.
- **"Several are stale"** of the comments (N17), and **"fixed"** of the sleeps (N15): a grep does not show either.
- **The critique's "Sound" column** — what it says each layer does well — was not verified at all.
- **Whether a forged SWIM `Dead` persists against a live TCP peer**, and so whether the quorum composition of L5
  with L11 needs one datagram or a stream of them.
- **The project's first response** is not a document in the repository; its six items are listed in the
  vendored file's header from the maintainer's account, and the reply's "item 1", "item 2" and "item 5" are read
  against that list.
- **Whether a docs gate checks `what-is-proven.md` anchors**: the links added here were checked by grep.
- **The inventory's own citations** are #772's, at `7d4c86a6`; they were not re-verified here.

## 8 · Housekeeping in #773, and in the decisions PR

- The critique vendored under `external/`, its own text unchanged.
- `what-is-proven.md` § *Not yet shown*: four rows dated 2026-11-12 for confirmed gaps that bound a claim on that
  page and were not listed — the WAL checksum, the lease's skew bound, the missing model, and catalogue currency
  with staging before provenance. #772's seven rows are not repeated.
- `docs/plans/README.md` and one line under `CLAUDE.md`'s *Open elsewhere*.
- The vendored file's header carries, marked as the project's and not the reviewer's, the six-item list the
  reply refers to; the critique's own text is unchanged.
- The wiki: a `.log` entry, and a pointer from `owner-signed-records.md` to this plan as where its rows are
  grouped (CR2–CR5, CR7).
- No release note corrected (P3). No code changed.
- **The decisions PR (rev 0.3):** §5 marked decided, each row's State line, `identity-and-lifecycle.md` rev 0.19
  (wave 1b also carries CR2's first PR, CR13's counter and CR12; notes under I4 and I2), the index, `CLAUDE.md`'s
  two lines, and a wiki `.log` entry. D10's two summaries are not edited until the next release edit.
