## [2026-10-03] ingest | zero gaps Z2 — `[[serve]].api_key_env`

**What:** `ServeDecl.api_key_env`; `validate()` refuses both forms or an empty name; `stem::resolve_serve_key`
runs once at `Stem::start` before `serve::spawn`, which now takes `(ServeDecl, Option<String>)`; an unset
variable is `StemError` naming it. Reference, lifecycle page and the proof page updated. Plan D2.

**Durable knowledge:** resolve a secret at the lifecycle boundary, not in the tick loop — the loop
re-registers a skill each time its install comes back, and a key read there would be read again (and
could go missing between two reads); resolving at start makes "unset" a refusal the operator meets
immediately, the same shape as every other fail-closed start since v2.18.1.
