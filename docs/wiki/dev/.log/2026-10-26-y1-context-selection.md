## [2026-10-26] ingest | adopter-surface Y1 — context selection (D14)

`src/knowledge/selection.rs`: `ReadScope` (from a current mandate enumerating `knowledge.read`; covers its scope and
what lies beneath by path segment; an empty scope covers nothing), `ScopedView` (the direct read under the same rule,
withholding a record whose `DerivedFrom` source is not held or not readable — one reason for both), `Selector::select`
(records with attribution, standing, verdict and `VerdictClass`, ranked by the adopter's closure), and
`write_condensed`/`check_condensed` (a claim signed by its own issuer, in scope, `DerivedFrom` only, sources readable and
in one class, re-derived at write). Invariant worth keeping: **the derivation rule is what stops a condensation
laundering out-of-scope content** — coverage by subject alone would let a record filed under an in-scope subject carry
an out-of-scope record's text. Pages touched: `history.md`; outside the wiki the feature page, guide 23, the concepts
row, the plan's §8 row, the CHANGELOG. No lock added.
