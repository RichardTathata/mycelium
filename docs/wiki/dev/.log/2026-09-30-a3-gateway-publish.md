# 2026-09-30 — A3: the gateway publish door

**What:** `docs/plans/design-time-tooling.md` §10 A3. `POST /gateway/artifacts/publish` in
`mycelium-wasm-host` (feature `gateway`, `src/gateway.rs`, `artifact_router`), merged through
`with_http_routes`; scope family `artifact:publish` in `required_scope` (`src/agent/http.rs`);
the stem binary mounts it from `[hosts].trusted_publishers`; SDK verbs `artifacts().publish`.

**Durable knowledge:**
- **The route lives in the companion, the scope row in core.** `mycelium` cannot depend on the
  wasm-host crate (the codec is there), so the handler is a companion router like the wiki's, but
  `required_scope` is an exact-path table in core with `admin` as the fallback — a companion route
  still needs its row there or it is deny-by-default. Both halves, or the door does not open.
- **A door, not the door.** A bearer holding `kv:write` can write `installable/` through
  `POST /gateway/kv` today (no reserved-prefix guard — *detection, not prevention*). The route adds a
  checked path; the defence that holds in every case is the provisioner's own `require_provenance`.
  Said in rbac.md rather than hidden.
- **A librarian tombstones what its manifest does not carry.** For every key it manages, the
  librarian diffs the KV catalogue against its manifest every sync interval and tombstones the
  rest — so a line published through the gateway under a librarian-managed key would vanish within
  seconds. The route refuses that by name (409) rather than letting it happen.
- **The SDKs carry the line, they do not sign it** (Q5, recorded in the plan): client-side signing
  would duplicate `Capability::encode` and the provenance message in two more languages, each a
  codec that can drift from the one the provisioner verifies. `mycelium-artifact` in CI is the signer.

**Pages touched:** rbac.md (scope table), `dev/security.md`, plan row A3 + Q5 (rev 0.9), CHANGELOG,
both SDK READMEs, `mycelium-wasm-host/README.md`, `operations/artifacts.md`.
