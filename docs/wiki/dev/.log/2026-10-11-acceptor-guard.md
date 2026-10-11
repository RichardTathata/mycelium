## [2026-10-11] ingest | the acceptor's memory guard — persistence required, an empty store warned, a forgotten promise withholds the slot

- The code half of #604's hazard (`docs/wiki/dev/.log/2026-10-11-restore-or-retire.md`): three detections in
  `src/agent/acceptor_guard.rs`, each by name and counted, none a guard on a Layer I write.
  1. **Persistence required** — `persistence_required(ctx, decl)` is `decl.contains(me) && config.persistence.is_none()`;
     refused at `electorate::declare_outcome` (`ElectorateError::PersistenceRequired`, `409 persistence_required` in
     `http::gw_govern_electorate_declare`), at `ConsensusEngine::electorate_door`'s genesis path (`DoorRefusal`), at
     `electorate_admits`'s genesis path (no answer, so a genesis naming such a member is never decided) and at
     `electorate_view`'s adoption (`refuse_adoption_without_persistence`, once per record). Enumerated by grepping
     `electorate_view(`, `electorate_admits(`, `declare_outcome` and `adopt`: adoption happens in one place
     (`electorate_view` → `adopt_through_gate` → `electorates.insert`).
  2. **An empty store** — `check_memory_for` / `check_memory`: a member of a *committed* genesis whose memory holds nothing
     for `electorate/{group}/{epoch}` (every member accepted it; `shrink_acceptor` keeps `{promised}`). Run at
     `lifecycle::start` after `prewarm_genesis_touched` (`check_at_start`, over `consensus/committed/electorate/` in the
     store), on adoption in `electorate_view`, and on the committed record's arrival through the inbound seam. Warned,
     not refused: the record is unverified there, and a new member holds nothing until genesis.
  3. **A forgotten promise** — `compare_own_record`, on the inbound seam: a copy of `sys/consensus-accepted/{me}/{slot}`
     whose `promised` or accepted ballot is above memory's `promised` (memory is monotone; a shrunk record's floor is
     within it). The slot goes into `TaskCtx::acceptor_withheld`, read at `answer_prepare`, the listener's vote path,
     `propose_inner` (after the door; `Timeout`, metrics reason `acceptor_withheld`) and `cross_propose_inner`. Cleared
     only by a restart — `prewarm_accepted` reads the repaired copy then. Never resumes live: refreshing memory from
     the store would race the acceptor's own writes.
- **The core seam:** `CoreCtx::inbound_observer: OnceLock<InboundObserver { prefixes, on_inbound }>`, set by
  `acceptor_guard::install` in `GossipAgent::new`, called from `connection::apply_inbound` (the one inbound apply, three
  call sites: anti-entropy entries, `Data`, `SignedData`) after the apply, whatever LWW decided — so an older copy that
  lost to a newer local write is still reported. Core reports and decides nothing; the closure captures only leaf
  state (the memory map, the two sets, the `NodeId`), never the context, so a stopped agent is freed. papaya sets — no
  lock-order row.
- **Seen failing first** (2026-10-11, every detection toggled off): the declaration was decided; the gateway answered
  200 with epoch 1; the restarted member adopted epoch 1; nothing was counted on the empty store; and the interleaving
  from #604's review committed **v2 at ballot 2** on C with B's memory moved from `(1, v1)` to `(2, v2)` — one earlier
  toggled run ended `Superseded { ballot: 1 }` instead (the hazard's manifestation is timing-dependent; the assertion
  fails either way).
- **The limit:** detection 3 needs the copy to arrive, and to arrive before the node answers a higher ballot for the
  slot (a copy carrying `b` is below a memory that promised `b+1` empty-handed); the test waits for the repair before
  the second proposer prepares. Stated in design §8.5, guide 04, `what-is-proven.md` (the P2 row and the third table),
  threat model §7, `runtime-invariants.md`.
- **Test harness:** `electorate_mesh` / `electorate_line` now configure `[persistence]` per node (`electorate_cfg`,
  a temp directory per port), as a member must; a restart over an empty store is `remove_dir_all` + the same config.
  A restarted member is reached again only once A's writer meets the dead socket at its next write and redials,
  then anti-entropy's next round — measured 15–25 s on a loaded host; the restart polls are bounded at 60 s.
- `acceptor_guard` is gated on `consensus` like the memory it guards (the first gate chain's `--no-default-features` build
  and the wasm-host build caught it ungated); the refusal's text lives in `electorate.rs`, whose error enum is in every build.
- Rule catalogue: `cons.acceptor_memory` rev 1 (the first consensus entry). Counters on `GET /gateway/stats`;
  `mycelium_acceptor_guard_total{event}`.
