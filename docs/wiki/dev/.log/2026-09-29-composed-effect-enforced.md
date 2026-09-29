# 2026-09-29 — the composed effect, enforced at the destination

**What:** `docs/design/composed-effect.md` §9. `mycelium-effects` gains `Composition` (principal,
operation, mandate, origin domain), `ComposedEffect`, `CompositionLeg`, `check_composition`,
`EffectRefusal::Unauthorised { leg, reason }` (the enum is `#[non_exhaustive]` now) and the trait's
`apply_composed` with a default that checks then applies. The gate `tests/composed.rs` was written first
and observed to fail: with the composed path delegating without checks, every planted leg committed.

**Durable knowledge:**
- **Attribution before authority.** The check refuses an effect presented under someone else's mandate
  as an *attribution* failure whatever that mandate permits; the mandate contract's own
  `ResourceAuthority::check` supplies every authority reason (superseded, wrong scope, out of window,
  not enumerated) — no second copy of that rule exists.
- **A refusal leaves nothing**: no business row, no dedup row, so a later authorised attempt is `Fresh`.
  That is the property that stops a refused attempt from later being read as a replay.
- **The domain leg is carried, not re-verified.** A destination holds no trust bundle; the origin the
  gateway established is recorded, and the sentence the record supports is *enforced attributed and
  authorised, recorded cross-domain*. Identity strength still rests on `require_identity_proofs`.
- **The window is stated, not closed:** check then transaction, the same check-then-act window every
  resource in the tree has; a destination that can take the authority inside its transaction should
  override the default.
- **The join's two halves landed the same day.** `Composition::from_envelope` (effects feature
  `envelope`, which pulls the substrate's `gateway` + `tls` in) takes the operation a grant must
  enumerate from `mandate_operation(operation, resource)` — the gateway's own rule, so the destination
  and the gateway check the same string — and refuses an envelope bound to a different holder, term,
  scope or epoch than the presented grant. `AeEvidence::for_destination_refusal` is a `Decided` record
  at enforcement point `destination`, `Deny`, `Execution::None`, the leg in `checked`; no new record
  kind, so the private exporter's mapping is untouched.

**Pages touched:** guide 18 (the composed path, the `_` arm); `what-is-proven.md` (a proven-in-CI row);
`CLAUDE.md`'s composed sentence; the buyer deck's evidence slide callout; `CHANGELOG.md`.
