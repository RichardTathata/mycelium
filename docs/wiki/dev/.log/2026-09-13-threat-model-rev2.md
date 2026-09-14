## [2026-09-13] ingest | item 8 — threat model revision 2

Up: [dev](../dev.md) · page: [security](../security.md) · record `docs/threat-model.md` §5–6 · plan §6.5 (done).

**Finding.** Revision 1 models an admitted mesh of cooperative members. Items 2, 3 and 5 each stated a threat model in
prose — foreign principals and authenticated-but-abusive clients, evidence confidentiality and the hash-as-credential,
a compromised former holder and a forged epoch — with no document they could cite, so three ADRs would each re-argue
a threat model and drift.

**Change.** §5 adds Boundaries D–G in revision 1's own shape (gains · mitigations by item · residual): **D** the
domain edge — transports never joined, membership / federation identity / service authorization kept apart,
bilateral bundles, no leader, SWIM off in the enforced profile; **E** the authenticated-but-abusive client — item 7's
attested `GatewayCaller` closes the deputy, AE0 bounds *what* at the route with `indeterminate ≠ permit`, the gateway
remains a preflight; **F** evidence — opaque addresses (the hash is integrity, never the credential), heads in KV
and bodies in an authorised store, equivocation preserved, evidence never grants what authorisation denies;
**G** mandates — the decisive epoch invariant inside the store's atomic boundary, `MandateSuperseded`, three
lifecycle events apart, attribution outliving authority. The shared rule: authority is recomputed, never inherited;
a named mechanism is not the composed guarantee. §6 fixes the artefact rules from plan §9: verified claims and scoped
attestations never carry credentials; replay bundles are redacted at the recording seam with stable placeholders so
determinism holds; a *protected reproduction artefact* class names what cannot be redacted and still reproduce.
§1–4 unchanged, so existing anchors hold.

**Kept honest.** Boundary E states the gateway's guarantee as a route-level preflight with `coverage.complete:
false`; D and G state that revocation cannot reach a disconnected domain or holder; F states that the substrate
decides nothing about truth. Item 6's bundle format will cite §6 rather than invent its own redaction rule.
