## [2026-10-05] ingest | realignment repairs A1 — "no control plane" defined, "profile" disambiguated

**What:** `docs/positioning.md` (the definition, under the visitor's expansion — the expansion text
itself is unchanged, so it does not drift from the README), the top of
`docs/guide/agentic-control-plane.md`, `docs/guide/00-concepts.md` (two vocabulary rows; the wiki
paragraph now says "control functions"), `docs/operations/control-profiles.md` and guide 22 (the three
profiles side by side), the plan's A1 row, and this log.

**Durable knowledge:**

- **"No control plane" means no separate control-plane service.** The definition used everywhere:
  *Control decisions live in participating nodes: enabled governors and provisioners act locally,
  subject to configured consensus and resource-side authority checks. No separate Mycelium
  control-plane service is required.* "Enabled" matters — a minimal embed runs none, and a governor
  enforces only under its node's control profile. The phrase stays in the positioning sentence
  (decision D2); the definition is what makes it true rather than rhetorical.
- **Three things are called "profile", and none implies another:** the startup profile (what a node
  refuses to start without), the control profile (how its governors act, set at runtime), and a
  consensus policy (how one operation is agreed, per call). A deployment that needs all three states
  all three. Use the qualified name in new prose.
