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

1. For each row, open each link under **Upstream** and note anything released, stabilised or withdrawn since the last
   review, with its date. A claim no source supports is marked *unverified* until one does; an announcement that is
   not a release is written *announced*. Add a link for any new source you rely on.
2. Re-check the row's **Where Mycelium is** against `main`: each `file:line`, each version in `Cargo.lock`. Code is
   canon: if the line is wrong, fix it before judging the row.
3. Set the row's **State** (below) and its **Next**: what would move it to the following state.
4. Add one line to the [review log](#review-log): the date, who reviewed, and what changed. *No change* is a valid
   line, and it is what a skipped month cannot claim.
5. Upgrades of a dependency for a security advisory are **not** this register's business: `cargo audit` in CI (the
   `audit` job, over every tracked `Cargo.lock`, `.github/workflows/ci.yml:1113`) and the security PATCH releases
   handle those.

**States.** `watch`: nothing to do until upstream moves. `spike`: a dated note in [`spikes/`](spikes/) would settle
it. `plan`: ready to become a plan row; once the row exists, link it here. `adopted`: shipped; cite the PR.
`declined`: decided against, with the reason.

## The rows

Ordered by the priority agreed with the maintainer on 2026-11-11. Each row gives its upstream sources, where Mycelium
is, what adopting it would buy, what it would cost or risk, its state, and what moves it on.

### W1. Build provenance for the node and its capabilities

- **State:** `plan` (no plan row yet).
- **Upstream:** GitHub artifact attestations, SLSA build provenance
  ([docs](https://docs.github.com/en/actions/security-for-github-actions/using-artifact-attestations), available
  now); Cargo's SBOM precursor, `-Zsbom`, unstable
  ([Cargo unstable features](https://doc.rust-lang.org/cargo/reference/unstable.html)); the crates.io
  registry-hardening goal (verifiable mirroring, quarantine), *unverified* end date, see the
  [Rust project goals](https://rust-lang.github.io/rust-project-goals/).
- **Where Mycelium is:** `.github/workflows/release.yml:15` publishes `SHA256SUMS` and says *the sums are integrity,
  not provenance*. `cargo audit` runs over every tracked lockfile (`ci.yml:1113`). On the capability side, the
  artifact library already has signed manifests, fuel by publisher (D19) and proposed → shadow → accept (D20).
- **What it buys:** a customer can check what produced the binary or image they run, from which commit, with which
  dependencies, and (for a capability) which approval admitted it. It makes the customer-control claim checkable.
- **Cost and risk:** low for attestations on the release binaries and the `ghcr.io` image. Linking an installed
  capability to its build record is design work in the artifact library.
- **Next:** a plan row (attestations on every `v*` tag, verified in the release runbook); link it here, then
  `adopted` with its PR.

### W2. The official Rust MCP SDK (`rmcp`)

- **State:** `spike`.
- **Upstream:** [`rmcp`](https://github.com/modelcontextprotocol/rust-sdk), the official SDK: client and server,
  version negotiation, streamable HTTP transport. Long-running task support: *unverified*.
- **Where Mycelium is:** a custom bridge with no `rmcp` dependency:
  - `src/agent/mcp.rs`: the server and client roles;
  - `src/agent/mcp_handle.rs`: the `McpHandle` API;
  - `src/agent/http/mcp_bridge.rs`: the `/mcp` door;
  - the SDKs' clients, `mycelium-py/src/mycelium/mcp.py` and `mycelium-ts/src/mcp.ts`.
- **What it buys:** protocol conformance inherited rather than maintained; easier connection to customers' existing
  MCP clients and servers.
- **Cost and risk:** most of Mycelium's own logic sits at that boundary: the action evaluator, the grant claim and
  its give-back, the caller envelope, `_meta.correlation`/`_meta.parent`, and the evidence record. The question is
  whether `rmcp` can sit under those without moving them. A dependency on a fast-moving protocol crate also widens
  the advisory surface.
- **Next:** a spike: run a conformance suite against both implementations behind the existing boundary, and list
  every door check that would have to move. Then `plan` or `declined`. Swap nothing before that note exists.

### W3. Telemetry beyond the record spans

- **State:** `watch`.
- **Upstream:** [OpenTelemetry Rust](https://opentelemetry.io/docs/languages/rust/): metrics and logs stable, traces
  beta (its status page, read 2026-11-10); Tokio runtime diagnostics. A Tokio flight recorder called *dial9* was
  proposed to us on 2026-11-10; we could not find a source for it, so it stays *unverified* until one is.
- **Where Mycelium is:** the record spans already ship (adopter-surface Y4, v2.39.0).
  - Under feature `otel`, `src/span_export.rs` exports each decision record and evidence record as an OTLP span
    (`otlp::export`, `src/span_export.rs:422`). The span carries the X4 correlation id, the parent, the record's
    content hash, the rule, its revision and the outcome.
  - The spans are expressly not evidence: guarantee `trace.spans_not_evidence`, and the page
    [`docs/features/span-export.md`](../features/span-export.md).
  - The embedder drives the export (`SpanCollector`, `src/span_export.rs:270`). The node binary does not.
  - There are no request, dispatch or scheduler spans.
  - `opentelemetry` is 0.27 (`Cargo.toml:157-159`), several versions behind.
- **What it buys:** the remaining gap is the other half: an operator following one call through the doors, the
  dispatch and the scheduler in the tools they already run, and finding queueing delays that aggregate metrics hide.
- **Cost and risk:** the bump from 0.27 is an API migration. **The line we keep:** telemetry is sampled and lossy, so
  it never substitutes for the evidence journal or the execution record; a span cites a record, it is not the record.
- **Next:** `spike` when an adopter asks for door- or dispatch-level spans, or the node binary should export by
  itself; the 0.27 bump goes with either.

### W4. WASI 0.3 and asynchronous components

- **State:** `watch`.
- **Upstream:**
  - WASI 0.3.0, ratified 2026-06-11 ([wasi.dev](https://wasi.dev/releases/wasi-p3),
    [Bytecode Alliance](https://bytecodealliance.org/articles/WASI-0.3)): native `async func`, `stream<T>` and
    `future<T>`, with `wasi:io` gone. Wasmtime 46 ships 0.3 with component-model async on by default.
  - 0.3.1 (`map<K, V>`), said to be adopted 2026-08-06: *unverified*.
  - The Rust goal to bring `wasm32-wasip3` to tier 2 ([goal](https://goals.rust-lang.org/2026/wasm-components.html)):
    not confirmed stable as of 2026-11-10.
- **Where Mycelium is:**
  - The host links WASI 0.2 synchronously (`mycelium-wasm-host/src/host.rs:454`,
    `wasmtime_wasi::p2::add_to_linker_sync`), on wasmtime 49 (`mycelium-wasm-host/Cargo.toml:26`).
  - Guest calls run off the async workers, under fuel, a call deadline and a memory cap.
- **What it buys:** capabilities that stream through several components while Mycelium controls their permitted
  external actions; portable, composable capability packages.
- **Cost and risk:**
  - wasmtime has driven more of Mycelium's security releases than any other dependency. Its advisories are behind
    v2.4.1, v2.18.2, v2.36.1 and v2.38.3, and behind v2.38.1 through wasmtime's `cache` feature; the MINOR v2.34.0
    carried more (CHANGELOG).
  - Of v2.38.3's RUSTSEC-2026-0420, -0422, -0423 and -0424, one (-0420) affects only WASI 0.3 sockets.
  - Async components change the confinement model: streams that cross components need new answers for the fuel
    budget, the call deadline and the memory cap. This is interface and host design, not a dependency bump.
- **Next:** `spike` when `wasm32-wasip3` is tier 2 on stable. The spike's question: what fuel, deadline and memory
  mean for a stream that outlives the call that opened it.

### W5. Hybrid post-quantum key exchange in TLS

- **State:** `watch`.
- **Upstream:** rustls offers `X25519MLKEM768` through its `aws-lc-rs` provider (in core rustls since 0.23.22; the
  `prefer-post-quantum` feature makes it the preferred group rather than enabling it) —
  [provider source](https://docs.rs/crate/rustls/latest/source/src/crypto/aws_lc_rs/mod.rs). No evidence the `ring`
  provider has it.
- **Where Mycelium is:**
  - `ring` is the provider Mycelium installs: `mycelium-core/Cargo.toml:65`, and `ensure_crypto_provider` at
    `mycelium-core/src/tls.rs:1122`.
  - But `aws-lc-rs` is already compiled into both release binaries, through reqwest's rustls and through
    `object_store` (`aws-lc-rs 1.18.1` in `Cargo.lock`).
  - The mesh and gateway configs use the process-default provider, which a host that installs one first decides
    (`tls.rs:1120`). So an embedder that installs `aws-lc-rs` first may already negotiate the hybrid group on the
    mesh. That is an inference, not tested.
  - `src/agent/bulk.rs` (628, 643, 707, 733) and `src/federation/pinning.rs:238` name `ring` directly.
- **What it buys:** protection against recording traffic now and decrypting it later, for customers whose data must
  stay confidential for a long time; on the mesh, at the gateway and on the federation edge.
- **Cost and risk:** no new C build (it is there already). The work is one provider choice in place of the direct
  `ring` calls, and identity and revocation tests run under both providers. **The claim stays exact:** post-quantum key
  exchange protects session establishment; it does not make the Ed25519 identity signatures, or the system,
  quantum-resistant.
- **Next:** `spike` when a customer asks: test whether an embedder-installed `aws-lc-rs` provider negotiates the
  hybrid group on the mesh, and list every direct `ring` call that would need to follow it.

### W6. Async memory and dynamically dispatched async traits

- **State:** `watch`.
- **Upstream:** the Rust project goals to shrink async futures and simplify their state machines, and native `dyn`
  async traits ([Rust project goals](https://rust-lang.github.io/rust-project-goals/)). These are goals, not releases.
- **Where Mycelium is:** `rust-toolchain.toml:11` pins 1.96.0; `rust-version` is 1.89 (`Cargo.toml:5`).
- **What it buys:** less memory per concurrent operation; simpler provider interfaces.
- **Cost and risk:** none until a stable compiler ships one. Nothing here replaces the explicit recovery after process
  loss or an uncertain external action.
- **Next:** `spike` when a stable release ships one: benchmark it against the scale suites before changing any
  interface.

### W7. Faster compilation and linking

- **State:** `watch`.
- **Upstream:** the compiler build-performance goals: front-end parallelism, faster development backends, and
  *relink, don't rebuild*, its stabilisation said to be prepared for 2027 (*unverified*;
  [Rust project goals](https://rust-lang.github.io/rust-project-goals/)).
- **Where Mycelium is:** a many-crate workspace; `make check` runs about 3 min, and a cold `make check-full` much
  longer.
- **What it buys:** shorter local and CI cycles. Engineering capacity, not a customer feature.
- **Cost and risk:** a development-only change; production toolchains stay pinned.
- **Next:** `spike` when one ships on stable: measure it on `make check` in a development build.

## Review log

| Date | Reviewer | What changed |
|---|---|---|
| 2026-11-10 | maintainer + Claude | Register started from an external roadmap note, its code claims checked against `main` at `634f655f`. *dial9* unverified. Priority proposed: W1–W7. |
| 2026-11-11 | maintainer | Priority agreed as proposed (W1–W7 in order). |
| 2026-11-11 | independent review (#755) | Two of the first draft's own claims corrected. **W3:** record spans with correlation ids already ship (`src/span_export.rs`, Y4), so its proposed spike was already built; it is now about door and dispatch spans. **W5:** `aws-lc-rs` is already compiled in, so post-quantum TLS needs no new C build. W4's advisory list, the procedure's sources and `file:line`s, and the states made consistent. |
