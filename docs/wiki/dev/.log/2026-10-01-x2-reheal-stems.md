## [2026-10-01] ingest | D22 and the last artifact-shaped demo as stems

**What:** `[[serve]]` (`src/capability_config.rs`, `mycelium-wasm-host/src/serve.rs`, feature `llm`), the
checker counting served skills as offers (`src/wire_check.rs`), the `reheal_deploy` units and suite
profile, and the driver arm routing through `mycelium-reason`'s `InferenceRouter`.

**Durable knowledge:**
- **`serve_model` was two core calls.** `register_prompt_skill` plus an `llm-meta` advertisement the
  router reads only when a query carries constraints. So a stem can serve a model on core's `llm`
  feature, without the reasoning companion as a dependency.
- **Serve while the install is live, never unconditionally.** The skill follows the install named in
  `while_live`: when the activation probe fails and the provisioner withdraws the install, the skill
  is retracted on the next tick, so a router stops sending calls to a node whose model is gone. The
  test gates the activation on a file it controls, so the retraction is observed rather than raced.
- **Two names, one model.** The install (`llm/storyteller-deploy`) and the skill (`llm/storyteller`)
  share a namespace and must not share a key — the code demo learned this from an LWW churn;
  `validate()` now refuses the collision by name.
- **The reheal is the floor.** Two hosts, each with its own Ollama, and `min_providers = 1`: the
  survivor joins late and idles; when the origin dies the floor is unmet, the survivor installs,
  activates into its own Ollama and serves, and the router's next call lands there.

**Pages touched:** plan D22 + row X2, `operations/capability-lifecycle.md`, CHANGELOG.
