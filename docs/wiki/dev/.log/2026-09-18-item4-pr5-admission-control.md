## [2026-09-18] ingest | item 4 PR 5 — admission control at the companions' queues, reported

Up: [dev](../dev.md) · record `docs/design/adaptive-stability.md` §1, §3, §6, §9 row 5 · runbook
`docs/operations/admission-control.md` · code `mycelium-tuple-space/src/{store,lib}.rs` (`rejected_total`,
`StageAdmission`, `TupleSpace::admission`), `mycelium-blackboard/src/{lib,store,rpc}.rs`
(`BoardConfig.high_watermark`, `BlackboardError::Backpressure`, `BoardStats.rejected`, `ST_BACKPRESSURE`).

### What landed

The last row of item 4's table. The tuple space already *had* admission control — the watermark refused a
`put` — but the refusal was a silence: no count, nothing beside completions. Now each stage counts what it
turned away and the primary reports it beside the admitted and the taken, and the metrics writer publishes
it. The blackboard had no bound at all; it gets one — `BoardConfig.high_watermark`, `None` by default — with
the same counting, and the refusal crosses its RPC as itself.

### Three things worth carrying forward

1. **The mechanism was there; the report was the gap.** §1's service objective is not "refuse work" — the
   watermark did that — it is *rejected work reported beside completions*. Reading the store showed
   `put_total`, `take_total`, `hot_total` and no `rejected_total`: the bound worked and nobody could see how
   much it refused. The PR is mostly a counter and its readers.
2. **A fixed encoding decides where a count can live.** The depth RPC is a hand-rolled, unversioned frame,
   so `TupleDepth` cannot grow a field without breaking mixed-version decode. The count lives at the
   primary — which §3 says owns the deficit anyway — behind a `Some`/`None` API whose `None` means *ask the
   primary*, and in the metrics writer. The blackboard's RPC had a free status code, so its refusal could
   cross as itself; a pre-2.8 client sees a generic error, never a false success.
3. **Replication is not admission.** The bound applies where work enters, not where it is replayed or
   mirrored: `post_with_id` never refuses, or a bounded board could not recover what it held. The test
   pins that beside the refusal.

### The §6.6 entry

`BoardConfig` gained a field and is not `#[non_exhaustive]`, like `GossipConfig`; the plan's removal ledger
now names it beside the others. `TupleConfig` gained nothing — the watermark existed.

### Not done

A governor that acts on the count; fair scheduling; a hard (rights-backed) bound at a queue — the record
keeps those apart on purpose.

### Pages touched

- [history.md](../history.md) — the item 4 PR 5 section; the ADR's §9 marks row 5 landed.
- `docs/operations/admission-control.md` (new), indexed from the operations README; CHANGELOG; the plan's §6.6.
