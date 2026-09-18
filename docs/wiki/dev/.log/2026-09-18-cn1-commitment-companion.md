## [2026-09-18] ingest | CN1 — the commitment companion (the contract net as five records)

Up: [dev](../dev.md) · plan `docs/plans/v3-contracts-axis.md` §6.9, §13.2 · code `mycelium-commitment/`
(`src/lib.rs`, `examples/redistribution_cn.rs`), `mycelium-core/src/signal.rs` (`kv_ns::CN`), `src/lib.rs` (the
namespace row), CI job `commitment`.

### What landed

The third coordination model of the epoch, beside the tuple space's `take` and the blackboard's facts, built as
the plan insists — a composition, not a subsystem. Five records with a mechanism each on the public API: the
announcement as a declarer-owned head, offers as an `append` stream, the award by the tuple-space election's
lowest-participant rule and written with `set_with_receipt`, reports as an `append` stream whose outcome may be
*unknown*, assessments signed by whoever has standing. The gallery entry re-runs the redistribution workload the
other two companions run, so the three models meet on one workload.

### Three things worth carrying forward

1. **The award is a receipt, and that is what makes it an acceptance rather than a KV write with a hopeful
   name** (D39's sentence). `Awarded { award, receipt }` carries item 1's `WriteReceipt` under
   `operation_id = cn/{requirement}/award`, so a retried award is recognisable and its durability is what the
   receipt says. The example asserts `application == Applied` on every award; it does not assert `OnDisk`,
   because the node has no persistence configured and a receipt that said so would be the collapse item 1 undid.
2. **A second award is refused, not written over.** The local half of "one award per requirement per epoch":
   `award` reads the award head first and returns `AlreadyAwarded(existing)`. Two declarers racing to the key is
   LWW's problem and the companion's defining failure — CN2's replay witness, not a local check, is where that is
   caught. Named here so the refusal is not mistaken for the whole rule.
3. **No offers is a state, and so is a late one.** `NoOffers` is returned, not retried; an offer after the
   deadline stays on the record and is not considered. Nobody is assigned anything: the award records an offer
   the participant made, and a requirement nobody offered on is visible as exactly that.

### Not done here

CN2 (the award replays; a double-award witness under `mycelium-sim`) and CN3 (the award checked against the
acceptor's mandate epoch). Participants are named, not authenticated — authority is item 7's and item 5's, not
this crate's. Single node in the example: the model, not the topology, is what it shows.

### Pages touched

- [companions/companions.md](../companions/companions.md) — the `mycelium-commitment` bullet.
- [history.md](../history.md) — the CN1 section.
- CLAUDE.md's companion-crates line; the plan's §6.9 table marks CN1 landed; CHANGELOG.
