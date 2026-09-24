# Knowledge validity, heads and storage (ADR, Boundary H items K1–K3)

**Status:** K1 **adopted and implemented** 2026-09-24 (`KnowledgeStore::put_signed`, `src/knowledge/store.rs`). K1b,
K2 and K3 are **proposed**: decisions recorded here, implementation to follow. Plan:
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

## 2. K1b: present eligibility at resolution (proposed)

`classify` re-derives, for every record and at read time:
- **authenticity**, from its `Attribution`: `Unchecked` counts only under a policy that explicitly allows it, and
  the confined-fleet profile forbids it;
- **present authorisation of the issuer**, by re-running `verify_issuer` on the retained signature against the
  reader's *current* key view. A key revoked after storage makes the record non-current;
- **currency for this decision**: not retracted or superseded (through the `DependencyIndex`), no withdrawn basis,
  within `max_evidence_age_ms`, and judged under the current policy revision.

Eligibility is never cached as "eligible". Verdict reasons name the layer that excluded a record.

## 3. K2: signed heads, ancestry and durable checkpoints (proposed)

**What exists.** `Head { issuer, stream, record, seq }` in `store.rs`, with `advance_head` enforcing that an issuer
publishes only its own heads, that a head points at its issuer's record, and that a lower or equal `seq` is refused
as `StaleHead`. Heads are **not signed**, despite the knowledge-layer ADR calling them "signed heads", and they
carry no ancestry.

**Decision.**
- **The head format.** A head gains `prev_head_digest` and a signature by its issuer, verified through P1. A head
  that fails verification reads as absent.
- **Checkpoints.** The reader keeps a per-stream **checkpoint**: the latest head verified *by ancestry*.
  Checkpoints are **durable across restart**.
- **Rules for a new head:**
  - A higher `seq` advances the checkpoint **only** through an unbroken chain of signed heads linked by
    `prev_head_digest`.
  - If that chain cannot be obtained, the result is `ContinuityUnavailable`, and the checkpoint is kept.
  - If the chain diverges before the checkpoint, the result is `ForkedStream`, and both heads are retained and
    reported.
  - A lower `seq` is `StaleHead`, which is the existing behaviour.
  - Two different heads at the same `seq` are a fork.
  - A head whose body cannot be fetched is `BodyUnavailable`: insufficient evidence, never absence.

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

**K1b, K2 and K3:** as listed in plan §6 and §10, cases 2 and 3.
