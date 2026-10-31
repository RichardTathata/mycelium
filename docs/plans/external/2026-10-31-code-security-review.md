# Mycelium adversarial code security and architecture review

*Vendored external document, received 2026-10-31. Reviewed commit `d656f50cbb6e4b5f1c7d4fb1ffc022d2bfd9e241`. The
file-path links of the original are rendered here as repository-relative `file:line` references; the
`dependency-audit.json` the review says sits beside it is not vendored (its content is the four `cargo audit` warnings
the plan's §2 re-derives). The maintainer's
verification of each finding and the work it led to are in
[`../external-review-2026-10-31.md`](../external-review-2026-10-31.md).*

---

31 October 2026 · Reviewed commit `d656f50cbb6e4b5f1c7d4fb1ffc022d2bfd9e241`

## Assessment

**Mycelium has substantial security and reliability engineering, but the reviewed revision has material gaps in
authorisation, durable execution control and transport composition.** I would require the high-priority findings
below to be resolved before relying on the affected features for security-sensitive production workloads.

The recurring architectural problem is that a protection established by one subsystem is not consistently preserved
by another entry point, persistence path or transport. A scoped HTTP route can be bypassed through generic RPC; a
single-use grant can authorise work without durably recording its consumption; a replica can acknowledge persistence
before its specific record reaches the WAL.

Seven findings are reported: five P1 items requiring prompt remediation and two P2 items requiring planned
correction. Six have supporting local reproductions; the seventh is established by source inspection and the
filesystem durability contract. Priorities assume the affected feature is used and its stated preconditions hold. No
critical remote-code-execution claim is made.

The source checkout was not modified by this review. Compilation and probes ran in a separate archive of the reviewed
commit. Working-tree documentation changes appeared during the review; they were left untouched and are outside this
commit-specific assessment.

## Findings at a glance

| ID | Priority | Finding | Evidence |
| --- | --- | --- | --- |
| F1 | P1 | Generic RPC bypasses blackboard HTTP scopes | Live HTTP reproduction |
| F2 | P1 | Protected work proceeds when its grant-use record cannot be persisted | Fault injection and WAL replay |
| F3 | P1 | ReplicaSync can report persistence before the record is appended | Deterministic interleaving reproduction |
| F4 | P1 | An empty successful JWKS refresh retains withdrawn signing keys | Scripted identity-provider reproduction |
| F5 | P1 | Native HTTPS leaves unauthenticated TLS handshakes unbounded | Local connection probe and source inspection |
| F6 | P2 | Bulk transfer uses plaintext HTTP and fails with native gateway TLS | Plaintext and TLS comparison |
| F7 | P2 | A newly created journal can acknowledge durability without syncing its directory | Source inspection and filesystem contract |

## F1 Generic RPC bypasses blackboard HTTP scopes

**Preconditions:** A gateway hosts a blackboard, and a caller has `mesh:write` without `board:write`. The relevant
companion RPC kind has not been manually blocked.

The dedicated post endpoint requires `board:write`, while `/gateway/rpc/call` requires only `mesh:write`. A caller
can invoke `blackboard.<namespace>.post` through that generic endpoint. The blackboard handler accepts the operation
without independently enforcing the board scope. The built-in protected-kind list covers MCP, skills and LLM
invocation, but does not automatically cover companion operations.

**Observed:** With the same token, direct posting returned HTTP 403; generic RPC returned `ok: true`; reading the
board confirmed the inserted payload. This is a demonstrated cross-route scope bypass. It also means operators cannot
safely infer that withholding `board:write` prevents a `mesh:write` principal from writing to the board. Similar
companion entry points warrant inspection; the reproduction establishes blackboard posting specifically.

**Correction:** Authorise the semantic operation and resource at the provider, preserving the authenticated caller.
Register companion operations and their required permissions centrally so generic routes enforce them too. If
`mesh:write` is intentionally unrestricted companion authority, document that broad authority explicitly and do not
present the narrower scopes as isolation from it.

**Acceptance criterion:** The same principal is refused through dedicated HTTP, generic RPC and remote forwarding
unless it has authority for the actual board operation.

Sources: scope mapping `src/agent/http/authz.rs:296`, board scope `src/agent/http/authz.rs:372`, generic dispatch
`src/agent/http/mesh.rs:113`, provider handler `mycelium-blackboard/src/rpc.rs:240`, protected kinds
`src/agent/mod.rs:196`.

## F2 Grant consumption does not require durable persistence

**Preconditions:** Grant-based provider enforcement is used with persistence enabled, and the WAL writer fails, its
queue rejects an append, or a crash occurs before the marker becomes durable.

The provider claims a grant in an in-memory map, then writes its `used/provider` marker through `write_owned_kv`.
That helper uses `append_try`, which neither waits for durability nor reports failure to the caller. Work can proceed
after the supposedly durable single-use record has been dropped. Configuring WAL flush mode and a strict evidence
journal does not make this write await the WAL.

**Observed:** After issuing a valid grant, the probe stopped the KV WAL writer and invoked a protected deployment
tool. The call succeeded and executed once. Replaying 17 persisted records found no provider-use marker; the WAL
recorded two dropped appends.

The demonstrated defect is execution without a recoverable use marker. Reuse after restart is the resulting risk
because the in-memory consumption map is lost and the provider checks the persisted marker to remember prior use. An
actual second execution after process restart was not reproduced in this review.

**Correction:** Make grant consumption an acknowledged, durable operation before dispatching protected work. Refuse
execution if that persistence cannot be established. Define how consumption, expiry and recovery interact, including
installations without persistence.

**Acceptance criterion:** WAL closure, queue saturation and crash injection cannot yield both an executed effect and
a reusable grant after recovery.

Sources: grant-use decision `src/agent/http/authz.rs:1206`, unchecked write `src/agent/http/authz.rs:1224`, write
helper `src/agent/state_machine.rs:886`, append semantics `mycelium-core/src/persistence.rs:209`.

## F3 ReplicaSync can acknowledge a record not yet in its WAL

**Preconditions:** A persistence query runs concurrently with an incoming update between its application to memory
and its WAL enqueue.

The receive path applies the update before appending it. ReplicaSync checks the in-memory stamp and content, checks
the dropped-append counter, then synchronises the WAL. Synchronising a queue does not cover a record that has not
entered that queue. A concurrent query can therefore return `Persisted` while the receive worker is paused before
append. No dropped append is necessary.

**Observed:** The deterministic probe reproduced that legal interleaving using the real apply, answer and replay
implementations: `answer=Persisted`, matching record on disk `false`, dropped appends `0`. The probe controls the
interleaving directly; it is not a measured frequency from a live distributed race.

**Impact:** A caller can count a replica towards its durability requirement although a crash at that point would
lose the acknowledged record.

**Correction:** Bind the receipt to durability of the exact entry, using an acknowledged append or a per-entry log
position whose completion is awaited. A global flush after an independent memory check is insufficient.

**Acceptance criterion:** Pause the receive path immediately after applying an entry; a simultaneous query must not
acknowledge persistence until that entry has a durable representation.

Sources: apply before append `mycelium-core/src/connection.rs:1036`, anti-entropy path
`mycelium-core/src/connection.rs:837`, receipt decision `src/agent/replica_sync.rs:164`, sync implementation
`mycelium-core/src/persistence.rs:262`.

## F4 Successful withdrawal of all JWKS keys is ignored

**Preconditions:** OIDC has cached a valid signing key and the issuer later serves a successful empty key set, for
example while withdrawing all active keys.

The fetcher represents both failure and a valid empty result as an empty vector. The updater treats both as failure
and retains the previous keys. Known keys continue to be used while background refreshes run; there is no maximum
stale-key age in this decision path.

**Observed:** A scripted issuer first supplied a valid key, then returned HTTP 200 with `{"keys":[]}`. After the
second fetch completed, verification with the withdrawn key still succeeded.

**Impact:** Withdrawal does not take effect in the running verifier. If an old private key is compromised, accepting
that key can permit newly minted tokens with fresh expiry dates. Existing token expiry alone does not contain that
scenario.

**Correction:** Return a typed success or failure result, accept a successful empty set as authoritative, reject
unsuccessful HTTP responses, and define a bounded stale-key policy for genuine fetch failures.

**Acceptance criterion:** A completed successful empty refresh removes the previous keys; outage behaviour has a
documented maximum stale period.

Sources: refresh update `src/agent/oidc.rs:354`, fetch result `src/agent/oidc.rs:377`, cache decision
`src/agent/oidc.rs:203`. The [JWK Set specification](https://www.rfc-editor.org/rfc/rfc7517.html#section-5) defines
the keys array; the availability-versus-revocation policy remains an application design responsibility.

## F5 Native HTTPS has no handshake admission bound or timeout

**Preconditions:** The native HTTPS gateway is reachable by an untrusted client without an upstream connection
limiter.

Each accepted TCP socket spawns a task that awaits the TLS handshake without a timeout or admission semaphore.
Authentication is reached only afterwards. The gossip transport's connection limit and handshake timeout do not cover
this gateway accept loop.

**Observed:** All 12 deliberately silent connections remained open beyond 750 ms, despite a configured gossip
connection cap of two and handshake timeout of 100 ms. This bounded probe demonstrates that those controls do not
apply to the gateway; it did not attempt to exhaust the host. The source establishes the missing gateway-specific
bound.

**Impact:** An unauthenticated client can retain sockets and tasks, consuming process resources and potentially
denying service.

**Correction:** Add a gateway handshake semaphore, deadline and tracked shutdown cancellation. Apply explicit HTTP
connection, header and body budgets after the handshake. A correctly configured reverse proxy is an interim
deployment mitigation.

**Acceptance criterion:** Silent handshakes expire, concurrent handshakes cannot exceed the configured bound, and
shutdown releases them.

Source: native HTTPS accept loop `src/agent/http/mod.rs:578`.

## F6 Bulk transfer conflicts with native gateway TLS

**Preconditions:** Applications use the bulk API, with either native gateway TLS or an expectation that gossip TLS
also protects the payload transfer.

Bulk notifications carry a nonce and HTTP port. The receiver constructs an `http://` URL unconditionally, then
downloads the payload over that separate connection. The ticket contains no transport scheme or peer certificate
trust information.

**Observed:** An echo transfer succeeded with a plaintext gateway, including when gossip TLS was configured. Enabling
native gateway TLS made the same operation time out.

**Impact:** Native TLS breaks this feature combination. With a plaintext gateway, bulk payloads are exposed to network
observation or modification despite gossip TLS. The implementation also collects the response body without an
explicit bulk-response size limit.

**Correction:** Define an authenticated bulk transport contract, including endpoint scheme, peer verification,
payload integrity and size limits. Test it with native TLS and reverse-proxy deployments. Do not solve the
compatibility failure by silently falling back to plaintext.

**Acceptance criterion:** Bulk calls succeed with the supported secure deployment profiles, reject untrusted or
altered payloads, and enforce payload limits.

Sources: hardcoded HTTP URL `src/agent/bulk.rs:259`, response collection `src/agent/bulk.rs:116`.

## F7 New journals acknowledge durability before their directory entry is synced

**Preconditions:** A journal has just been created and the machine or storage loses power before its new directory
entry is otherwise persisted. This is a power-loss durability concern, not ordinary process termination.

Journal creation opens a new file and starts the writer. Parent-directory synchronisation occurs only in the
torn-tail repair branch. Each append synchronises the file data and returns `LocalDurability::OnDisk`, but the normal
new-file path does not synchronise the directory that names that file.

On Linux, syncing file contents does not itself guarantee persistence of the containing directory entry; an explicit
directory sync is needed for that guarantee. See the
[Linux fsync documentation](https://man7.org/linux/man-pages/man2/fsync.2.html). Consequently, a new journal can
acknowledge a record while recovery of the file name remains unestablished. Newly created parent directories require
attention too.

**Evidence status:** Source-confirmed ordering gap; no power-loss or filesystem fault experiment was performed.

**Correction:** Complete durable creation of the file and required directory hierarchy before acknowledging its first
durable record. Propagate synchronisation errors. Preserve the existing repair and writer-poisoning behaviour.

**Acceptance criterion:** A filesystem fault model verifies that every acknowledged first record remains
discoverable after recovery, including creation of previously absent parent directories.

Sources: creation and repair `src/agent/journal.rs:181`, durability receipt `src/agent/journal.rs:231`, file-only
synchronisation `src/agent/journal.rs:336`.

## Architecture assessment

### Foundations worth preserving

The separation between core transport/storage and higher coordination layers is useful. Explicit receipts, unknown
outcomes, signature domain separation, repairable journals, ownership locks and documented threat boundaries show
serious attention to failure semantics. The repository contains substantial regression and adversarial testing,
including negative cases. These foundations support incremental correction.

### Authorisation needs to follow the operation

Route-level checks and lists of protected RPC kinds require every new entry path to remember the same policy. F1 is
a concrete consequence. Define a provider-side contract carrying authenticated principal, operation, resource,
arguments and required authority. Dedicated endpoints, generic RPC, companion calls and forwarding should converge on
it. Policy coverage should be measurable across entry paths and feature combinations.

### Durability needs one shared meaning

F2, F3 and F7 expose three different gaps between a receipt or promise and recoverable state. Distinguish memory
application, queue acceptance, file persistence and durable file creation in the API. Security-critical state should
not use a helper with silent-drop semantics. Associate an effect with the durable decisions that authorised it, and
make recovery part of that contract.

### Membership does not establish integrity of all replicated state

The repository already documents that anti-entropy `StateResponse` entries carry no originator signature and can
introduce values through last-write-wins replication. This is a significant existing trust limitation, not a newly
discovered issue. Signed gossip updates do not close an unsigned alternate replication path. Consumers verifying
their own signed records narrow the impact but do not authenticate every namespace.

If the deployment must withstand a compromised admitted member, authenticated replicated entries and
namespace-specific write authority are necessary design work. The present trusted-fleet assumptions must remain
visible to adopters. See the documented limitation `docs/operations/what-is-proven.md:106` and the receive path
`mycelium-core/src/connection.rs:824`.

### Resource budgets must cover the whole request

The core transport and WASM facilities have explicit limits, but the gateway handshake and bulk response paths show
uneven coverage. Establish budgets across sockets, tasks, queues, request bodies, outgoing responses and retries.
Report which layer enforces each budget; a similarly named gossip setting should not imply gateway coverage.

### Configuration reports should identify residual trust

A configured security mechanism is useful evidence, but it does not establish that every path preserves the desired
property. Guarantee reports should state applicable entry points, persistence prerequisites, feature dependencies and
residual assumptions. Consensus and fencing claims also need stable electorates, persistence and resource-side
enforcement appropriate to their advertised guarantee.

## Dependency review

The root Cargo.lock was checked with cargo-audit against the locally available advisory database, whose latest commit
was dated 30 October 2026. The scan inspected 549 packages. It reported no vulnerability-class findings, but four
warnings:

| Package | Warning | Action |
| --- | --- | --- |
| Tokio 1.52.3 | RUSTSEC-2026-0386, AsyncFd constructor I/O unsoundness | Upgrade to a fixed compatible version and inspect transitive AsyncFd usage |
| backoff 0.4.0 | RUSTSEC-2025-0012, unmaintained | Plan replacement |
| instant 0.1.13 | RUSTSEC-2024-0384, unmaintained | Remove or replace through its dependency chain |
| rustls-pemfile 2.2.0 | RUSTSEC-2025-0134, unmaintained | Migrate to the maintained underlying PEM API |

No direct AsyncFd use was found in the selected root, core and WASM source searches. That does not establish
transitive unreachability, and this review does not claim an exploitable Tokio vulnerability in Mycelium. The audit
was offline, excluded yanked-package checks, and did not cover every auxiliary lockfile or the Python and JavaScript
dependency graphs. Its raw output is retained in `dependency-audit.json` beside this report.

## Recommended order of work

1. **Close authorisation and key-withdrawal gaps:** F1 and F4. Preserve the reproductions as regression tests and
   expand the route/provider matrix.
2. **Repair durable execution and receipts:** F2 and F3, followed by F7. Use deterministic crash and queue-failure
   cases with recovery assertions.
3. **Harden and reconcile network surfaces:** F5 and F6. Establish supported transport combinations and enforce
   budgets at each listener and downloader.
4. **Resolve architectural trust choices:** Define the intended threat model for compromised members, signed
   anti-entropy, operation-level authority and recovery. Update guarantee reports to match those choices.
5. **Maintain dependencies and release assurance:** Address the warnings and extend dependency coverage across
   shipped SDKs and build artefacts.

## Verification and limits

The pristine commit compiled successfully using `cargo check --lib --features compliance,a2a,llm --locked
--offline`. Five independent review test functions then passed: four root-library probes and one blackboard
integration probe. One root test exercises both bulk transport and TLS handshake retention, producing supporting
evidence for six findings in total.

These tests intentionally assert the presence of a defect. A passing probe means the reported behaviour was
reproduced, not that the product passed a security acceptance test. Early attempts were blocked by the sandbox's
localhost restriction; after network permission, all final probes completed successfully. A WAL replay path error in
the review harness was corrected before the final successful run.

The review combined source inspection of gateway authentication and authorisation, OIDC, provider enforcement,
grants, replication, persistence, journals, selected companion operations, federation, egress, WASM boundaries and
SDK paths with the targeted probes. It is a risk-focused review of a large codebase, not a line-by-line audit of
every file. No full test matrix, Linux sandbox execution, distributed partition campaign, production deployment
assessment, exhaustive secret scan or host-exhaustion test was performed.

The evidence bundle contains the source snapshot, a patch containing only the review probes, final execution logs,
dependency audit output and reproduction instructions. No production code fixes were applied.
