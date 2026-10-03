## [2026-10-03] ingest | I6 — the trace as a bundle attachment, compared and explained

**What:** `Bundle::attachments` (+ `DECISION_ATTACHMENT`, `COVERAGE_MANIFEST`, `KNOWN_ATTACHMENTS`);
`decision::{canonical, attributable, compare, Comparison, Divergence, coverage_manifest, explain}`;
`mycelium rules`, `mycelium explain`; the stem writes `coverage.json` beside its trace.

**Durable knowledge:** the comparison's unit is the *canonical tuple*, and what it leaves out is the
point — the sequence number and the stamps differ between a recording and its replay by construction.
And a replay can only be *asked* to reproduce decisions whose inputs the seams cover: a decision that
read a gossiped view or a sampled resource is reported *unattributable*, which keeps `reproduces()` from
being either a false pass (ignoring it) or a false fail (counting it). The bundle gap is a binary gap,
not a design one: the node that records bundles has no instrumented decision point yet.
