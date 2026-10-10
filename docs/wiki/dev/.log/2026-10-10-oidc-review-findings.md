## [2026-10-10] ingest | OIDC key refresh: the adversarial review's findings on #590

- **A cached `kid` waited behind a fetch.** Holding the `RwLock`'s write guard across the fetch made it
  single-flight and also queued every verify behind it, valid cached `kid`s included — one random-`kid` request per
  cooldown stalled every login for a round trip (up to the timeout against a hanging IdP). The cache is now a
  `std::sync::RwLock` written only to swap in a result; single-flight is an `AtomicBool` + `tokio::sync::Notify`
  (no second lock, so one lock per function holds and no guard crosses an `await`). Row 17 rewritten; the
  `tokio::sync` exceptions are down to row 37.
- **A cancelled verify spent the cooldown without fetching** (the attempt was stamped before the fetch, and the
  fetch was the caller's future). The fetch runs in a detached task that stamps its own outcome.
- **"At most 10 s" was 20 s with discovery.** One deadline across discovery + JWKS: each request gets what is left
  (`RequestBuilder::timeout`, so no `tokio::time` site).
- **An IdP blip at start refused every token for 30 s after recovery.** With no keys at all the retry spacing is
  `JWKS_EMPTY_RETRY_BACKOFF` (2 s).
- Past the TTL, a cached `kid` is served from the cached keys while they refresh in the background
  (stale-while-revalidate) — the same never-wait rule applied to the TTL path.
- Tests for rotation after the cooldown, previous keys kept on failure, and the TTL refresh; the cooldown test's
  margins widened (2 s cooldown). Pages: `dev/concurrency/lock-order.md`, `dev/security.md`; `docs/operations/sso.md`.
