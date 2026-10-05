## [2026-10-05] ingest | realignment repairs A2 — the configuration table and the hook contracts

**What:** `docs/reference/configuration.md` (new: all 66 `GossipConfig` fields — consumer, feature,
where refused, restart, default and env var — traced at `f67b3f37`), `docs/wiki/dev/architecture/hooks.md`
(new: the five cross-layer hooks as contracts), a pointer in `GossipConfig`'s rustdoc, links from
`architecture.md` and `docs/README.md`, the plan's A2 row, and this log.

**Durable knowledge:**

- **Only five fields change at runtime** (`HotConfig`); the other 61 are read once when the agent is
  built, from an immutable `Arc`. Re-reading `ctx.config` does not make a field live.
- **28 of 66 fields are not refused when wrong** — clamped, ignored, or read as "0 = auto". That is
  not a defect list (most are deliberate), but it is the list a reviewer should read before assuming
  a setting took effect.
- **Tracing ownership found a durability defect** (R7: an uncreatable persistence directory ran the
  node in memory under a report that said persistence was enforced) and two open items (R8, R9). The
  method — for each field, find its consumer and its refusal point — is worth repeating whenever the
  config grows.
- **The trace is dated.** Both pages name the commit they were traced at; a later field added without
  a row is drift a reviewer can see.
