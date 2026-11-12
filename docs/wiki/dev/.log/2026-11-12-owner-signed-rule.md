## [2026-11-12] ingest | the owner-signed rule, and the inventory of where it does not hold

The maintainer adopted a design rule on 2026-11-12: **a security-bearing record kept in the gossip KV is signed by
its owner and verified at read.** It had been rediscovered three times in a week by independent reviews (#757's
DPoP binding, #766's window record bound to its key's gateway segment, the grant use marker — still open), and an
external critique of the same day named it as one root cause across grants, audit, provisioning and caller identity.

- **New page:** [`owner-signed-records.md`](../owner-signed-records.md) — the rule, why it does not contradict
  *detection, not prevention*, the four conditions (a)–(d), what is security-bearing, what is outside the rule, and
  the inventory.
- **The inventory** was enumerated from `OWNED_KV_PREFIXES` / `refuse_protected_key`
  (`src/agent/http/authz.rs`) and `src/lib.rs` § KV namespace ownership, each row read from the code at `7d4c86a6`.
  Meets: the identity, certificate, revocation and client-certificate records, per-record audit, I5's exchanged
  token, electorate certificates, the verified award, a publisher-signed catalogue line. Does not hold: the grant
  use marker and `consumed`, the audit chain's head, `cap/`·`req/`·`prov-shed` and the other advertisements,
  `sys/ae-tally/` and `sys/ae-exposure/`, `sys/capauthz/`, `sys/caller-context/`, a key-less principal in a caller
  envelope, decision records under `consensus/life/`, `grp/`, `sys/topology-override/`, a membership intent, and
  anti-entropy entries (no writer, by design).
- **Found on the way:** `filter_protected` (H3) has no caller outside its tests; I4's `call/…/window` record is
  specified in the plan and not on `main`; several effects are marked *inferred* and need a test before a fix.
- **Touched:** `security.md` (a section pointing here), `dev.md` (folder-note), `AGENTS.md` (routing row),
  `CLAUDE.md` (one hot-invariant bullet), `.claude/commands/adversarial-review.md` (check 8),
  `docs/plans/boundary-h.md` §17.

No code changed. The philosophy document and the forwarding invariant are untouched: that is a separate decision.
