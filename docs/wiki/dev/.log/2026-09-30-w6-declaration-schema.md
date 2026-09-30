# 2026-09-30 — W6, the public half: the declaration's schema and its call shapes

**What:** `docs/reference/declaration.schema.json` pins `mycelium.design/declaration/1` (JSON Schema
2020-12) and `tests/declaration_schema.rs` validates the golden and freshly generated reports against
it with a real validator (`jsonschema`, dev-only), plus three plants (an unknown top-level key, an
unknown provider kind, an unknown severity are refused). Every `Edge` now carries `operations` — its
two call shapes as `{operation, resource}` — and `wire_check::call_shapes` is public.

**Durable knowledge:**
- **A schema file nobody validates with is prose.** The pin is a test with a validator behind it, and
  the plants are what make "the golden validates" a claim rather than a tautology.
- **Carry the call shapes; do not make a consumer re-derive them.** The authority overlay reasons
  over `skill.invoke skill:{ns}/{name}` and `tools/call tool:{name}`; an exporter that inverted those
  from an observation's `(operation, resource)` would restate a private rule — and would meet the
  ambiguity the tool shape has (it drops `ns`: `logistics/intake` and `depot/intake` both become
  `tool:intake`). With the shapes on the edge, a consumer maps each through its reviewed catalogue and
  reports the ambiguity as `ambiguous`, never a guess.
- **The private half waits for a tag.** The private companion pins the substrate by tag (its rule 1),
  and no tag contains the checker (W2 landed after `v2.16.0`). Its design, from the survey of the
  exporter: `NovusLensSink::deployment_declaration(subject, &Report, &ActivityCatalogue, at_ms)` beside
  `policy_deployment` — the same `envelope_record → push → take_batch → seal` path, so signing, the
  batch id and the `issued`/`retry`/`commit` memory are reused unchanged; a content-derived record id
  over the revision; a payload that says *declared* (never *desired* or *enforced*) with
  `coverage{complete:false}` and the edges' operations mapped through the catalogue (`unmapped` /
  `ambiguous` by name); a `deployment_declaration` arm in the stub consumer's per-type schema and the
  enrolment's record types; the join in a new `declared.rs` keyed on `(principal, ns, name, schema_id)`
  with `null` a real key value; tests seen failing first (retry returns remembered bytes; an unreviewed
  operation exports as `unmapped`; on the coop fixture a `skill.invoke skill:depot/intake` observation
  from `worker` resolves to exactly one edge and a `tools/call tool:intake` one is *ambiguous*).

**Pages touched:** plan row W6 (◐), the checker's module doc, CHANGELOG, the golden fixture.

**Closed the same day, after the tag.** `v2.17.0` was cut (the first tag carrying the checker) and the
companion's PR #4 shipped the private half on that pin: the `deployment_declaration` batch beside
`policy_deployment`, the conformance arm (seen failing first — *no payload schema for type*), and
`declared::join` / `declared::wired` on the vendored co-op golden. The plan row is ✅; the proof page's
open line is now the consumer's own rendering.
