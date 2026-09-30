# 2026-09-30 — F2/F3: proposed → shadow → accept (D20), and the guide section

**What:** `docs/plans/design-time-tooling.md` §17 F2 + F3. Manifest v2 (`proposed` inside the
publisher's signature, `Acceptance` outside it over the publisher's signature), the shadow lane in the
provisioner (`{name}.shadow`), `require_reviewers`, promotion by withdrawal, `mycelium-artifact accept`
/ `verify --reviewer`, A2's `would bind after acceptance`, the coop demo's wave 3, guide 16 § Agent-
authored functions, the runbook's trust paragraph, the doc-coverage row.

**Durable knowledge:**
- **Two domains, one line.** A proposal signs under `mycelium-installable-proposed-v1`, an accepted
  entry under the v1 domain, so the same content proposed and not-proposed never share a signature and
  flipping the byte breaks it. The acceptance signs over the publisher's *message and signature*, so it
  names one proposal under one publisher key — moved onto another proposal it fails.
- **v1 stays byte-identical.** An entry with neither field encodes exactly as before, so nothing
  published changes and the golden fixtures hold; only a proposed or accepted entry is v2, which a v1
  decoder does not see — the fail-safe direction (an un-upgraded provisioner cannot load a proposal).
- **A shadow must not share the incumbent's RPC kind.** The runtime claims `cap.invoke/{ns}/{name}`
  whether or not it advertises, and `deliver_to_handlers` fans out to every handler of a kind — so
  skipping the advertisement is not a safe shadow (on the incumbent's node it would execute every call).
  The shadow is a rename: installed and advertised as `{name}.shadow`, which no requirer's filter
  matches and which a comparison can call on purpose.
- **Promotion is withdrawal.** The hosted map is keyed by artifact, so an accepted entry whose artifact
  is already live as a shadow would be skipped forever; the promotion pass withdraws a shadow whose
  catalogue line is now loadable (or gone) and lets the ordinary demand pass bring it live under its
  own name when demand is unmet. Acceptance travels as the *same* catalogue line rewritten (same KV
  key), so the librarian's signer scoping still holds.
- **The checker sees a proposal only where it sees artifacts at all** — when no deployed provider
  exists. A proposed replacement beside a deployed incumbent is invisible to `wire-check` by design
  (the runtime keeps the last word); the finding is a class, not a lane.

**Pages touched:** guide 16, `operations/artifacts.md`, `doc-coverage.md`, plan rows F2/F3, CHANGELOG,
`mycelium-wasm-host/README.md`.
