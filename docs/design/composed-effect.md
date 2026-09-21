# The composed guarantee — a durable, attributed, cross-domain effect (ADR, v3 contracts axis)

**Status:** adopted 2026-09-21 · closes the Phase-C audit's **composition finding**, recorded in
`CHANGELOG.md` since v2.9.1 as *known and not fixed*. Composes item 1 (receipts), item 7 (caller
identity), item 2 (domains) and the AE slice's evidence journal. **No new ledger, no new identity,
no new receipt** — the whole of this record is a join between things that already exist.

> Posture, once: stated at the strength it has (rule 6). This record makes the composed claim
> **reconstructable from evidence**. That is a weaker thing than making it *enforced*, and §7 says
> exactly where the line is.

---

## 1. The finding, and why it is not a bug

§12.6's Phase-C adversarial audit ran against items 1, 2 and 7 *together*, because those three
compose into the axis' headline claim:

> **a durable, attributed, cross-domain effect**

and posture rule 6 says a composed guarantee is established by argument **and** gate, never by
naming. The audit's finding:

> The composed guarantee is proved **leg by leg and nowhere as a whole** — the receipt tests and the
> federation tests have zero overlap. Underneath: a receipt carries **no principal**, the evidence
> journal carries **no durability rung**, and a receipt is **never persisted**.

Every leg works. Nothing is broken. What is missing is that no single artefact ever holds all four
properties at once, so the sentence cannot be checked — only believed.

## 2. What each half has, and what it lacks

| | durable | attributed | cross-domain | survives the call |
|---|---|---|---|---|
| **the receipt** (item 1) | **yes** — `LocalDurability`, the rung it names | no principal | no origin domain | **no** — returned, then gone |
| **the evidence record** (AE) | no durability rung | **yes** — `subject`, item 7's verified principal | partial — the principal is issuer-qualified | **yes** — the node-local journal |

Two halves of one sentence, describing the same `operation_id`, that never meet.

**A correction worth recording, because I argued the opposite first.** `EvidenceState`
(`OnDisk`/`Unknown`/`NotEstablished`/`NotConfigured`) looks like the missing rung and is not: it
sits on `AeReference` and describes whether **the evidence record** reached disk, not whether **the
effect** did. Two durabilities, one name away from being confused — which is itself an argument for
writing this down.

## 3. The join

**The execution record carries the effect's receipt.** That is the entire mechanism.

`AeEvidence` already carries `subject` (the verified principal), `operation_id` / `attempt_id`
(item 1's identities), and `execution` (whether it ran). It gains:

- the **durability rung** the receipt named — item 1's own `LocalDurability`, not a copy of it;
- the **origin domain**, when the caller came through a federated edge, so *cross-domain* is a fact
  on the record rather than an inference from the principal's spelling.

A recorded execution record is then the composed claim, in one place, after the fact.

### Why this way and not the other

The alternative is to put the principal and the domain on the *receipt*. Rejected: a receipt is
returned to a caller synchronously and is the wrong place to accumulate context — it would grow a
field per composing item, and it still would not be *persisted*, which is the third half of the
finding. The evidence journal already solves persistence; the receipt already solves durability.
Join them where the durable thing already lives.

## 4. Ordering — the concern, and why it resolves

AE0 §5 sets the strict profile's boundary as *journal append acknowledged (`OnDisk`) before the
effect proceeds, or the effect is refused*. Putting a **receipt** in the journal looks like it
inverts that: a receipt exists only *after* the effect.

It does not, because the two records are different:

| Record | When | What it establishes |
|---|---|---|
| `Decided` | **before** the effect | the decision, which is what the pre-effect barrier protects |
| `Execution` | **after** the effect | what became of it — and now, how durable it was |

The barrier stays where AE0 put it. The receipt rides the record that was always post-effect, so
nothing is inverted and no append is added in front of an effect.

## 5. When the composed claim cannot be made

The execution record's own append can fail, and then there is an effect whose receipt was never
recorded. **This is a gap and is reported as one**, never resolved in the reader's favour:

- `EvidenceState::OnDisk` — the composed claim is reconstructable.
- `EvidenceState::Unknown` — the record may exist; the claim is **not established**.
- `EvidenceState::NotEstablished` / `NotConfigured` — the claim cannot be made at all.

The AE3 correlator already expresses exactly this distinction and already refuses to invent an
all-clear from silence, so this record adds no new reading rule — it adds a field the existing rules
can read.

## 6. The gate

Posture rule 6 requires a gate, not an argument. The gate is one test, and it must fail in **both**
directions:

> From the evidence journal alone — with no access to the receipt that was returned, the caller's
> memory, or the federation client's state — reconstruct: **this effect, on this operation, was
> durable to this rung, attributed to this verified principal, originating in this domain.**

Planted both ways: remove the durability rung and it must fail; remove the origin domain and it must
fail. A test that passes with either absent is not testing the composition.

**And the negative**: an operation whose execution record is `Unknown` must reconstruct as *not
established*, never as a durable attributed effect. The finding is about a claim nobody could check;
a gate that can be satisfied by a missing record would reproduce it.

## 7. What this does and does not establish

**Does.** After the fact, the composed sentence is **checkable from one artefact** rather than
believed across four. That is what the audit asked for and it is all this record claims.

**Does not.** It does not make the composition *enforced*. Nothing here prevents a durable effect
from being attributed to the wrong principal — item 7's verification does that, at its own strength,
and this record carries that verdict rather than re-deciding it. The composed guarantee is
**evidenced at the strength of its weakest leg**, and the record names which leg that is rather than
averaging them.

It also does not close §13's composition *hypothesis*, which is a research question and explicitly
not a v3 deliverable. This is about the axis' own claim, not about whether composition generalises.

## 8. What lands next

The `AeEvidence` fields and the reconstruction gate, in that order — the gate is written first and
observed to fail, because a gate written after the thing it gates tends to describe it.
