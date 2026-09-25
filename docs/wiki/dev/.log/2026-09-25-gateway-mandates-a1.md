## [2026-09-25] ingest | mandates established at the gateway (Boundary H A1 wiring)

Up: [dev](../dev.md) · page: [security](../security.md) · record `docs/design/authority-at-execution.md` §6 · code
`src/agent/gateway_authority.rs`, `src/agent/http.rs` (`ae_preflight`), `mycelium-py/src/mycelium/a2a.py`,
`mycelium-ts/src/a2a.ts` · lock-order row 43.

**Finding.** The gateway always bound `mandate: None`, so a mandate-requiring policy rule was undecidable at every door
(`/mcp`, `/a2a`, federation). All three pass through `ae_preflight`, which is the one wiring point.

**Change.**
- `ExecutionAuthority` wraps A1's gate, P2's verifier and the revocation view.
- `ae_preflight` assesses `params._meta.mandate`: holder = authenticated caller; P2; possession bound to operation,
  resource before `@` and arguments digest; A1 at dispatch. It binds `Established`/`Refused`/`Unknown` and clamps
  the envelope window.
- SDK parity: `mandate=`, plus digest and request-bytes helpers, with golden vectors identical in Rust, Python and
  TypeScript.
- An end-to-end `/a2a` test: without a grant the skill is never reached; with one it is; after revocation it is not.

**Kept honest.** Route-level, with H7 closing the walk-around. The live member view needs `compliance`. The SDKs do not
sign.
