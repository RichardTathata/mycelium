## [2026-11-20] ingest | two CI flakes: a pooled test client that lost a request, a scaffold that waited for one side

**`an_ordinary_kind_passes_every_door`** (`src/agent/http/substrate_kind_tests.rs`; CI run 44752877045, the strict
`compliance,a2a` lib line). Reproduced 17 in 200 under load. Server-side counters showed the lost request never reached
the gateway; the client's `hyper-util` pool trace showed a connection re-pooled before its request was answered, once in
each failing run and never in the passing ones. The module's client now keeps no connection (`door_client`,
`pool_max_idle_per_host(0)`): 0 in 200. `the_door_client_keeps_no_connection_between_requests` pins it (fails with the
pool on). A test-client fault, not the gateway's.

**The `governed-call` scaffold** (`templates/governed-call/src/main.rs`; the Scaffold job, same run, attempt 1). It
waited for B's connection only; A, the seed, drops a reply to a node not yet in its table and does not resend it. Not
reproduced unplanted; with A made to learn B late the old wait failed 9 of 9 with CI's *"Error: Timeout"*, the new
two-sided bounded wait passed 13 of 14. The `stem_compliance` 60 s miss is a separate start-up bound, left open.

Durable knowledge on the [testing page](../testing.md) § The CI flake tier; the record in
`docs/plans/post-360-hardening.md` §3.6.
