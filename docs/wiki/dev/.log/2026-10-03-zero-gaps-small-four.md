## [2026-10-03] ingest | zero gaps Z6–Z9 — the small four

**What:** `mycelium_effects::Counting<D>` + `RefusalCounts` (+ feature `metrics`); `KvReceipt` from both
SDKs' `set()` (py 0.2.5, ts 0.1.2); `gossip_peers_connected` published from `writer.rs` where the
writer map changes (`get_or_spawn_writer`'s won claim, `reap_finished_writers`); `provisioning_viz`
on :8101. Plan `docs/plans/zero-gaps.md` D6–D9.

**Durable knowledge:** a counter on a destination must wrap `apply_composed` by delegation, not
re-derive it from `apply` — the inner destination may take the authority inside its transaction, and
re-deriving would count a composed refusal twice or not at all. The connected-peer gauge is set at
the two mutation sites rather than polled: a gauge polled on the stats path is only as fresh as the
last scrape, and the GC sweep already visits the map.
