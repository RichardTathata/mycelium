# Realignment repairs — twelve findings, four pieces

**Status:** adopted 2026-10-04, rev 1.0 — nothing delivered yet; §5 tracks delivery. Source baseline: `main` at
`5e4dd12b` (v2.22.0 plus #515, #516) and the private companion at `abf406c` (pinned to v2.22.0). The private half
of this plan is `docs/plans/realignment-exporter.md` in the private repository; §3.3 here says only what it covers
and what the public side owes it.

## 1 · Why one plan, and why four pieces

An external review of 2026-10-04 reported twelve findings, three P1 and nine P2, with a probe for each. An
implementation plan accompanied it that bundled the twelve repairs with an architectural programme: a config
split, a governed-fleet profile, a signed handover manifest, a ten-step composed scenario, a research experiment,
and four new catalogues. Both documents were verified here against the code on the same day, by reading and by
re-running six of the probes in scratch worktrees. The verdicts are in §2. In short: eleven findings hold as stated
or worse, one (F01) is narrower than reported, and three things the review did not find are larger than what it did.

The repairs do not need the programme, and the programme would delay the repairs. So this plan has **four pieces
that do not block each other**, in the order they should ship:

| Piece | Scope | Ships as |
|---|---|---|
| **1 · Security and durability patch** (§3.1) | F01, F02, F03 and its two siblings, F12; witnesses, runbook corrections | `v2.22.1` |
| **2 · SDK and integration release** (§3.2) | F06, F07, F08, F09, F10; the live suite in CI | `mycelium-ts 0.2.0`, `langgraph-checkpoint-mycelium 0.2.0`, one additive gateway field in `v2.23.0` |
| **3 · Private export durability** (§3.3) | F04, F05, F11 | the private plan, on its own line |
| **4 · Architecture, decided small** (§3.4) | the three decisions in §4, strict eligibility, the field table | `v2.23.0` plus documentation |

Piece 1 has no dependency on anything. Piece 2 depends on nothing in piece 1. Piece 3 depends on the journal
repair in piece 1 only through the substrate pin. Piece 4 lands after the others and blocks none of them.

Every repair follows the repository's standing rule: **the regression test is written first and seen to fail on the
unfixed code**, and the commit says so. The review's probes are the templates; each is ported into the proper
suite with its expectation inverted.

## 2 · What the review found, verified by `file:line` (2026-10-04)

**F01 — WAL torn tail (narrower than reported, P2; P1 for a direct embedder).** `replay`
(`mycelium-core/src/persistence.rs:336`) applies the good prefix on `WalEnd::Torn` and logs; `open_wal` (`:568`)
opens `create+append` and nothing truncates. `decode_wal_records` (`:356-370`) loops `while pos + 4 <= len`, so a
trailing 1–3-byte length prefix returns `Clean`, not `Torn`. **What the review missed:** `GossipAgent::start`
(`src/agent/lifecycle.rs:235-250`) spawns the writer and awaits `trigger_snapshot()` before the handle is
installed, and `do_snapshot` (`persistence.rs:694`, `set_len(0)` at `:831`) truncates the WAL on `Torn` or
`Clean`. So the node binary repairs the tail before any acknowledged append, encrypted or not. **The residual
holes:** (a) line 249 discards the snapshot's result with `let _ =`, so a failed startup snapshot (ENOSPC, a failed
directory fsync) starts the node behind the torn tail; (b) once appends outgrow the torn record's claimed length
its bytes absorb the new ones, which decode as `Corrupt`, after which every later snapshot aborts and the WAL grows
unbounded until the next restart refuses to start (`on_unreadable = refuse`); (c) `replay` and `spawn_wal_writer`
are `pub`, and an embedder composing them without the lifecycle gets the review's scenario exactly. No test pins
the startup repair; the wiki's persistence section (`runtime-invariants.md` §Persistence) does not state it.

**F02 — journal torn tail (confirmed, P1, broader than reported).** `Journal::open` (`src/agent/journal.rs:125`)
opens `create+append`; `count_records` (`:231`) stops at a torn frame and never truncates it; the writer closure
writes length and payload as two `write_all` calls, replies `Err` on failure and keeps looping. The v2.10.0 fix
covered **counting only** (`a_torn_tail_is_not_counted_as_a_record`, `:660`), and
`a_truncated_tail_does_not_stop_the_node_from_starting` (`:610`) **pins the bug**: it appends after a torn frame
and asserts only that the append returned `Ok`. A probe (good record, torn frame claiming 9 bytes with 2 present,
then two appends) received `seq=1 OnDisk` and `seq=2`, and read back `["one", "hi\x05\0\0\0aft"]`: a garbled
record built from the torn frame plus the new one. **Every consumer that folds its journal on open then fails to
start:** `EvidenceJournal` (`src/agent/evidence_journal.rs:107`), `RightsLedger` (`src/control/ledger.rs:356`), the
knowledge durable heads and records (`src/knowledge/durable.rs:158, :274`), and `DurableEpochs`
(`src/mandate/authority.rs:561`). While the claimed length still exceeds the appended bytes the new records are
silently invisible, so an epoch floor recorded after a crash can vanish on the next restart: the "a restart no
longer restores revoked authority" property of v2.15.0 rides on this journal.

**F03 — egress (confirmed, P1, and two siblings that need no redirect).** `connect_mcp_server`
(`src/agent/mcp_handle.rs:162-167`) checks `permits_url` once on the initial URL, then builds
`reqwest::Client::new()`: reqwest's default follows ten redirects and re-sends a 307/308 body. The same client is
moved into the per-call proxy. **Sibling 1, the parser:** `host_of_url` (`mycelium-core/src/config.rs:318-334`)
splits the authority on `/?#` and takes what follows the last `@`; the WHATWG parser reqwest uses treats `\` as
`/`. So `http://evil.example\@allowed.example/mcp` is gated as `allowed.example` and dialled as `evil.example`;
`http://evil.example\.allowed.example/` passes a suffix entry `.allowed.example` the same way. Reproduced. This
affects every caller of `permits_url` that hands the same string to reqwest. **Sibling 2, the object store:**
`ObjectStoreFetcher` (`mycelium-wasm-host/src/object_store_source.rs:57-63`) gates `permits_url("s3://bucket/…")`,
i.e. on the bucket name, never on the endpoint (`*.amazonaws.com`, `AWS_ENDPOINT`, `storage.googleapis.com`) or
the credential fetches. **The inventory of every outbound client** (production code only):

| Client | `file:line` | Gated on | Follows redirects |
|---|---|---|---|
| MCP bridge (connect + proxy) | `src/agent/mcp_handle.rs:167` → `mcp.rs:235` | initial URL, once | yes, unchecked |
| `OpenAiBackend` (LLM, `[[serve]]`) | `src/agent/llm.rs:87` (gate `:282`) | initial URL, per call | yes; prompt body re-sent on 307/308 |
| SkillRunner LLM | `src/bin/skillrunner/main.rs:132` (gate `runner.rs:122`) | initial URL | yes |
| capability probes | `src/capability_config.rs:907` (gate `:826`) | initial URL | yes (GET) |
| OIDC discovery + JWKS | `src/agent/oidc.rs:148` (gates `:204`, `:230`) | each URL | yes — a redirected JWKS trusts keys from a denied host |
| federation client | `src/federation/client.rs:247-264` (gate `:450`, `:637`) | `base_url` | yes; the `x-mycelium-federation-call` credential header is **not** stripped cross-host |
| bulk peer fetch | `src/agent/bulk.rs:82` (URL from the sender's IP, `:264`) | not gated, by design | yes — a peer can redirect the fetch anywhere |
| `HttpLibrarySource` | `mycelium-wasm-host/src/http_source.rs:158` (gate `:174`) | only via `.with_egress` | yes |
| `ObjectStoreFetcher` | `object_store_source.rs:57-63` | the bucket name | yes (object_store leaves the default) |
| `OllamaProbe` | `mycelium-reason/src/ollama.rs:72` | not gated | yes |
| `GitMirror` push | `mycelium-wiki/src/sink.rs:141, :293` | remote host | git's `http.followRedirects=initial` |

The guarantee catalogue (`docs/reference/guarantee-catalogue.md:27`) says `egress.allow_list` restricts "the
substrate's own outbound calls … (MCP bridge, LLM, probes, skillrunner, the wasm host, the wiki sink, the
federation client, OIDC)"; `docs/threat-model.md:89` says it gates "every outbound HTTP path"; the same section at
`:94-96` still lists JWKS as ungated, which has been false since 2026-10-03. The policy is purely name-based: it
never resolves, so an allowed name that resolves to a denied or link-local address is not covered either.

**F04, F05 — AE export (confirmed, and worse).** Private `mycelium-ae/src/export.rs`: `journal_cursor()` (`:428`)
returns the read position and `resume_from` (`:433`) restores only that; `ingest_journal` advances it at `:477`
while `pending`, the `awaiting` held permits, `flushed` and `issued` live in memory. The `State.journal` field doc
tells the operator to persist exactly that value. `batch_id` (`:703`) hashes the record ids; `seal` (`:666`) signs a
body that also carries `cursor.previous` (`:621`), assigned only in `take_batch` (`:636`) and never restorable.
Two probes: a sink that ingested a permit and a deny, crashed before `take_batch`, and resumed from its persisted
cursor produced nothing — both records gone; a sink that emitted A and B, lost B's ack and resumed re-emitted B
with the same id, `previous=None`, a different body, and the conformance `StubConsumer` refused
`ReusedIdDifferentBody`. Because `st.cursor` cannot be restored, **the first batch after any restart is refused
either way** (new id → cursor mismatch; old id → different body). The v2.7.0 `at_ms` fix was this class once.

**F06–F09 — the TypeScript SDK (confirmed; Rust is the reference).** The gateway emits HLCs as JSON numbers
(`src/agent/http.rs:3575, :3602, :3677, :3814`) and nonces too (`:2105`); an HLC is `(ms << 16) | logical`
(`mycelium-core/src/hlc.rs:99, :118`), today ≈ 1.17e17, 57 bits; the SDK does `resp.json()` then `BigInt(data.hlc)`
(`mycelium-ts/src/agent.ts:629, :646, :668, :680`, nonce `:327`), so two appends in one millisecond read as one HLC
and a nonce `0xDEADBEEF01020304` comes back ending `…14752` instead of `…15524`. The fencing token already goes
out as a decimal string (`http.rs:3395`), which is the precedent the SDK gets right (`agent.ts:570`). `get()` checks
`value_b64 !== null` (`:337`) while the gateway sends `{"found": false}` with no field (`http.rs:2636`), so an
absent key throws; `resolveCapability` returns `{providers}` as if it were an array (`:272-275` vs `http.rs:2008`);
`emit` reads `queued` where the gateway sends `ok` (`:311` vs `:2084`); `emitReliable` reads `status` where the
gateway sends `ack` (`:700` vs `:3875`); `scanLog` sends `from_hlc`/`to_hlc` against `LogScanQuery { from, to }`
(`:640` vs `http.rs:3579`) and maps `data.entries` over a bare array (`:645` vs `:3605`); `compactLog` sends a
string to `before_hlc: u64` (`:655` vs `:3609`); `subscribeLog` sends `since_hlc` against `since` (`:664` vs
`:3639`), so every resume replays from zero. The SSE data has no `kind` field (`http.rs:2102-2106`; the kind is
the SSE event name), so `raw.kind` (`agent.ts:324`) is always undefined. `sseStream` (`mycelium-ts/src/sse.ts:17,
:46-72`) has no `AbortController`, no cancel path for its detached read loop, no `try/finally`, and an unbounded
`pending` array; `a2a.ts:320-344` shows the cleanup that is missing. **Run live, the shipped
`mycelium-ts/tests/gateway.test.ts` fails 7 of its own assertions on these**, and it has never run in CI: it skips
itself without `MYCELIUM_TEST_HOST`, which `.github/workflows/ci.yml:585-605` never sets. `mycelium-py` gets every
one of these routes right (`agent.py:520, :552, :578, :658, :1036-1055, :1070, :1123`).

**F10 — LangGraph pending writes (confirmed).** `saver.py:322-326` (sync) and `:581-585` (async) `continue` past a
pending write whose blob is unavailable and return a tuple with fewer `pending_writes`; a missing skeleton or
channel blob instead returns `None` (`:310-318`, `:568-576`), which LangGraph reads as *no checkpoint*, so the
"stricter" path is its own hazard. No docstring or README mentions either.

**F11 — RA retention (confirmed).** Private `mycelium-ra/src/outbox.rs:404` sets `meta.earliest = before`
unconditionally and persists it; a lower threshold after an earlier prune hides a `RetentionGap` across reopen.

**F12 — journal ownership (confirmed).** No `flock`, `fs2` or `fd-lock` anywhere in `src`, `mycelium-core/src` or
any manifest; the only lock file is the CA's (`mycelium-core/src/tls.rs:147`). Two `Journal`s on one path each
issued `seq 0`. The two-call frame write also lets two writers interleave a length with another's payload. The KV
WAL has the same gap for two agents sharing a `base_path`/node id.

## 3 · The four pieces

### 3.1 · Piece 1 — the security and durability patch (`v2.22.1`)

Six increments, each a PR with its fail-first witness, a changelog line and the documentation it touches. Wire
**v12** unchanged; no public API change beyond one new error variant.

**R1 · The journal truncates its torn tail and owns its file (F02, F12).** `Journal::open` returns the valid end
offset beside the count; a torn suffix (partial header or partial payload) is truncated to that offset and the
file and directory synced **before** the writer is spawned; corruption inside the log (a frame that decodes to the
wrong hash before the tail) is refused by name, never skipped. Each frame is written as **one** buffer. An
exclusive non-blocking OS lock (`std::fs::File::try_lock` on a `lock` file beside the journal — the pattern the
private RA outbox already uses, no new crate; `rust-version` goes from 1.88 to 1.89, CI runs 1.96) is taken before
counting and held by the writer for its lifetime; a second owner, same process or
another, gets `io::ErrorKind::WouldBlock` with the path in the message. A failed append **stops the writer**: the
reply is `Err`, every later append is `Err` with a named `JournalPoisoned` reason, and the next open repairs. The
witness: the review's probe, inverted — good record, torn frame, two appends, reopen, both records readable at
`seq 1` and `seq 2`; a second open on the same path refused; a second process refused (`std::process::Command` on
the test binary). **Invert** `a_truncated_tail_does_not_stop_the_node_from_starting` to read the record back. Run
each consumer's own open on a repaired file: ledger, durable heads, `DurableEpochs` (an epoch floor appended after
a torn tail survives a restart — the v2.15.0 property, re-pinned).

**R2 · The WAL's startup repair is pinned, its failure refuses, a partial header is torn (F01).** Line 249 stops
discarding: a failed startup snapshot is a refusal at `start()` in the v2.20.0 class ("a node used to start
degraded"), with the error named, unless `persistence.on_unreadable = "quarantine"` is set, in which case the
WAL is moved aside like the other unreadable cases. `decode_wal_records` reports a trailing 1–3-byte prefix as
`Torn`. A test pins the lifecycle: torn WAL on disk, `start()`, append with `set_with_receipt`, restart, record
present — and the same with the snapshot made to fail (a read-only directory) asserting the refusal. The wiki's
persistence section gains the fourth invariant: *the startup snapshot repairs the WAL tail before the first
acknowledged append; a direct embedder of `replay` + `spawn_wal_writer` must call `trigger_snapshot()` first*,
and the rustdoc on both `pub fn`s says the same. The WAL takes the same file lock as R1 (one agent per
`base_path`/node id).

**R3 · One egress-gated client builder; no redirects by default (F03).** A new `src/agent/egress.rs` in `mycelium`
(reqwest is already its dependency; nothing is added to core): `EgressClient::build(policy, ...)` returns a
`reqwest::Client` with `redirect::Policy::none()`, timeouts, and — where a caller declares it needs redirects —
`redirect::Policy::custom` that re-runs `permits_host` on every hop, caps hops at 5, and refuses an HTTPS → HTTP
downgrade. Every row of the §2 table in this crate moves onto it: MCP, LLM, SkillRunner, probes, OIDC, federation.
The wasm host and `mycelium-reason` each take the same contract through their own builder (companions do not
depend on `mycelium` for a helper); the object-store fetcher gates on the **endpoint host** it will dial, derived
from the store's resolved configuration, not the bucket. The bulk peer fetch stays ungated but gets
`Policy::none()` (a peer's redirect is not a peer). The witness is the review's listener: an allowed endpoint
answering 301, 302, 303, 307, 308 and a two-hop chain to a denied listener, which must receive **no connection**
(the listener's own accept counter, planted first with a direct call). A second witness per client: the same
listener behind the LLM backend, the OIDC JWKS URL and the federation client, the last asserting the credential
header never arrives at the denied host.

**R4 · The gate parses the URL the way the client does (F03 sibling 1).** `host_of_url` is replaced by a parse
with the `url` crate (reqwest's own parser; added to core as a dependency if `cargo tree` shows it is not already
there) and `permits_url` gates on `host_str()`; a URL the parser rejects is denied under a non-empty allow-list, as
today. The witness: the two reproduced strings, plus the `url` crate's own test vectors for authority edge cases,
asserting the gate's host equals reqwest's dialled host for each. The threat model and the guarantee catalogue
row say **hostname allow-list, no resolution**, and name the DNS gap as not covered.

**R5 · Documentation and runbooks for the patch.** `docs/threat-model.md:89-96` restated (JWKS gated since
2026-10-03; redirects now refused; the parser rule; DNS not covered); the guarantee catalogue row 27 gains the
redirect and parser sentences; `docs/operations/crown-jewel.md:84-96` adds the `OllamaProbe`, object-store
endpoint and bulk rows; the persistence invariant page and the recovery runbook gain the R1/R2 repair reports and
the exact log lines an operator sees; `docs/wiki/dev/.log/2026-09-16-ae-journal-reader-seam.md:27-29`'s reasoning
("a later read picks it up") is corrected on the page it fed. `examples/receipt_ladder.rs` gains a
*recover → append → recover* branch that prints the receipt's rung and the re-read.

**R6 · Release `v2.22.1`.** Changelog in the v2.18.1 form: what was accepted and ignored, what now refuses by
name, and the **check before upgrading** — a node whose startup snapshot was failing silently now refuses to
start; a deployment relying on a redirecting MCP, LLM or OIDC endpoint now gets a named egress refusal and must
either allow the target or declare redirects for that client. Follows `RELEASING.md` including step 2b.

### 3.2 · Piece 2 — the SDK and integration release

**S1 · Lossless integers in the TypeScript SDK, wire unchanged (F06).** Decision D1 (§4): the wire stays v12 and
Python is untouched. The SDK gains one internal decoder, `parseLossless(text)`: a bounded scanner that quotes
every bare integer literal outside strings before `JSON.parse`, after which the SDK's existing `BigInt(...)` calls
are exact. Applied to every HTTP body and every SSE `data:` line. No dependency, works on the CI floor of Node 20
(the reviver's `context.source` would need 21). Request bodies carrying a 64-bit value (`before_hlc`) are
serialised with the integer spliced in as text. Query parameters are already text. The witness is the review's
probe: values `2^53 ± 1`, `ms << 16 | 1` and `| 2` in one millisecond, and the nonce vector, round-tripped through
`appendLog`, `scanLog`, `subscribeLog` and `subscribe`.

**S2 · The shapes (F07, F08).** `get()` keys on `found`; `resolveCapability` returns `data.providers`; `emit`
returns `data.ok` and its doc says what that means (the gateway accepted the signal for local delivery and
propagation; not that any subscriber ran); `emitReliable` maps `ack: "acknowledged" | "timeout"` to its documented
result and a non-2xx to a thrown `GatewayError`, and `timeoutSecs` is documented as whole seconds and rounded up,
matching `Option<u64>` on the Rust side (the live test's `0.3` is corrected as test setup, separately);
`scanLog`/`subscribeLog` send `from`/`to`/`since`; `scanLog` maps the bare array; the signal subscription reads
the kind from the SSE event name. The additive gateway change: the SSE data object also carries `"kind"` so an
SDK that reads the body alone is not wrong (`v2.23.0`, wire unchanged). Each shape has a live assertion.

**S3 · `sseStream` has a lifetime (F09).** An `AbortController` shared by `fetch` and the reader; `try/finally`
on the generator that cancels the reader, aborts the fetch and releases the lock on `break`, `return()` and
throw; the read loop reads on demand rather than pumping, so the buffer is bounded by what the consumer has not
yet taken; a `maxPending` option (default 1024) after which the stream **throws** a named overflow — never a
silent drop of an ordered event. The witness: the review's `ReadableStream` probe asserting the source's cancel
hook ran; a slow-consumer test asserting the throw at the bound; the A2A stream's existing cleanup shared rather
than duplicated.

**S4 · The live suite runs in CI.** The `sdk-ts` job builds `target/debug/mycelium` with the features the suite
needs (consensus included, which the scratch run lacked), starts it on an allocated port, sets
`MYCELIUM_TEST_HOST`/`PORT`, and runs `jest`; the three stale expectations (`consistentSet` and `electLeader`
needing the feature, `rpcCall`'s `412 provider_without_caller_context`) are fixed as test setup in a separate
commit so the product assertions stay legible. The same for `mycelium-py`'s `test_gateway.py`, which CI also
never runs. CLAUDE.md's "CI also gates `tsc --noEmit`" line is corrected to name `jest` and the live suite.

**S5 · An incomplete checkpoint is an error, not a shorter tuple (F10).** Decision D5: both `_read_tuple` and
`_aread_tuple` raise `IncompleteCheckpoint(checkpoint_id, missing=[...])`, a named retriable error, when any
referenced pending-write blob is unavailable — and the same for a missing channel blob of an explicitly requested
checkpoint id, instead of returning `None`. `get_tuple()` with no id (latest) keeps returning `None` only when
there is no checkpoint at all. Absence, temporary unavailability, authorization refusal and corrupt content stay
distinguishable in the error. The witness: the review's probe inverted (remove one pending-write blob, assert the
raise; restore it, assert the completed task's write is present) and a two-node test where the metadata row
gossips in before the blob can be fetched. The README's consistency note and guide 15 say it; the LangGraph
ladder's recovery step shows the pause.

Releases: `mycelium-ts 0.2.0` (the return types of `get`, `emit`, `emitReliable`, `resolveCapability` and the
log verbs change in practice, so a minor), `langgraph-checkpoint-mycelium 0.2.0` (callers now see an exception),
each with a README migration note; the gateway's `kind` field and the CI change in `v2.23.0`.

### 3.3 · Piece 3 — private export durability (the private plan)

Covered in the private repository's `docs/plans/realignment-exporter.md`: the exporter's four durable positions
(journal read position · materialised pending/held records · sealed issued envelopes · consumer-acknowledged
progress), byte-identical retry from the persisted envelope, the export chain restored on open, a typed
`ExportCheckpoint` replacing `journal_cursor()` as the thing an operator persists, the migration that **never
fabricates a cursor**, and the RA watermark made monotonic (F11). What the public side owes it: R1's journal
repair (the exporter's own journal is a `Journal`), and a released tag to pin, which is `v2.22.1`.

### 3.4 · Piece 4 — architecture, decided small (`v2.23.0` and documentation)

**A1 · "No control plane", defined (D2).** The positioning sentence stays. The visitor's expansion in
`docs/positioning.md`, guide 00 and the top of `docs/guide/agentic-control-plane.md` gain one definition, in these
words: *Control decisions live in participating nodes: enabled governors and provisioners act locally, subject to
configured consensus and resource-side authority checks. No separate Mycelium control-plane service is required.*
The three things called "profile" — the startup profile (`secure-single-domain`), the control-profile ladder
(`Observe` … `EnforceAllocated`) and per-operation consensus policy — are named apart in `control-profiles.md`
and guide 22, with one paragraph on how they compose and that none implies another.

**A2 · Configuration ownership, documented, the split deferred (D3).** A table on `GossipConfig`'s rustdoc and on
the wiki's configuration page: for every field, the consuming component (core transport/state · gateway · control
· deployment), the feature it needs, where an unsupported setting is **refused** (the v2.18.1 start-time checks,
by name) or where it is not, whether a change needs a restart, and the default. The cross-layer hooks
(`ReplyInterceptor`, `QuorumObserver`, `SnapshotDeferHook`, decision tracing, store notifications) get one wiki
page stating, for each, the triggering event, the data passed, allowed side effects, blocking rules, registration
lifetime, and behaviour when absent — with the boundedness rule that a hook does no synchronous network I/O. No
`CoreConfig` type; the split is a breaking-release decision recorded in §4 as deferred.

**A3 · Strict eligibility from source-established coverage (D4).** Additive, beside the unchanged `eligible()`
(`src/mandate/handover.rs:291`, which keeps its stated contract). A `Coverage` statement attached to a history
source — scope, trusted starting point, continuity through an identified endpoint, availability of the required
records — produced by the source from what it can verify: `HandoverJournal` (`:105`) gains the scope, start and
endpoint it records; the signed-heads reader supplies continuity from its checkpoint onward and the availability
of what the heads reference, never earlier history. `eligible_strict(rules, source, candidate, now)` returns, per
configured rule, `Eligible | Ineligible(reason) | Unknown(what is missing)`: consecutive terms need a complete
ordered suffix, cumulative tenure needs lifetime history or a trusted baseline, cooling-off needs enough recent
coverage to rule out a relevant term; `Unknown` on any configured rule keeps `Successor::admit` (`:197`) unready.
The witness: correctly signed heads with one required record missing → `Unknown`; the record restored → a real
verdict; terms 20–30 verified → consecutive terms decided, cumulative tenure `Unknown` without a baseline.
`mycelium-wiki/examples/curator_handover.rs` shows both conditions — eligibility and acknowledged handover — with
the protected write refused until the history is present and read, and the resource's own authority check kept.
Guide 21 and the handover design record say who establishes coverage (the source) and who decides sufficiency
(the evaluator).

## 4 · Decisions (the register)

| # | Decision | Chosen | Why | Not chosen |
|---|---|---|---|---|
| D1 | 64-bit values on the wire | **unchanged**: numbers stay numbers, the fencing token stays a string; the TypeScript SDK parses losslessly | one parser is the defect; Python and every curl consumer are correct today; no compatibility window | decimal strings as canonical (breaks Python and the shipped contract for one SDK's bug) |
| D2 | "no control plane" | **kept**, with the definition in A1 | true for a library with no daemon or service to operate; the identity sentence is gated and on every door | "distributed embedded control without a required central controller" |
| D3 | A `CoreConfig` split | **deferred** to a planned breaking release; A2's table now | a core type cannot alias a type in `mycelium`, so the split is two types and an adapter until a removal; no defect depends on it | splitting now |
| D4 | Strict eligibility | **additive**, coverage established by the history source, sufficiency decided per rule by the evaluator, `Unknown` is not `Eligible` | a caller flag relocates the assumption; a range alone does not establish scope, start, continuity or availability | a governed-fleet profile label; a signed handover manifest format |
| D5 | An incomplete LangGraph checkpoint | **raises** a named retriable error | `None` means *no checkpoint* to LangGraph and restarts the thread | returning a shorter tuple; returning `None` |
| D6 | Egress redirects | **refused by default**; a client that declares it needs them gets per-hop re-checking with a hop cap and no downgrade | the allow-list gates a destination, and a redirect is a destination | following with a post-hoc check |
| D7 | Egress host parsing | **the `url` crate**, in core | the gate must read the host the client dials | fixing the hand-rolled parser's `\` case alone |
| D8 | Lossless JSON in TypeScript | **an internal scanner** that quotes bare integers before `JSON.parse` | zero dependencies, Node 20 | `JSON.parse` reviver with `context.source` (Node 21+); `json-bigint` |
| D9 | Journal and WAL file lock | **`std::fs::File::try_lock`** on a sidecar `lock` file; `rust-version` 1.88 → 1.89 | no new dependency; the private outbox already does exactly this; one decision for WAL and journal | `fd-lock`; an in-process registry (does not cover two processes) |
| D10 | The composed ten-step scenario | **not a gate**; per-boundary witnesses with a plant proving reach, as the v2.15.0 bypass matrix | one end-to-end run is green by accident or flaky forever | `tests/architectural_coherence.rs` as the gate |
| D11 | New catalogues | **none**; the guarantee catalogue, the example matrix and the proof page are extended in place | six catalogues exist | `claim-evidence.json`, `examples/contracts.json`, `architecture-boundaries.json`, two new canonical documents |
| D12 | Research work in this plan | **none**; the four-arm experiment stays research-track | a defect plan's completion cannot wait on an experiment | WP11 |

## 5 · Delivery

| Increment | PR | Status |
|---|---|---|
| R1 journal: truncate, one-buffer frame, lock, poisoned writer | #518 | in review |
| R2 WAL: pinned startup repair, refusal on failure, partial header | — | open |
| R3 egress client builder, every client, no redirects | — | open |
| R4 the gate parses with `url` | — | open |
| R5 patch documentation | — | open |
| R6 release `v2.22.1` | — | open |
| S1 lossless integers (TS) | — | open |
| S2 the shapes (TS, one additive gateway field) | — | open |
| S3 `sseStream` lifetime | — | open |
| S4 the live suites in CI | — | open |
| S5 incomplete checkpoint raises (py) | — | open |
| A1 "no control plane" defined; "profile" disambiguated | — | open |
| A2 configuration field table; hook contracts page | — | open |
| A3 strict eligibility; curator example | — | open |
| Piece 3 | private plan | open |

## 6 · Not claimed

- **DNS.** The allow-list is a hostname allow-list after this plan as before it; an allowed name resolving to a
  denied or link-local address is not covered. Stated in the threat model; an address-level policy is a separate
  decision.
- **Power loss.** The torn-tail witnesses use constructed crash-state bytes against the real code paths, not a
  physical power-loss test; the fault injection covers sync, rename and directory sync.
- **A second embedder.** R2 documents what a direct embedder of the WAL must do; it does not change the public
  shape of `replay` or `spawn_wal_writer`.
- **The private composition.** A public CI pass does not test the private exporter; its gates are in the private
  plan and its candidate record in `COMPATIBILITY.md`.
- **The hypotheses.** Nothing here is evidence for lower coordination effort or better recovery; those remain the
  research track's.
