## [2026-11-13] ingest | The built-in `llm.invoke` provider runs the provider check (#781's review, F1)

`docs/plans/identity-and-lifecycle.md` row I4, F1; `docs/guide/deprecations.md` §105.

- `handle_llm_invoke` (`src/agent/llm.rs`) verified the caller context and went to the backend. It now calls
  `provider_enforcement::check` straight after, before the parse, the registry lookup and the template read, holds
  the admission to the end of the call, and races the backend against `Admission::lapsed()`. A `Waiting` check (a
  seat claim) is awaited on the call's own task, raced against shutdown (`withdrawn`).
- **Five receivers take a protected kind off the mesh and each calls `check`:** `rpc_rx`, the MCP tool loop, the
  bridged MCP loop, the SDK serve stream, the built-in LLM provider. The enumeration: every
  `signal_handlers.register*` call in `src/` and the companions, read against `is_protected_kind`. A sixth
  receiver has to call it too; nothing gates that but the reviewer.
- **`derive` has an `llm.invoke` arm.** Operation `llm.invoke`, resource `prompt:{ns}/{name}@{self}` read from the
  **payload**, arguments `{"input", "context"}` — what the gateway's LLM doors and `mycelium-reason`'s router
  decide on. Under the generic arm the resource was the envelope's claim (decide on one skill, run another; a
  member's own call refused `resource_unnamed`) and the arguments `{"payload_b64"}`, which no mandate presented
  at a gateway could match. It is the kind's derivation, so an application's `rpc_rx` server of `llm.invoke` gets
  it too.
- **The refusal keeps the door's `{error, detail}`**, `error` the stable reason, with `code` and `data` beside
  them; `data.reason` is how a caller tells a refusal from a backend failure. The gateway answers it 403 (429
  `at_capacity`) and the stream as an error event — the stream sent `done` with an empty output for every error
  reply. No gateway give-back.
- **What dropping the backend's future does not stop:** a model endpoint that has begun, and a task a custom
  backend spawned.
- **Testing lore.** A `call:` mandate and a member-held mandate verify a **node's** key, which exists only in the
  member-key view `compliance` builds (`gateway_member_keys` is empty without it): under `tls` alone both read
  `not_verified`. A test that needs neither uses a client principal trusted in `TrustedExternalIssuers`. No CI lib
  line builds `compliance` with `llm`, so the three tests that need both run in a filtered step of their own
  (`--lib --features compliance,llm llm_door_tests`), as `otel,tls span_export` does; without it the
  `test-coverage` job fails on tests that compile in the universe and run nowhere. Without a gateway token the
  gateway's caller is `anonymous`, which holds no mandate.

Pages: `docs/wiki/dev/security.md`, `docs/wiki/dev/owner-signed-records.md` (the `prompts/` row's line),
`docs/features/provider-enforcement.md`, `docs/operations/what-is-proven.md`, rule `provider.enforcement` rev 5.
