## [2026-09-26] fix | the customer-materials review — defects, the kit, the deck

Up: [dev](../dev.md) · follows [review F7/F9](2026-09-26-review-f7-f9-doc-drift.md).

A second external review, of the customer materials (examples, operations guides, deployment
scaffolding, both decks). Assessed claim by claim against the tree; every concrete claim held.
Fixed on `docs/customer-materials` (PR to follow):

- **Defects first.** The A2A checklist's "configure a bearer" was not an admission requirement
  (`a2a_optional_auth` proceeds anonymous without a header) — replaced with the three real ones and
  a negative probe. "Copy the dirs while the node runs" was overpromised: a copier can take the old
  snapshot and the truncated WAL — replaced with quiesced copy or one filesystem snapshot, and a
  back-up-together inventory. The deployment guide said the Kubernetes manifests implemented its
  persistent profile; they never did (no PVCs, no TLS, no auth) — both pages say so now, and the
  example tag moves to the current release. "Strong consistency" became "quorum agreement" across
  the guide, with the **supported profile for exclusive effects** stated once in
  `docs/threat-model.md §7` and checked in `production-readiness.md`, not in a module comment.
  `guardrail_viz` and `control_envelope_viz` shared `:8096`; distinct defaults and
  `MYCELIUM_VIZ_PORT`. The co-op README said twelve CI demos and ran thirteen; entries 12 and 13
  gained walkthroughs in the tutorial-contract shape.
- **Direction.** `docs/operations/engagement-kit.md` — the Mycelium-specific consultancy kit the
  review found missing: seven parts, three roles, an acceptance script of eleven rows, and its own
  acceptance test. `examples/README.md` gained five recommended paths, a by-outcome index, and the
  tutorial contract; the co-op README a business translation table.
- **The deck.** Six lines making one overstated argument rewritten in one pass, the undersell fixed
  in the same pass; recorded in `docs/publications/README.md`'s overclaim ledger with the lesson
  for `/publication-lint`.

**Pushed back on, and left:** the A2A route stays public by design (the fix is the checklist and
the probe, not the route). "Nothing to fail" and the subpoena line were confirmed present after a
first search missed them under markup — the reviewer was right.
