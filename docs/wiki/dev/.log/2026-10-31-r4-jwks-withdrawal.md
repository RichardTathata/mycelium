# 2026-10-31 — R4: a successful JWKS fetch is authoritative; kept keys have a ceiling

External review 2026-10-31, finding F4 (`docs/plans/external-review-2026-10-31.md` R4, PR #712). The OIDC verifier
(`src/agent/oidc.rs`) read a fetch that returned no keys as a failed fetch and kept the keys it held, so a key the
issuer had withdrawn went on verifying tokens; nothing bounded how old kept keys could get while fetches failed.

**Durable knowledge.**

- **A successful fetch is the issuer's whole set.** `fetch_keys` returns `Result<Vec<_>, FetchFailed>`: `Ok` is what
  the IdP published, read **key by key** (one key `jsonwebtoken` cannot parse — a post-quantum `alg`, ES512/P-521,
  an unknown `crv` — is dropped, not the set), possibly empty; `Err` is a fetch that did not reach a JWKS —
  unreachable, timed out, egress-denied, a non-2xx status (`status().is_success()`, not `error_for_status`, which
  passes a 3xx), or not a JWKS. `Ok` replaces the cache; `Err` leaves it. The discovery document is likewise read
  only from a 2xx.
- **The ceiling is anchored on the last success.** `JWKS_MAX_STALE` (24 h) counts from the last successful fetch
  whether or not any token was seen meanwhile: `CachedKeys::past_ceiling` makes `plan()` know no `kid` and
  `usable()` hand out no keys, so a verify is a fetch or a refusal, never `UseAndRefresh`. A quiet node's first
  token after a day refreshes; if that one fetch fails, it and every token until a success are refused.
- **The 2 s back-off is for a cold start only** (`fetched_at.is_none()`), not for "no keys held": a successful empty
  set is a withdrawal to honour, retried on the 30 s cooldown.
- **The grants-style blind spot, here:** the pre-existing refresh test simulated failure with `"garbage"` and never
  with a valid empty set, so the empty-set reading was never exercised.

Pages touched: `dev/security.md`, `dev/concurrency/lock-order.md` row 17; operator docs `operations/sso.md`,
`operations/crown-jewel.md`; `guide/deprecations.md` §75.
