## [2026-10-10] ingest | locks and leases end where every node can see it (row A, with C1 and C2)

- The 360 review's K1–K3 had one cause: a slot's end (a release, a lapsed lease) was not written anywhere that
  named the decision it ended. Release tombstoned `committed` (and GC later removed the tombstone), expiry was
  measured against an entry a late COMMIT could re-stamp, and the ballot that ended lived in `consensus/decided`,
  which travels separately. Fix: `consensus/lease/{slot}` became a **lifecycle record** — window, ballot, value
  digest, released flag, measured from its own timestamp — and everything that asks "is this decision over, and at
  which ballot" reads `ended_at`. Wire unchanged; the first 8 bytes stay the window for older readers.
- C1: `elect_leader` leased (30 s), renewed by calling again, `release_leadership` to step down, permanence by
  `LeaderTerm::Permanent`; the gateway route likewise, plus `DELETE /gateway/overlay/elect/{group}`.
- C2: acceptor state collected on the listener's 60 s tick under the stated three-part condition; a new leaf-ish
  lock (`TaskCtx::acceptor_records`, row 56 → row 7) serialises record writes with collection.
- Pages touched: `dev/architecture/runtime-invariants.md` (§ acceptor memory rewritten: the lifecycle record,
  the collection condition, leased election); `dev/concurrency/lock-order.md` (row 56); `docs/guide/04-consensus.md`
  (operations table, § leader election); `docs/philosophy.md` (*Mandate TTL applies to decisions too*: the leader
  default is now leased). The decision record `docs/design/consensus-electorate.md` and philosophy's "protocol, not
  service" paragraph (row Φ) are not on `main` yet — their *not built* for C1/C2 is #594's to strike.
