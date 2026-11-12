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
  Signed and verified (Meets or Partly — see the review note below): the identity, certificate, revocation and
  client-certificate records, per-record audit, I5's exchanged token, electorate certificates, the verified award,
  a publisher-signed catalogue line. Does not hold: the grant
  use marker and `consumed`, the audit chain's head, `cap/`·`req/`·`prov-shed` and the other advertisements,
  `sys/ae-tally/` and `sys/ae-exposure/`, `sys/capauthz/`, `sys/caller-context/`, a key-less principal in a caller
  envelope, decision records under `consensus/life/`, `grp/`, `sys/topology-override/`, a membership intent, and
  anti-entropy entries (no writer, by design).
- **Found on the way:** `filter_protected` (H3) has no caller outside its tests; I4's `call/…/window` record is
  specified in the plan and not on `main`; several effects are marked *inferred* and need a test before a fix.
- **Touched:** `security.md` (a section pointing here), `dev.md` (folder-note), `AGENTS.md` (routing row),
  `CLAUDE.md` (one hot-invariant bullet), `.claude/commands/adversarial-review.md` (check 8),
  `docs/plans/boundary-h.md` §17.

**#772's review, fixed in the same PR** (the inventory was not yet fit to plan from):
- **The rule's text:** (a) gained its second clause (where the path names no principal, or the record's subject,
  the owner is the authority the reader is configured to trust); (b) now includes currency (sequence, issue time,
  supersession); (c) says absence fails closed; **Meets** means all four hold, so most signed rows are now **Partly**
  (only per-record audit and I5's token are at Meets).
- **Reclassified:** `grant/…/revoked` (a planted marker stops a real revocation), `consensus/trust/`, the
  ballot/decided floors with `sys/consensus-accepted/`, the electorate certificate (a self-certified genesis, read
  from the code), `sys/govern/membership/` (a tombstone takes the floor to 0), `consensus/committed/` (every
  never-decided slot), `sys/load/` (entries counted, not nodes), `sys/govern/fleet` and `sys/config/` (no range).
- **Rows added:** the bare RPC's `sender`, `sys/identity-cert/{self}` as the owner reads it, `tools/` through
  `tool_provider`, `log/cn/…`, `ckpt/`·`ckptw/`·`strands/`, `cap/{node}/locality/self`, `log/reason/` (untraced),
  the second `prompts/` reader.
- **For the plan:** the gateway tombstones the use marker itself when a call is refused before dispatch
  (`authz.rs:716`), so a signed marker needs a signed release.
- **Also touched:** `docs/threat-model.md` (H3 is built, not in force), `boundary-h.md` §9 step 2,
  `docs/operations/what-is-proven.md` (six dated rows under *Not yet shown*), `runtime-invariants.md` (a
  back-pointer), check 8's wording. Every `file:line` on the page was re-read.

No code changed. The philosophy document and the forwarding invariant are untouched: that is a separate decision.
