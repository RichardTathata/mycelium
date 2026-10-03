## [2026-10-03] ingest | I1's rule half — the catalogue, generated and gated

**What:** `mycelium_core::rule` (descriptor types + structural `check` + `Catalogue::gather/to_markdown`),
`mycelium::rules::RULES` (11) and `mycelium_wasm_host::rules::RULES` (15), the generated
`docs/reference/rule-catalogue.{json,md}`, and `mycelium-wasm-host/tests/rule_catalogue.rs` which fails on a
duplicate id, a dangling relation, an outcome without typed reasons, a descriptor naming no test or a test
absent from the tree, or a stale checked-in document.

**Durable knowledge:** a generated document cannot establish that a description is *true* — only the
behavioural test each descriptor names can, which is why "names a test that exists" is the gate and not
"has prose". Relationships are recorded as hypotheses (plan §6): the trace is what will show them. Writing
the entries exposed what the trace must be able to say: `prov.eligible` has side effects (counters) and is
never re-evaluated for a record, so a decision record must carry the outcome the live evaluation produced,
not re-run the rule (G7's first risk, now concrete for I4).
