# Knowledge validity, heads and storage (ADR, Boundary H items K1–K3)

**Status:** K1 **adopted and implemented** 2026-09-24 (`KnowledgeStore::put_signed`, `src/knowledge/store.rs`). K1b
**adopted and implemented** 2026-09-24 (`classify_eligible`, `src/knowledge/resolution.rs`). K2
**adopted and implemented** 2026-09-24 (`src/knowledge/heads.rs`). K3 is **proposed**: decisions recorded here, implementation to follow. Plan:
[`docs/plans/boundary-h.md`](../plans/boundary-h.md) (rev 0.4, proposed) §6. It builds on the
[issuer-binding ADR](knowledge-issuer-binding.md) (P1) and the [knowledge-layer ADR](knowledge-layer.md) (item 3).

> Posture, once: **an authentic historical record is not current evidence.** Three questions stay separate. Is the
> record authentic (K1)? Is its issuer currently authorised? Is it current, unretracted support for *this*
> decision (K1b)?

---

## 1. K1: authenticity on storage (adopted)

**Problem.** `KnowledgeStore::put` accepted any `KnowledgeRecord` and verified nothing. A record also carried no
signature to verify. Separately, `KnowledgeRecord` derives `Deserialize`, so a record from the wire never passed
through `KnowledgeRecord::new`: its id might not match its content, and a foreign retraction could be smuggled in.

**Decision.**
- **`SignedRecord { record, signature }`** is the travelling form. The signature is not part of the record or its
  id: identity stays content-derived.
- **`KnowledgeStore::put_signed(signed, members, external)`** checks two things before storing:
  1. **integrity:** the id is the content's digest, and the record satisfies `new`'s rules (no foreign retraction,
     no self-link, a non-empty subject);
  2. **attribution:** through P1's two admissible paths, with the key taken from the reader's own view.
- The outcome is recorded as an **`Attribution`** snapshot:
  - `Verified { path, key, revoked_at_storage }`, with the signature retained;
  - or, for the legacy path, `Unchecked`.
- **Refusals are counted, never silent.** `PutRefusal` covers `IdMismatch`, `Malformed(RecordError)` and
  `Unverifiable(reason)`, and `refusal_counts()` reports them by stable label.
- **A record signed under a since-revoked key is stored**, as authentic history, flagged `revoked_at_storage`.
  Refusing it would erase attribution, and attribution outlives authority.
- **Idempotent.** A record already verified keeps its first attribution. An `Unchecked` record is upgraded by a
  later `put_signed`, and a verified one is never downgraded by a later `put`.

**Deviation from the plan, recorded.** Plan §11 listed "`KnowledgeStore::put` returns `Result`" as a breaking
change. K1 is **additive** instead: `put_signed` is new, and `put` is kept with its records marked `Unchecked`.
- Why: `put` has 72 call sites across tests, the gate, the example and two companion crates. Nearly all of them
  build records locally, so those records are trusted by construction.
- The guarantee is unchanged. A record from outside enters only through `put_signed`, and K1b's resolver policy
  decides whether `Unchecked` records count.
- Whether `put` is later deprecated goes to the §6.6 removal ledger once K1b exists and every in-repo caller has a
  stated reason to stay unchecked.

**Not claimed.**
- `Attribution` is a snapshot. It settles authenticity, which is historical, and does not settle present standing.
- The member path's strength rests on `require_identity_proofs`, which is default-off (see the P1 ADR).
- Nothing here yet stops an `Unchecked` record from counting. That is K1b.

## 2. K1b: present eligibility at resolution (adopted)

**Problem.**
- `classify` never consulted retraction. A withdrawn assessment kept supporting a release.
- Nothing distinguished a verified record from an unchecked one.
- Nothing re-checked a stored signature against the reader's current keys.

**Decision.** Eligibility is **re-derived on every call** and never cached as "eligible". Each exclusion answers
one of three separate questions:

| Question | Exclusion | Source |
|---|---|---|
| Authentic? | `Unchecked` (only under `UncheckedRule::Exclude`) · `NotAttributableNow(reason)` | The `Attribution` from K1; `verify_issuer` re-run on the retained signature against the reader's **current** key view |
| Present authority? | `KeyRevoked { key }` | Revoked at storage, or since |
| Current for this decision? | `Retracted` · `SupersededByIssuer` · `BasisWithdrawn` · `Expired` | `correction::standing`, plus same-issuer `Supersedes` |

- **`classify_eligible(…, members, external) -> Classification { verdict, excluded }`** re-verifies against the
  current key view and reports every excluded record with its reason, sorted by record id.
- **`classify`** keeps its signature and now shares the same core. It applies the currency checks, the
  `Unchecked` rule and `revoked_at_storage`. It does **not** re-verify signatures, because it has no key view.
  This is a **behaviour change that can only narrow**: a retracted, superseded or basis-withdrawn assessment no
  longer counts.
- **Only an issuer supersedes its own record.** Another issuer's `Supersedes` link is disagreement, not
  replacement, and does not exclude anything.
- **Suppression resistance.** The `DependencyIndex` used for currency is built only over records the reader
  accepts as authentic and currently authorised (`DependencyIndex::build_filtered`). Under `Exclude`, an unchecked
  "retraction" of someone's verified support has no effect. Suppression is an attack in its own right, not only
  over-assertion.
- **The default is `UncheckedRule::Count`, for compatibility.** Existing stores are built with `put` and keep
  their verdicts. A reader receiving records from others sets `Exclude`, and the confined-fleet profile requires
  it. Flipping the default goes to the §6.6 ledger.

**Not built here.** The plan's "judged under the current policy revision" layer: `ReaderPolicy` has no revision
concept yet. It is recorded as open, not claimed.

## 3. K2: signed heads, ancestry and durable checkpoints (adopted)

**What existed.** `Head { issuer, stream, record, seq }`, with `advance_head` checking the issuer, the record's
issuer and a higher `seq`. Heads were **unsigned**, despite the knowledge-layer ADR calling them "signed heads", and
had no ancestry, so a higher `seq` from a different history passed.

**Decision.**
- **Format.** `Head` gains `prev: Option<[u8; 32]>`: the digest of the previous head in the stream, or `None` for
  the first. It has its own tagged `canonical_bytes`, so a head signature can never authenticate a record, and a
  `digest`. `SignedHead { head, signature }` is the travelling form.
- **Authentication.** Heads are verified through P1's two paths (`verify_signed_by`, the byte-level form P1 now
  exposes). A head signed under a revoked key is refused (`SignedUnderRevokedKey`): a head is a present-tense claim.
- **`HeadCheckpoints::offer(offered, intermediates, members, external) -> HeadVerdict`:**
  - lower `seq` → `StaleHead`;
  - same `seq`: the same head → `AlreadyHeld`, a different one → `ForkedStream`;
  - higher `seq`: walk `prev` back through authenticated intermediates, and then:
    - meeting the checkpoint → `Advanced`;
    - a missing or unauthenticated link, or a non-descending chain → `ContinuityUnavailable`, and the checkpoint
      stays;
    - passing the checkpoint's height without meeting it, or reaching a genesis head above it → `ForkedStream`.
  - Forks are retained (`forks()`) and never resolved.
- **Durability as a contract.** A `CheckpointStore` trait:
  - `open` **loads** state and **refuses** unreadable state (`OpenError::Unreadable`), rather than starting empty
    and silently resetting rollback protection;
  - every advance is **persisted before it is reported**, and a persist failure (`CheckpointNotPersisted`) leaves
    the checkpoint where it was.
- **Trust on first use, stated.** The first authenticated head for a stream becomes its checkpoint. Continuity is
  guaranteed from then on, never before.

**Not claimed.**
- **The only store shipped is `MemoryCheckpointStore`, which is not durable.** A file-backed store must go through
  the filesystem seam (`scripts/check-sim-seams.sh`) and arrives with K3's durable record store. Until then,
  rollback protection across a restart is exactly as durable as the store the embedder supplies.
- `KnowledgeStore::advance_head` remains, unsigned and without ancestry, for local use. Its doc comment now says so
  and points to `HeadCheckpoints`.
- Forks are held in memory, not persisted.
- `BodyUnavailable` (a head whose record cannot be fetched) belongs to K3's transport. `is_resolvable` already
  reports it locally.

## 4. K3: durable store, head transport, body authorisation (proposed)

- **Durability.** Records live in a durable, fsynced store, following the evidence journal's pattern, keyed by
  `RecordId`, with the `Attribution` and signature stored beside each record.
- **Transport.** Heads gossip under the reserved `knowledge/head/{issuer}/{stream}`. Record bodies are fetched on
  demand and enter only through `put_signed`.
- **Body authorisation.** Boundary F's opaque-address rule applies: the content hash checks integrity and is never
  the access credential.
- **Ingestion bounds.** Per issuer and per cohort, with refusals counted (plan H1).

## 5. Gates

**K1 (built, `src/knowledge/store.rs` tests, `mod k1`):**
- a verified record is stored with its attribution and signature;
- a forged record is refused, counted and not stored;
- an invented issuer is refused;
- a revoked-key record is stored as history and flagged;
- a deserialised record with a stale id is refused;
- a deserialised foreign retraction, with a recomputed id, is refused;
- an unchecked `put` is marked, is upgraded by a later `put_signed`, and never downgrades a verified record.

**K1b (built, `src/knowledge/resolution.rs` tests, `mod k1b`):**
- a retracted assessment stops counting, and nothing is deleted;
- unchecked records are excluded under `Exclude` and counted by default;
- a key revoked after storage removes the record from the count while it stays in the store;
- an external issuer no longer trusted stops counting, with the reason named;
- an unverified retraction cannot suppress verified support;
- only the issuer can supersede its own record;
- an assessment whose basis was withdrawn stops counting.

**K2 (built, `src/knowledge/heads.rs` tests):**
- a verified extension advances;
- a higher head without its ancestry leaves the checkpoint where it was;
- the reviewer's case, a higher head from a branch that diverged below the checkpoint, is a fork, and both heads are
  kept;
- same-`seq` forks, duplicates and stale heads are each reported as such;
- checkpoints survive a restart and keep refusing rollback;
- unreadable state refuses to open;
- an unpersisted advance is not taken;
- forged heads and forged links do not count;
- a head signed under a revoked key is refused;
- a head must point at its own issuer's record;
- a new genesis above the checkpoint is a fork.

**K3:** as listed in plan §6.
