## [2026-11-13] ingest | the gateway's kind-taking doors refuse the substrate's own kinds

A survey after the slot guard (#778) found the consensus kinds observable and emittable through the gateway's
signal routes; the maintainer decided the fix the same day, with no debugging exception.

- **Proved first, each a failing test** (`src/agent/http/substrate_kind_tests.rs`): a `mesh:read` tap carried a
  peer's proposed value and its `cn/…/award` slot on `consensus.propose` and `consensus.commit`, at both stream
  doors; without `[tls]` a `mesh:write` client's `CommitTerm` on `signal/emit` was written as the slot's decision
  on the gateway's node and on its peer; a tapped nonce and an emitted `rpc.result` answered a caller in the
  provider's place. With the guards as they were, 110 of 110 (kind, door) pairs did not refuse.
- **Built:** `SUBSTRATE_KINDS`, `SUBSTRATE_KIND_PREFIXES`, `is_substrate_kind`, `HOSTED_CAPABILITY_KIND_PREFIX`
  (`src/agent/mod.rs`); `refuse_internal_kind` under `refuse_protected_kind`, `refuse_unservable_kind` and
  `refuse_unobservable_kind` (`src/agent/http/authz.rs`); the mailbox stream refuses what `mailbox/deliver`
  refuses; `confine_kind` (`mycelium-wasm-host`) refuses a listed substrate kind; `mailbox` (both SDKs) and
  `onSignal` (TypeScript) raise `ProtectedKindError`.
- **Pages touched:** [security.md](../security.md) (new section, *A kind the caller names*). Outside the wiki:
  `deprecations.md` §103, `rbac.md`, guides 09 and 10, the feature pages `signal-mesh.md` and `service-calls.md`,
  `what-is-proven.md`, the critique plan (CR25 closed; CR26–CR28 opened).
- **Durable lesson:** the handler table is one table. A door that lets a caller name a kind is a door onto every
  receiver registered there, so it needs a refusal list that defaults closed for the substrate's prefixes — the
  same shape as the owned-key and owned-stream tables, for kinds.
