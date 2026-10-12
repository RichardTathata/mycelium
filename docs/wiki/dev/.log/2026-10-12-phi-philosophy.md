## [2026-10-12] ingest | philosophy: "a protocol, not a service" stated in full, cited from runtime-invariants (row Φ)

- The post-360 plan's row Φ. The philosophy's corrected-litmus blockquote **A protocol, not a service** (`docs/philosophy.md`)
  now states §2's line in full — consensus is a protocol every member of a group runs, never a service a named node
  provides, Layers I and II knowing nothing of it — and the deployment rule as a rule over every shape: no profile,
  configuration default, unit file, governance route or document may make a named node set the place where agreement
  happens. The electorate's statement was corrected from the pre-P2 reading ("declared by governance with its size",
  "fixed for the life of that decision") to what 2.32.0 built: a group whose members are fixed at genesis by a
  decision of every member it names, changed only by a migration that terminates (#606). What 2.32.0 enforces is
  named with the version (P2's `consensus_require_electorate`, the default set once, nothing resizing an electorate
  group; C1 leased leadership; C2 collected acceptor state), and the detection-not-prevention posture restated for
  an embedded `grp/` write.
- Pages touched: `dev/architecture/runtime-invariants.md` § *Layer III posture* — the *Discovery is not an electorate*
  paragraph gains a one-sentence pointer with a relative link to `philosophy.md#the-corrected-litmus` (the page cited
  the paragraph by name before, without a link); outside the wiki `docs/philosophy.md`,
  `docs/plans/post-360-hardening.md` (head, row Φ, §3.5 Φ block), `CHANGELOG.md`.
- Gates run: `scripts/check-positioning.sh`, `scripts/test-check-materials.py`, `scripts/test-check-front-doors.py`,
  and a relative-link resolution pass over the touched files (no link-check script exists; the resolver is in the
  session's scratchpad, not the repo).
