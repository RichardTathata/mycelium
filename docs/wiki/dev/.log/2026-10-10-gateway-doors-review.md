# 2026-10-10 — #587's adversarial review, answered

## [2026-10-10] ingest | gateway doors review findings (F1–F10)

- **Replicated serve loops (F1).** A signal fans to every receiver, so N serve streams of one principal on one
  kind each get every request; `rpc/respond` now accepts the repeat as `200 {"duplicate": true}` (dropped,
  counted in `rpc_respond_duplicates`) and the served record is insert-if-absent. A refusal an SDK raises is a
  loop-killer: any "answer once" rule on a fan-out needs an idempotent second answer.
- **The 300 s answer window (F2)** is the gateway's RPC ceiling; `/gateway/llm/call` now clamps to it; an
  in-process `rpc_call` is not bounded by it (stated on `SERVED_RPC_TTL_MS`).
- **`Signal::sender` is emitter-written (F3).** The `(nonce, sender)` claim stops a stray reply, not a forger who
  writes the target's id — stated in `SystemStats`, lock-order row 2, deprecations §23.
- **Public surface (F4).** `COMPANION_PUBLIC_PATHS` lists agentfacts' two routes; the test is renamed
  `every_listed_public_path_is_mounted_and_public_and_gated_paths_are_not` (it was
  `the_public_surface_is_exactly_the_documented_list`) and checks mounting by 405 — axum cannot enumerate routes,
  so an unlisted public route remains a review rule.
- **A2A id reservation (F5, F6).** `reserve_task` claims the id with one `compute` before the dispatch, after
  federation authorisation; `TaskReservation`'s `Drop` removes only its own pending entry.
- **SSE reply kinds (F7)**, **`mesh:serve` visibility sentence (F8)**, **amortised sweep (F9)**, manifest (F10).

Pages touched: `dev/concurrency/lock-order.md` rows 2 and 55; this log.
