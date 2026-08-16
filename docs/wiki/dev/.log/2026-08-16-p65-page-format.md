# ingest — P6.5: the pluggable page codec (2026-08-16)

**Shipped (the Mycelium half of Gap 1 — the substrate-fidelity finding):** `pub trait PageFormat
{ render / parse }` extracted from `GitStore`'s hardwired codec, with the two contract clauses in
its docs: **byte-exact round-trip** (`parse(render(m, b)) == (m, b)`; refuse unrepresentable
content, never mangle) and **orphans must survive** (a `None` manifest + pre-membership blocks are
a real on-disk state — the curator's two-step write breaks if a format drops them).
`MyceliumFormat` is the built-in; `GitStoreConfig.format: Arc<dyn PageFormat>` defaults to it
(manual `Debug` since dyn traits don't derive).

**Gates (green):** codec-level byte-exact round-trip incl. the orphan-only shape; and the real
proof — `a_custom_page_format_plugs_in_end_to_end`: a JSON codec (standing in for FTT's
`CouncilWikiFormat`) replaces the built-in and the ENTIRE store contract holds unchanged (CAS
conflicts on stale content, manifest-authoritative reads) while the committed document on disk is
in the custom format with no built-in markers leaking. All 27 git_store tests green under the
default; curator + exactly-once suites unchanged.

**What remains of Gap 1 is FTT-side by design:** `CouncilWikiFormat` encodes THEIR entity contract
(entity-type kebab-case front-matter, labeled links, decisions.md row-stores — which map naturally
to a page with N sections), so it lives with their validator, gated by their fixture-vault
conformance test. The trait is ready for it.

Remaining in Phase 6: P6.6 (lower tier — spawn_blocking, timeout param, sizing contract).
