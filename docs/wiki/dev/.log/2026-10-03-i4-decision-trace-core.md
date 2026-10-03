## [2026-10-03] ingest | I4 — the decision trace core, a leaf that changes no decision

**What:** `mycelium-core/src/decision.rs` — `DecisionRecord`, `DecisionSink` (count + byte bounds, drop-newest,
`try_lock` only, counters that travel with an export), `to_jsonl()`, `DECISION_ATTACHMENT`. Lock-order row 53.

**Durable knowledge:** G7 in code is three choices — (1) the sink never reads a clock or draws: `at_ms` is
the time the decision point already held, and the only number the sink mints is its own sequence; (2) the
sink's lock is taken with `try_lock` and a contended record is *dropped and counted*, so a trace can never
make a decision wait; (3) bounds drop the **newest** record, so a trace is a prefix of what happened plus the
count of what it did not keep — evicting the oldest would lose the beginning, which is where a cascade
starts. And G8's rule for the reader: every `Completeness` flag is *unknown*, never *none* (`no_parent()` and
`no_effect()` exist so a decision can say "none by nature" explicitly).
