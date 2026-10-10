# mycelium-wasm-host

New developers: start with [Your first stem fleet](../docs/guide/tutorials/01-first-stem-fleet.md), then the [tutorial sequence](../docs/guide/tutorials/README.md).

Use the [capability map](../docs/capabilities.md) to connect this material to other mechanisms, operating guidance and evidence.


WASM Component host for Mycelium — the **install mechanism** (M12) plus the **catalog
selection step** (M15) of the autonomic-provisioning chain (v2.0 **WS-E** M12 → M15 → M14;
see [`docs/plans/v2.0.md`](../docs/plans/v2.0.md) §WS-E).

A capability can be shipped as a sandboxed **WASM component**, pulled by content address,
instantiated here, and made to provide a capability on the local node — the OSGi-bundle
analogue Rust natively lacks. This crate is built **entirely on Mycelium's public API**;
`wasmtime` is confined to it and never leaks into `mycelium` / `mycelium-core` (the WS-A
dep-tree invariant — same companion-crate posture as `mycelium-tuple-space`).

## The host ⇄ component boundary

A component runs **sandboxed, in-process** (wasmtime). It touches the node **only** through the
WIT world in [`wit/host.wit`] — in-process FFI over the Component-Model canonical ABI, *not* a
socket:

- **Host imports** (what a component may call) — a *capability-scoped projection* of Mycelium's
  public handles: confined `kv`, confined `mesh` emit, `log`. Component KV is confined to
  `comp/{node}/{namespace}/…`; a component can never escape its subtree or reach the capability
  registry. A component's `mesh.emit` is confined to kinds under `comp/{namespace}/…` (or kinds
  the host listed for it) and never a protected RPC kind — `mcp.invoke`, `skill.invoke`,
  `llm.invoke`, the node's `protected_rpc_kinds` — since an emit from inside the process would
  reach that kind's handlers unframed, past the door that checks authority; a refused emit is
  dropped, logged and counted (`mycelium_wasm_host_emits_refused_total`). This is the one place the substrate's *detection-not-prevention* posture flips to
  genuine **prevention** — the guest is untrusted foreign code in the node's own process, so the
  host mediating every import is legitimate. The host also provides a **restricted, deny-by-default
  WASI** context (no filesystem, network, env, or inherited stdio) — std-based guests link `wasi:*`
  at init, but their only real doors are the scoped imports above.
- **Component export** `handle(request) -> response` — the capability entry point the host calls
  on an inbound invocation.

WIT imports = the component's *requires*; WIT exports = its *provides* (M15's one-hop contract):
a component imports the **mesh**, not other capabilities — a call to another skill is
runtime-mesh-resolved, never link-time-bound into a deployment set.

## Status

Shipped. The host, the catalog selection step, the provisioner and supervision (WS-E, v2.0) are
complete; the stem node, the artifact tool, object stores, fuel by publisher and
proposed → shadow → accept shipped in v2.17.0; declared activation (`[[activation]]`) and declared
model serving (`[[serve]]`) in v2.18.0. The sections below record what landed, in the order it
landed.

## The stem, the artifact tool and object stores

| Feature | What it adds |
|---|---|
| `stem` | the binaries `mycelium-stem` (a node that declares and provisions from a unit file) and `mycelium-artifact` (`publish \| list \| verify \| accept` over a library) |
| `gateway` | `POST /gateway/artifacts/publish` (scope `artifact:publish`), mounted by the stem when its `[hosts]` names trusted publishers |
| `llm` | `[[serve]]`: a stem registers a routable model skill while a local install is live; without it a stem with a `[[serve]]` section refuses to start |
| `object_store` | `ObjectStoreFetcher`: a library in `s3://`, `gs://`, `az://`, `https://` or `file://` for `mycelium-artifact` and the librarian API |

```bash
cargo build -p mycelium-wasm-host --features stem,gateway,llm --bin mycelium-stem
mycelium-stem --units examples/units/model_deploy/model-host.toml --library /lib \
              --host 172.40.0.42 -p 57000 -r 172.40.0.40:57000
```

- **Every section of the unit file**, including `[[activation]]` (a command run after a blob is
  placed, gated by a probe; a failing initial probe is an activation error, and both record on the
  decision trace under the install's token — `ActivationCtx`, the activation hook's third argument
  since 2.21.0) and `[[serve]]`: [`docs/reference/unit-file.md`](../docs/reference/unit-file.md).
- **Running a stem**, its flags and what it needs on the node:
  [`docs/operations/capability-lifecycle.md`](../docs/operations/capability-lifecycle.md) §3. The byte
  source is one flag with three answers — the mesh (none), a directory (`--library /lib`), an object
  store (`--library s3://bucket/prefix`, feature `object_store`) — every one staged to disk and verified,
  a blob past the frame cap in ranges; a librarian over a store takes `--manifest-source <url>`. A keyed
  `[[serve]]` names its variable (`api_key_env`), read once at start.
- **Publishing, verifying and accepting entries; object stores and their limits:**
  [`docs/operations/artifacts.md`](../docs/operations/artifacts.md).

An activation command is the operator's, run with the stem's privileges. It is not a sandbox.

## What landed (history)

**Landed:** crate scaffold; the WIT contract; `confine` (the enforcement point, unit-tested);
`HostState` scoped operations proven against a live node; restricted WASI; `bindgen!` host-import
impls; the `WasmHost::instantiate` / `Instance::invoke` path; **pull + verify + instantiate**
end to end — `ArtifactId` (content address = SHA-256), `verify_artifact` (run before the engine
ever sees the bytes), pluggable untrusted [`ArtifactSource`] (`InMemorySource`); and a
**real-guest end-to-end test** (`tests/e2e.rs`) — an actual WASM component (built from
`tests/fixtures/echo-component/`, committed as `tests/fixtures/echo_component.wasm`) is
instantiated, invoked, and its `kv` import is observed crossing into the confined subtree. The
`.wasm` is committed so CI needs no wasm toolchain; regenerate with the fixture's `build.sh`.

**M15 selection (landed):** `InstallableCatalog` / `InstallableEntry` (`src/catalog.rs`) resolve a
requirement against installable artifacts using the **same `CapFilter::matches`** the live resolver
uses — pointed at each entry's declared-provide `Capability` instead of a running `cap/` entry —
and pick the cheapest match. `WasmHost::provision_for(catalog, filter, source, state)` ties it to
M12: resolve → pull → verify → instantiate (`Ok(None)` if nothing satisfies the requirement). This
is **one hop, not a constraint solver** by design — *service* dependencies are runtime-mesh-resolved
(a component imports the mesh), never frozen into an install closure.

**Provisioner (landed — the loop closes):** `Provisioner` (`src/provisioner.rs`) is the app-layer
agent that watches demand, resolves unmet requirements against the catalog, and pulls + verifies +
instantiates + advertises — relieving demand. `provision_round()` is the testable convergence pass;
it self-elects probabilistically (herd damping) and is idempotent. **Core Principle 1:** it is a
regular agent on the public API, never a substrate mechanism — no coordinator assigns provisioning
duty; each node runs its own and self-elects. The full autonomic loop (declare requirement → demand
→ provision → advertise → demand relieved) is proven by `provisioner::tests`.

**Serve path (landed — provisioned capabilities are callable):** when the provisioner brings a
capability live it registers an RPC handler (`cap_invoke_kind(ns, name)`) and spawns a serve task
that owns the component instance (wasmtime stores are single-threaded → one task per instance) and
routes each inbound invocation to the component's `handle`, replying with its output. A caller
resolves the capability to a provider, then `rpc_call(provider, cap_invoke_kind(ns, name), …)`.

**M14 supervision (landed):** `Provisioner::supervise(filter, min_providers)` adds a
capability-presence invariant — keep ≥ `min_providers` live providers of `filter` alive. The same
`provision_round` reconciles it: a freshness-aware provider count below the floor triggers a
catalog resolve + `bring_live` — with **no organic demand**. Self-healing falls out for free: a
crashed provider's `cap/` entry evaporates, the count drops, the invariant re-provisions —
**restart and first-time provisioning are the same resolve-and-pull path**.

**Gossip-backed catalog (landed):** `publish_installable(kv, entry)` writes an installable artifact
to the cluster-wide `installable/{ns}/{name}/{artifact-hex}` KV prefix (declared-provide `Capability`
+ `ArtifactId` + cost, framed on the public `Capability::encode`); `InstallableCatalog::from_kv(kv)`
rebuilds the catalog from the gossiped view. So any node publishes artifacts and every provisioner
resolves against the live cluster catalog — no embedder-supplied in-memory list required.

**Fuel metering (landed):** `WasmHost::with_fuel_per_call(n)` grants each `invoke` a budget of `n`
wasm instructions; a runaway component is **stopped** (`WasmHostError::FuelExhausted { budget }`)
instead of hanging the serve task. Instantiation runs with unlimited fuel so only `invoke` is
bounded; `WasmHost::new()` stays unmetered (zero overhead). **Fuel by publisher (D19, plan F1):**
`WasmHost::metered()` counts fuel without a default budget, and `Provisioner::set_fuel_policy`
(`FuelPolicy`) decides per entry from its *verified signer* — an operator key's entries run under
`operator_budget` (absent = unbounded), every other trusted key is an agent principal and runs
under `agent_budget`. A stem reads this from `[hosts]`: `fuel_per_call` (the agent budget),
`operator_publishers` (a subset of `trusted_publishers`, refused otherwise — without provenance a
signer is a claim), `operator_fuel_per_call`. Every hosted call is recorded
(`Provisioner::invocations()`, `InvocationOutcome::FuelExhausted { budget }` by name; counter
`mycelium_artifact_invocations_total{outcome}`); a stopped call leaves the install live because the
trapped instance is **replaced** from the verified bytes (a trap poisons a component instance — found by
the gate). Gate:
`an_agent_published_entry_that_loops_is_stopped_at_its_budget_and_the_operators_is_not` over the
committed `spin_component.wasm` fixture (a guest that never returns).

**Execution limits (row D, landed):** every engine has **epoch interruption** on, and every call — and
every instantiation, which runs the guest's start-up code — is bounded in wall-clock time:
`DEFAULT_CALL_DEADLINE` (5 s) unless `WasmHost::with_call_deadline(Some(d))` (`None` = unbounded); a
stem reads `[hosts].call_deadline_ms`. Unlike fuel it needs no metered engine and applies to the
operator's entries too. A call past it returns `WasmHostError::DeadlineExceeded { deadline_ms }`,
recorded as `InvocationOutcome::DeadlineExceeded` (counter outcome `deadline_exceeded`), and the trapped
instance is replaced like any other — unless the install was uninstalled while the call ran, in which
case nothing is re-instantiated. Start-up past the deadline is `DeadlineExceeded` too; `Duration::ZERO`
stops the guest at its first epoch check; a deadline too large to count in ticks is no deadline. The epoch is advanced every `EPOCH_TICK` (10 ms) by one thread per
host (`mycelium-wasm-epoch`, holding a weak engine reference so it ends with the engine), so a call stops
between `d` and `d + 10 ms` after it starts. The serve loop runs each guest call — and install's
compile, instantiation and `describe` — on `tokio::task::spawn_blocking`, so a long call holds a
blocking-pool thread, never a runtime worker; `Instance::invoke` itself stays synchronous, and an
embedder calling it from async code does the same (the co-op `catalog`, `catalog_viz` and
`mcp_toolgrowth` demos show the pattern). A trapped instance is replaced from the
install's **compiled** component (`WasmHost::compiles()` counts compiles), so a payload that makes a
guest trap no longer buys a full compile per request. Gates: `a_guest_call_past_its_deadline_is_stopped_by_name`
(`tests/e2e.rs`), `a_long_guest_call_does_not_block_another_task_on_a_current_thread_runtime`,
`a_trapping_guest_called_repeatedly_compiles_once` (`src/provisioner.rs`). Not bounded: a call's
memory beyond `DEFAULT_MEMORY_LIMIT_BYTES`, and a host import that blocks (the kv/mesh/log imports do
not); a deadline is wall time, so whether a call reaches it is not replayable (fuel is).

**Proposed → shadow → accept (D20, plan F2):** a description with `proposed = true` publishes an entry
that `Provisioner` loads only into the **shadow lane** — installed and advertised as `{ns}/{name}.shadow`,
callable by name for comparison, never resolved by the incumbent's filter, taking no demand and keeping
no presence floor — until a key in `require_reviewers` co-signs it (`mycelium-artifact accept <library>
<ns/name> --key <reviewer-seed>`; `verify --reviewer` names a forged or unlisted acceptance). The
acceptance is an Ed25519 signature over the publisher's signed content *and* signature; the line is
rewritten in place (same KV key). An entry with neither field still encodes as v1 byte for byte.
**The gateway door (A3, feature `gateway`):** `artifact_router(agent, trusted, librarian_publisher)`
mounts `POST /gateway/artifacts/publish` (scope `artifact:publish`) through `with_http_routes` before
`start()`; the body is one already-signed manifest line (`{"entry_hex": …}`), verified against
`trusted` and written to `installable/`. Refusals by name: 403 unsigned / untrusted / does-not-verify /
no-trusted-configured, 409 for a librarian-managed signer, 400 malformed. The stem binary mounts it
from `[hosts].trusted_publishers`. Gate: `tests/gateway.rs`.

**Provenance (landed):** content-addressing gives *integrity* (the bytes are what the catalog
named); `InstallableEntry::signed_by(key)` adds *provenance* — an Ed25519 signature over the
content address by a publisher. `Provisioner::require_provenance(trusted_keys)` then installs only
artifacts a trusted publisher vouched for (unsigned / untrusted-signer / swapped-artifact refused).

**Mesh artifact pull (landed):** `serve_artifacts(agent, source)` answers `artifact.fetch` RPCs
with bytes by content address; `pull_artifact(agent, peer, id, …)` / `MeshArtifactSource` pull from
a peer and **verify the content address on arrival** (untrusted source). So nodes distribute
artifacts to each other over the cluster — no external registry required (RPC frame ≤ 10 MiB;
larger artifacts want the bulk transport). Closes §E.4.4 on the public API.

**WS-E is complete.** Two further ideas were considered and **deliberately deferred** — neither is
a gap in what the host does today:

- **Epoch / wall-clock invocation limit** — *not needed yet.* `fuel` already traps any runaway
  *compute*; the only thing it can't see is time spent *inside a host call*, but every host import
  (`kv`/`mesh`/`log`) is fast, synchronous, and non-blocking. Epoch becomes worthwhile only if a
  **blocking** host import is ever added (e.g. guest-triggered outbound I/O) — add it just-in-time
  then, not speculatively now.
- **Strict (leased-consensus) singleton** — *out of grain, niche.* The shipped probabilistic
  self-election + Track-2b shed gives **eventual-single** (converges to ~1 provider, self-corrects
  overshoot, re-provisions on death) — adequate for any idempotent/stateless or "about-one"
  capability. *Strict* exactly-one needs an async consensus election/lease (a `provision_round`
  async refactor) and trades liveness for safety under partition — the substrate's AP/convergence
  grain. Worth it only for a capability where two simultaneous providers is actively harmful (an
  exclusive external resource); opt in then via `consensus().elect_leader` / `distributed_lock`.

[`ArtifactSource`]: src/artifact.rs

## Build / test

```bash
cargo test  -p mycelium-wasm-host
cargo clippy -p mycelium-wasm-host --all-targets -- -D warnings
```

[`wit/host.wit`]: wit/host.wit
