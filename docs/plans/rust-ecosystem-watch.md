# Rust ecosystem watch

**Status:** a recurring register, not a build plan (started 2026-11-10). Once a month someone reads the upstream
sources below against `main`, updates each row, and adds a line to the review log at the end. A row that is ready to
build becomes a row in a real plan (or a spike under [`spikes/`](spikes/)), and this file links to it. Nothing here is
a commitment: an upstream date is the upstream's, not ours.

**Why it exists.** Mycelium's distinctive work is governance: authority at every door, evidence, receipts, replay.
Every hour spent following a protocol, a runtime or a toolchain by hand is an hour not spent there. This register asks
one question each month: *has the Rust ecosystem shipped something that lets Mycelium inherit work it does today by
hand, or offer a customer something it cannot today?* It also records what we decided not to adopt, and why, so the
same assessment is not redone from scratch.

## How to run the monthly review

1. For each row, read its **upstream sources** and note anything released, stabilised or withdrawn since the last
   review. Quote the source and its date; an announcement that is not a release is written *announced*.
2. Re-check the row's **where Mycelium is** line against the code (`file:line`, the version in the lockfile). Code is
   canon: if the line is wrong, fix it before judging the row.
3. Set the row's **state**: `watch` (nothing to do yet) · `spike` (a dated note in `spikes/` would settle it) ·
   `plan` (ready to become a plan row; link it) · `adopted` (shipped; cite the PR) · `declined` (with the reason).
4. Add one line to the [review log](#review-log): the date, who reviewed, and what changed. *No change* is a valid
   line, and it is what a skipped month cannot claim.
5. Upgrades of a dependency for a security advisory are **not** this register's business: `cargo audit` in CI
   (`.github/workflows/ci.yml`, every tracked lockfile) and the security PATCH releases handle those.

## The rows

Ordered by the priority proposed on 2026-11-10 (the maintainer may reorder). Each row states what Mycelium has today, what upstream offers, what
adopting it would buy, what it would cost or risk, and what would move it to the next state.

### W1. Build provenance for the node and its capabilities — `plan`

- **Upstream:** GitHub artifact attestations and SLSA build provenance (available now); Cargo's SBOM precursor
  (`-Zsbom`, experimental); the crates.io registry-hardening goal (verifiable mirroring, quarantine), into early 2027.
- **Where Mycelium is:** `.github/workflows/release.yml` publishes `SHA256SUMS` and says in its own comment that the
  sums are *integrity, not provenance*. `cargo audit` runs over every tracked lockfile. On the capability side, the
  artifact library already has signed manifests, fuel by publisher (D19) and proposed → shadow → accept (D20).
- **What it buys:** a customer can check what produced the binary or image they run, from which commit, with which
  dependencies, and (for a capability) which approval admitted it. It makes the customer-control claim checkable.
- **Cost and risk:** low for attestations on the release binaries and the `ghcr.io` image. Linking an installed
  capability to its build record is design work in the artifact library.
- **Next:** the cheapest item on this page; ready to become a plan row.

### W2. The official Rust MCP SDK (`rmcp`) — `spike`

- **Upstream:** `rmcp` (the `modelcontextprotocol/rust-sdk` repository): client and server, version negotiation,
  streamable HTTP transport, long-running tasks.
- **Where Mycelium is:** a custom bridge, `src/agent/mcp.rs` (server and client roles) and
  `src/agent/http/mcp_bridge.rs` (the `/mcp` door); no `rmcp` dependency.
- **What it buys:** protocol conformance inherited rather than maintained; easier connection to customers' existing
  MCP clients and servers.
- **Cost and risk:** most of Mycelium's own logic sits at that boundary: the action evaluator, the grant claim and
  its give-back, the caller envelope, `_meta.correlation`/`_meta.parent`, and the evidence record. The question is
  whether `rmcp` can sit under those without moving them. A dependency on a fast-moving protocol crate also widens
  the advisory surface.
- **Next:** a spike: run a conformance suite against both implementations behind the existing boundary, and list
  every door check that would have to move. Swap nothing before that note exists.

### W3. Telemetry that correlates with the evidence — `spike`

- **Upstream:** OpenTelemetry Rust (metrics and logs stable, traces beta, per its status page); Tokio runtime
  diagnostics. A Tokio flight recorder called *dial9* was proposed to us on 2026-11-10; we could not find a source
  for it, so it stays unverified until one is.
- **Where Mycelium is:** OTLP export exists only behind feature `otel`, only for `SkillRunner` spans, on
  `opentelemetry` 0.27 (`Cargo.toml`), several versions behind. Every record already carries one correlation id and
  one parent (adopter-surface X4), read back by `mycelium explain --correlation/--closure`.
- **What it buys:** an operator can follow one call from request to authorisation, dispatch, execution and recovery
  in the tools they already run, and find queueing and scheduler delays that aggregate metrics hide.
- **Cost and risk:** the bump from 0.27 is an API migration. **The line we keep:** telemetry is sampled and lossy, so
  it never substitutes for the evidence journal or the execution record; a span cites a record's correlation id, it
  is not the record.
- **Next:** a spike: export the X4 correlation and parent ids as span attributes on the existing doors, and measure
  the cost.

### W4. WASI 0.3 and asynchronous components — `watch`

- **Upstream:** WASI 0.3.0 ratified 2026-06-11 (native `async func`, `stream<T>`, `future<T>`; `wasi:io` gone);
  0.3.1 adopted 2026-08-06 (`map<K, V>`); wasmtime enables 0.3 by default from 46; the Rust goal to bring
  `wasm32-wasip3` to tier 2 (not confirmed stable as of 2026-11-10).
- **Where Mycelium is:** the host links WASI 0.2, synchronously (`mycelium-wasm-host/src/host.rs`,
  `wasmtime_wasi::p2::add_to_linker_sync`), on wasmtime 49. Guest calls run off the async workers, under fuel, a call
  deadline and a memory cap, with one `Store` per instance.
- **What it buys:** capabilities that stream through several components while Mycelium controls their permitted
  external actions; portable, composable capability packages.
- **Cost and risk:** wasmtime is the dependency behind the most security releases here (v2.34.0, v2.36.1 and v2.38.3,
  2026-10-16 to 2026-10-30), and one of the advisories in RUSTSEC-2026-0420..0424 affects only WASI 0.3 sockets. Async
  components change the confinement model: streams that cross components need new answers for the fuel budget, the
  call deadline and the memory cap. This is interface and host design, not a dependency bump.
- **Next:** move to `spike` when `wasm32-wasip3` is tier 2 on stable. The spike's question: what fuel, deadline and
  memory mean for a stream that outlives the call that opened it.

### W5. Hybrid post-quantum key exchange in TLS — `watch`

- **Upstream:** rustls offers `X25519MLKEM768` through its `aws-lc-rs` provider (built into rustls since 0.23.22,
  with `prefer-post-quantum`). We found no evidence the `ring` provider has it.
- **Where Mycelium is:** `ring` is pinned on purpose: `rustls` with `default-features = false, features = ["ring"]`
  (`mycelium-core/Cargo.toml`), installed once in `mycelium-core/src/tls.rs` (`ensure_crypto_provider`).
- **What it buys:** protection against recording traffic now and decrypting it later, for customers whose data must
  stay confidential for a long time; on the mesh and at the gateway.
- **Cost and risk:** `aws-lc-rs` brings a C and cmake build into the release binaries for three platforms. It must
  keep identity and revocation behaviour: test both providers. **The claim stays exact:** post-quantum key exchange
  protects session establishment; it does not make the Ed25519 identity signatures, or the system, quantum-resistant.
- **Next:** offer as a feature-gated crypto profile when a customer needs it, or when `ring` gains the group.

### W6. Async memory and dynamically dispatched async traits — `watch`

- **Upstream:** Rust project goals to shrink async futures and simplify their state machines; native `dyn` async
  traits.
- **Where Mycelium is:** `rust-toolchain.toml` pins 1.96.0; `rust-version` is 1.89.
- **What it buys:** less memory per concurrent operation; simpler provider interfaces.
- **Next:** when a stable compiler ships one, benchmark it against the scale suites before changing any interface.
  Nothing here replaces the explicit recovery after process loss or an uncertain external action.

### W7. Faster compilation and linking — `watch`

- **Upstream:** the compiler build-performance goals (front-end parallelism, faster development backends, *relink,
  don't rebuild*, its stabilisation prepared for 2027).
- **What it buys:** shorter local and CI cycles for a many-crate workspace. Engineering capacity, not a customer
  feature.
- **Next:** try in development builds first; production toolchains stay pinned.

## Review log

| Date | Reviewer | What changed |
|---|---|---|
| 2026-11-10 | maintainer + Claude | Register started from an external roadmap note, its code claims checked against `main` at `634f655f`. Two corrections to the note: OTLP export is `otel`-gated `SkillRunner` spans only, not node-wide; *dial9* unverified. Priority proposed: W1, W2, W3, W4, W5. |
