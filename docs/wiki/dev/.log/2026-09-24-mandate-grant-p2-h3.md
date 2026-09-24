## [2026-09-24] ingest | signed mandate grants (P2) and advertisements bound to authority (H3)

Up: [dev](../dev.md) · record `docs/design/knowledge-issuer-binding.md` §5–6 · code `src/mandate/grant.rs`,
`src/mandate/protected.rs`.

**Finding.** `Mandate` was unsigned, so no reader outside the resource could check an appointment. Any member could
advertise any capability, so a power no mandate gave could be offered as a service.

**Change.**
- P2: tagged canonical bytes, and `GrantVerifier::check` covering issuance (P1), configured entitlement, the window,
  the retained epoch (superseded, conflicting), and possession bound to the request.
- H3: `filter_protected` over resolve results. The grant must name the advertiser, permit `serve:{ns}/{name}`, and
  carry possession bound to node, namespace and name. Unbacked capabilities are filtered and reported.
- Nine P2 tests (including a revoked member authority) and six H3 tests.

**Kept honest.** Entitlement is configuration only (the consensus gate). Retained epochs are in memory. H3 is
reader-side, so colluders are exposed, not stopped. The report is a return value, not yet wired to metrics or audit.
