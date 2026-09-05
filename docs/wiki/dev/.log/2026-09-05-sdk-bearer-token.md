# SDK gateway-bearer support — mycelium-py 0.2.4 / mycelium-ts 0.1.1 (2026-09-05)

Found by doc-coverage run 16 while auditing the gateway-auth fixes: **neither SDK sent an
`Authorization` header or exposed a token option** (grep of both `src/` trees: zero hits), while every
operations page tells operators to set `gateway_auth_token` beyond loopback and — after #172 — every
companion and node-level route sits behind it. A token-protected node was unreachable from Python and
TypeScript. Reported as a code gap in the matrix, fixed the same day.

## Shape
- **One place per SDK.** Python: `ClientPool(base_url, timeout, *, token)` computes
  `headers` once (`resolve_token` → explicit, else `MYCELIUM_GATEWAY_TOKEN`; `auth_headers`) and passes
  them to both pooled clients; the five dedicated SSE clients in `agent.py` and the A2A
  `httpx.stream` pass `self._pool.headers`. TypeScript: `src/auth.ts` (`resolveToken`, `authHeaders`,
  `AuthOptions`); each class holds `private readonly auth` merged into every `fetch`; `sseStream`
  accepts `{ url, headers }`.
- **Every handle, same option:** py `token=` (keyword-only) on `MyceliumAgent`, `Wiki`, `TupleSpace`,
  `Blackboard`, `PromptSkillClient`, `ReasonClient`, `A2aClient`; ts trailing `{ token }` on
  `MyceliumAgent`, `Wiki`, `TupleSpace`, `Blackboard`, `PromptSkillClient`, `A2aClient`. Positional
  signatures unchanged — additive.
- **No token → no header.** Open (loopback) gateways see no difference.

## Gates (no node needed)
- `mycelium-py/tests/test_gateway_token.py` — a stub HTTP server records `Authorization` per
  request: explicit / env / none on `MyceliumAgent` sync calls, **the SSE path** (`on_signal`), the
  four async companion handles, A2A card fetch, `ReasonClient` pool headers. 7 tests.
- `mycelium-ts/tests/auth.test.ts` — `fetch` recorder: helpers, agent GET/POST/DELETE, env
  fallback, no-token, the five other clients, `sseStream` with headers. 6 tests.
- **CI:** the `sdk-ts` job now runs `npx jest` after `tsc --noEmit` (the live suite self-skips).
  This closes the review's TypeScript-CI coverage note.

## Same-day follow-up — `persisted` surfaced
`consistent_set` / `cross_group_propose` (py) and `consistentSet` / `crossGroupPropose` (ts) return
a `CommitResult { persisted }` (`None`/`null` when the gateway predates v2.4.2). Both returned
nothing before → additive. Python truthiness is `persisted is True`, deliberately *not* the commit
status (the methods raise on a failed commit). Gates: `test_commit_result.py`, `commit_result.test.ts`.

## Not done
- Browser builds of the TS SDK have no `process.env`; pass the token explicitly (documented).
