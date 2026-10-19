## [2026-10-19] ingest | Feature pages F1: the template and two pilots

`docs/features/` started (plan `docs/plans/feature-pages.md`, row F1, in progress): the front page
`docs/features/README.md` (what a feature page is, the six sections, link-don't-copy, the list of pages), the
skeleton `_template.md`, and two pilots — `activation-egress-proxy.md` (post-360 rows L and M) and
`inference-routing.md` (row R). The third pilot, K2's fleet-wide history limits, waits for PR #653. The capability
map's *Acquire and adapt*, *Constrain actions* and *Run tools and reasoning* rows link the pages; its maintainer note
is F3's.

What the pilots taught, recorded as proposed revisions in the plan's §4.1: a feature turned on in code has no
setting to link; neither pilot has an example in the index (each is shown end to end by a test); the `stem.*`
guarantees are registered by the stem and absent from the guarantee catalogue; catalogue, ledger, configuration
and plan rows have no per-entry anchors; and neither script the F2 gate is to share resolution with resolves both
heading and `<a id>` anchors. Checking the pilots' links found one dead anchor already in the repository
(`unit-file.md` → `crown-jewel.md#credentials-attached-at-egress`), fixed.

Pages: none in the wiki. Repo: `docs/features/` (new), `docs/capabilities.md`, `docs/plans/feature-pages.md` (rev
0.3), `docs/reference/unit-file.md` (one anchor).

## [2026-10-19] ingest | #654 review round 1: the Cedar claim, fixed markers, the pilots corrected

The independent review found a stale claim the routing page had copied: the private Cedar adapter has expressed
`ForbidClassAt` and `AllowAtMostAcross` and passed all twenty-seven contract cases since its v2.36.0 re-pin (private
#33, `0d654fd`). Corrected everywhere it was stated as current: guide 20, `deprecations.md` §52 and §53, the two
what-is-proven rows, and the post-360 plan's rows R and K2, §3.5 (dated notes), §3.6 and §5. Corrected on the
pages too: both reason doors give the preflight destination and locality (only the class needs the classifying
door); `guide_20_routing_policy` runs a hand-kept copy; the proxy's port is added to the command's own `connect`
entries; the credential path's test named. P1 and P2 now carry fixed markers the F2 gate can check
("This feature is turned on in code", "No runnable example exists", `example: none`); `docs/README.md` lists
`features/`.
