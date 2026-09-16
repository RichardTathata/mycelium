## [2026-09-16] ingest | the evidence record carries its own event time

Up: [dev](../dev.md) §AE · record `docs/design/action-envelope-ae0.md` §5 · code
`src/agent/action_evaluator.rs`.

**Found from outside, again.** Not by reading the substrate, but by building the exporter against it:
the consumer's `activity_observation` needs an `at`, `AeEvidence` had no timestamp, so the exporter
stamped its read time. Its own retry test failed within minutes.

**Why read time is wrong twice over.**

1. **It breaks retry identity.** Record ids derive from journal position, so an exporter that loses
   its cursor and re-reads produces the *same* `batch_id` — with a different `at`, hence a different
   body. The consumer refuses "a different body under a reused id". That is precisely the rule the
   T3 gate exists to guarantee, broken by the exporter's own hand.
2. **It is the wrong fact.** The contract says *event time is yours* — when the decision happened,
   not when somebody got round to reading it.

**The fix is one field the substrate was already holding.** `ActionEnvelope::issued_at_ms` is the
enforcement point's assembly time and `for_decision` was throwing it away. Now carried as
`AeEvidence::at_ms`. Both records of one attempt carry the same value, because they are about one
attempt and an exporter merging them into a single observation needs one event time for it.

**The general shape, worth naming.** *A timestamp a record does not carry is one its reader has to
invent, and an invented one cannot be stable.* The same is true of any field a downstream contract
requires: if the producer does not carry it, every consumer makes one up, and they disagree. Worth
checking the other §5 records against the consumer's required fields before they are written.

**Also:** `AeEvidence` is now `#[non_exhaustive]`. It is a type an exporter turns into a record
another organisation parses; the next field addition should not break anyone. Doing it now costs
one upgrade note and nothing else, because the only external consumer is ours.

**Gates.** `make check` clean; 521 (`compliance,a2a`).
