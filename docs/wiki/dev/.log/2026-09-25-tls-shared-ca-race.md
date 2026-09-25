## [2026-09-25] ingest | the shared-CA race behind the federation suite's peering flakes

Up: [dev](../dev.md) · page: [testing](../testing/testing.md) · code `mycelium-core/src/tls.rs`
(`load_or_create_ca`).

**Finding.** The federation Docker suite failed intermittently on "alpha a1 sees its three peers" (nightly 09-24)
and "beta b1 sees its one peer" (a PR run 09-24). Those checks poll for 90 s, so a timing margin could not explain
them. The domain's nodes start concurrently on one shared `/ca` volume, and `load_or_generate` was check-then-act:
each node could find no CA, generate its own, trust a different root, and never complete mTLS with its peers. A
reproduction (8 threads, one empty directory, a barrier) fails on the old logic immediately: nodes also read
half-written CA files.

**Change.** `load_or_create_ca`:
- an exclusive-create `ca.lock`, so exactly one process generates;
- a re-check under the lock;
- files published by write-to-temp then rename;
- others wait, bounded at 30 s, and load;
- a stale lock is an error naming the lock.

Three tests, including the 20-round race.

**Separately.** The overlay suite's S11 failures on `main` (09-24, twice) were the 5 s readiness re-check made real
by #374. The already-written fix `d89c208` is landed as #396. The open S12 intermittency is neither of these: overlay
nodes run without TLS.
