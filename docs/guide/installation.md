# Installation and integration modes

Choose how your application joins the mesh before selecting an example. These are deployment
choices over the same substrate, not different architecture layers.

| Mode | What you deploy | First working path | Operations |
|---|---|---|---|
| Rust embed | Your application containing `GossipAgent` | [Integrator contract](building-on-mycelium.md), then `cargo run --example hello_mesh` from a source checkout | [Deployment](../operations/deployment.md) |
| Python or TypeScript | Your application plus a Rust node exposing the HTTP gateway | [Language bridges](10-language-bridges.md), [Python SDK](../../mycelium-py/README.md), [TypeScript SDK](../../mycelium-ts/README.md) | [Gateway TLS](../operations/gateway-tls.md) (the SDKs reach it with `scheme="https"` / `{ scheme: "https" }` — [bridges § TLS](10-language-bridges.md#reaching-a-gateway-over-tls)), [readiness](../operations/production-readiness.md) |
| Published binary or image | The released `mycelium` node (and `mycelium-stem`), downloaded or pulled — no Rust toolchain | [The download block below](#the-published-node), then an SDK README | [Deployment](../operations/deployment.md#get-the-node) |
| Generic stem host | `mycelium-stem`, unit declarations and configured artifact/runtime dependencies | [Wasm host](../../mycelium-wasm-host/README.md), [co-op examples](../../examples/coop/README.md) | [Capability lifecycle](../operations/capability-lifecycle.md) |

## Pin the source

**Toolchain:** Rust **1.89** or newer for `mycelium` and `mycelium-core` (`rust-version` in `Cargo.toml`);
**1.94** for `mycelium-wasm-host`. The repository pins its own toolchain in `rust-toolchain.toml`.

The supported Rust dependency path is a git tag; do not substitute a similarly named registry
package. The copyable dependencies and feature choices live in the [integrator contract](building-on-mycelium.md#1-the-dependency).
For a reproducible example checkout:

```sh
# Set this to the tag selected from building-on-mycelium.md §1:
MYCELIUM_RELEASE_TAG="v2.38.1"   # a release anchor: moved by the release script with building-on-mycelium.md §1
git clone --branch "$MYCELIUM_RELEASE_TAG" https://github.com/RichardTathata/mycelium.git
cd mycelium
cargo run --example hello_mesh
```

The tag you choose is a known baseline, not a claim that it is the latest release; the integrator contract carries the current one and is updated at each release. Record the actual commit
(`git rev-parse HEAD`), toolchain, features and configuration with your results. SDK source installs
are documented in their READMEs; use them from the same selected checkout and verify the target
node's endpoint support. Companion releases have independent version lines; review their dependency
pins rather than assuming every tag is interchangeable.

## The published node

Every `v*` tag publishes the node ([feature page](../features/published-node.md)): three binaries per platform
(`mycelium`, `mycelium-stem`, `mycelium-artifact`) for Linux x86_64, Linux aarch64 and macOS aarch64, with a
`SHA256SUMS`, and a multi-arch image. The Linux binaries are built on Debian bookworm and need glibc 2.36 or newer
and `libssl3`; the macOS binary is arm64. The sums are integrity, not provenance: nothing signs the binaries yet.

```sh
# MYCELIUM_RELEASE_TAG as set in § Pin the source (one assignment per page, so the release step moves one line);
# the first tag with artefacts is the first one cut after X0 merged.
arch="linux-x86_64"               # or linux-aarch64, macos-aarch64
base="https://github.com/RichardTathata/mycelium/releases/download/$MYCELIUM_RELEASE_TAG"
curl -fsSLO "$base/mycelium-${MYCELIUM_RELEASE_TAG#v}-$arch.tar.gz" && curl -fsSLO "$base/SHA256SUMS"
sha256sum --ignore-missing -c SHA256SUMS && tar -xzf "mycelium-${MYCELIUM_RELEASE_TAG#v}-$arch.tar.gz"
"./mycelium-${MYCELIUM_RELEASE_TAG#v}-$arch/mycelium" --version
```

```sh
docker run --rm "ghcr.io/richardtathata/mycelium:${MYCELIUM_RELEASE_TAG#v}" --version   # the image tag has no v
```

Inside a container the node binds every address for gossip (the image sets `GOSSIP_BIND_ADDRESS=0.0.0.0`); to reach
the gateway, the configuration file sets `http_addr` and a credential — the run command with ports and a mounted
file is in [deployment § Get the node](../operations/deployment.md#get-the-node).

A first fleet in one command: `mycelium init <dir>` writes the `governed-call` template (Rust; two nodes, one call
permitted and one refused by a policy enforced at the provider, with the evidence printed), and
`mycelium init <dir> --template governed-call-py` the Python one against the published node, and
`mycelium init <dir> --template two-domains` two fleets, each its own domain under `secure-multi-domain`, calling
each other across one edge — each a project of its own, pinned to this binary's release, with a README that says
what it shows and what it does not
([feature page](../features/init.md)).

Before starting a node from a configuration, `mycelium doctor node.toml` says whether `start()` would refuse and why,
and prints the effective configuration with each field's source
([diagnostics § Before you start](../operations/diagnostics.md#before-you-start-mycelium-doctor)).

The published node is built with `cli,tls,compliance,a2a,llm,metrics`, so a configuration that names `[tls]`, a token
table, `[oidc]` or `[gateway_tls]` starts (a build without the feature refuses it by name, v2.18.1). The stem is built
with `stem,gateway,llm,object_store`. Building from source stays the path for any other feature set.

## What to configure explicitly

An embedded node still needs network reachability, bootstrap discovery and a lifecycle owner.
A gateway introduces a process and an HTTP trust boundary; configure authentication and TLS before
exposing it, and point SDK clients at it with their bearer and `scheme="https"` (`mycelium-py` ≥ 0.2.9,
`mycelium-ts` ≥ 0.2.3, `langgraph-checkpoint-mycelium` ≥ 0.3.2 — earlier versions speak `http://` only). Default features do not enable `compliance`; authority and evidence require their
features, evaluator and configuration. A stem can place, activate and serve declared artifacts;
its activation command runs with the host's privileges. Offline unit checks do not turn declarations
into a sandbox or automatically install runtime mandate enforcement.

Continue to the [capability map](../capabilities.md), [developer guide](README.md), and
[readiness checklist](../operations/production-readiness.md). For measured coverage and remaining
limits use [what is proven](../operations/what-is-proven.md).
