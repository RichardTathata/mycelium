## [2026-11-09] ingest | the published stem gets `compliance`; `gateway_a2a` mounts `/a2a` from configuration

`dev/operations.md` records the published stem's feature set as `stem,gateway,llm,object_store,compliance`:
`mycelium-wasm-host`'s new `compliance` passes through to mycelium's, so a stem takes a token table, `[oidc]` and
`[authority]`, and `start()` attaches the authority as on the node (the stem calls `start()`; nothing bypasses it).
`GossipConfig::gateway_a2a` makes `start()` mount the card and `/a2a` (once, beside `with_a2a()` too); a build without
`a2a` refuses it at start, and the warning, `a2a.admission` and `mycelium doctor` read the switch before the mount
(`TaskCtx::a2a_served`). Feature pages `docs/features/a2a.md`, `docs/features/stem.md`; configuration row
`gateway_a2a`; upgrade note `deprecations.md` §92.
