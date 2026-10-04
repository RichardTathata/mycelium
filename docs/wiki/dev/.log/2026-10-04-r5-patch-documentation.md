## [2026-10-04] ingest | realignment repairs R5 — the egress claim restated, the recovery shown

**What:** `docs/threat-model.md` Boundary C, the `egress.allow_list` guarantee in
`src/agent/guarantee.rs` and the regenerated `docs/reference/guarantee-catalogue.md`,
`docs/operations/crown-jewel.md`'s coverage table, `docs/operations/confined-fleet.md`'s egress row,
`examples/receipt_ladder.rs` step 8b, the changelog, the plan row, and this log.

**Durable knowledge:**

- **The egress claim is a hostname allow-list on the first URL and every redirect hop.** Say it that
  way. "Every outbound HTTP path" was never true (redirects, the parser, the object store's bucket)
  and is still not the claim: name resolution, a cloud identity's credential traffic and
  `object_store`'s internal redirects are outside it, and the docs now name all three.
- **The threat model contradicted itself.** One paragraph said the list gated every path; the next
  said OIDC JWKS was ungated, which had been false since 2026-10-03. A claim and its residual sitting
  side by side is where drift shows first; read both when either changes.
- **The receipt ladder can stage a crash honestly** by copying the files while the node runs and
  replaying the copy — the same method as the R2 witness. A clean shutdown cannot show WAL durability:
  its final snapshot is taken from memory.
