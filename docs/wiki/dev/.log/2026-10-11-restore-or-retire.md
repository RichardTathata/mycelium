## [2026-10-11] ingest | a lost electorate voter is restored with its state or retired — never reissued empty

- The correction: 2.32.0's pages said an electorate member could be replaced "under the same identity".
  Unsafe as stated — since 2.30.0 a member's promises and acceptances are durable in its own persistence
  directory (`sys/consensus-accepted/{node}/{slot}`, `persist_acceptor` with a forced fsync before the
  answer leaves, `src/consensus.rs:1331-1357`, `:4121-4128`) and read back once at start by
  `prewarm_accepted` (`src/consensus.rs:3725-3743`, called at `src/agent/lifecycle.rs:349-354` after the
  WAL replay at `:274`). No empty-acceptor guard: the call site logs only when it found records, and the
  memory is never fed from anti-entropy. An acceptor reissued its `NodeId` over an empty or older store
  can promise below what it promised and forget what it accepted — two decided values.
- The removal ledger forecloses a restore: monotonic (`src/membership.rs:20`), a removed key refused at the
  TLS handshake (`mycelium-core/src/tls.rs:570-579`), a removed `NodeId` dropped at the connection whatever
  key it carries (`mycelium-core/src/connection.rs:392`, `:678`); `NodeId` is a socket address
  (`mycelium-core/src/node_id.rs:20-22`), so a reissued certificate does not readmit it.
- Pages touched: `runtime-invariants.md` (the *Since P2* paragraph); outside the wiki `docs/guide/04-consensus.md`,
  `docs/design/consensus-electorate.md` §8.3/§8.5/§8.6, `docs/threat-model.md` §7, `docs/operations/what-is-proven.md`
  (the P2 row and a new third-table row: a lost majority's disaster recovery is not built), `docs/guide/faq.md`,
  `docs/operations/cert-rotation.md` (a new section before *Removing a member*), `deployment.md`'s restore table,
  `production-readiness.md`, `confined-fleet.md`, `CHANGELOG.md`.
- Not built, named: a start-time refusal for a known electorate member whose store holds no acceptor records; the
  disaster-recovery genesis for a lost majority.
