## [2026-10-01] ingest | D21: a placed blob's activation is declared

**What:** `[[activation]]` in the unit file (`src/capability_config.rs`), `BlobRuntime::with_entry_activation`
(`mycelium-wasm-host/src/runtime.rs`), the declarative hook and re-probe task
(`mycelium-wasm-host/src/activation.rs`), wired by the stem. Two stem tests; lock-order row 51.

**Durable knowledge:**
- **The hook existed; the declaration did not.** `BlobRuntime::with_activation` / `with_probe` were
  already how the model demo ran `ollama create`, but they are code, keyed on a path. A stem is
  configured by a unit file, so it could place a model and never load it. The new hook sees the whole
  entry (capability and content address), which is what a declaration can key on.
- **Probe off the lock, read a flag on it.** The provisioner calls `Installed::probe` under its hosted
  lock every round; a command there would stall the round. The activation returns a per-install
  `AtomicBool`, a background task re-runs the declared probe and flips it, and the install's probe
  reads it. A dead install's flag is dropped from the re-probe list when nothing else holds it.
- **Ordering by retry, again.** `resolve_artifact_refs` makes the model demo's `FROM artifact:<hex>`
  rendering declarative; a reference not yet placed fails the activation, and the next round retries
  — the same mechanism the code demo used, now available to a stem.

**Pages touched:** plan D21 + row X2, `operations/capability-lifecycle.md`, CHANGELOG, lock-order row 51.
