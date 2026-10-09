# Installation and integration modes

Choose how your application joins the mesh before selecting an example. These are deployment
choices over the same substrate, not different architecture layers.

| Mode | What you deploy | First working path | Operations |
|---|---|---|---|
| Rust embed | Your application containing `GossipAgent` | [Integrator contract](building-on-mycelium.md), then `cargo run --example hello_mesh` from a source checkout | [Deployment](../operations/deployment.md) |
| Python or TypeScript | Your application plus a Rust node exposing the HTTP gateway | [Language bridges](10-language-bridges.md), [Python SDK](../../mycelium-py/README.md), [TypeScript SDK](../../mycelium-ts/README.md) | [Gateway TLS](../operations/gateway-tls.md), [readiness](../operations/production-readiness.md) |
| Generic stem host | `mycelium-stem`, unit declarations and configured artifact/runtime dependencies | [Wasm host](../../mycelium-wasm-host/README.md), [co-op examples](../../examples/coop/README.md) | [Capability lifecycle](../operations/capability-lifecycle.md) |

## Pin the source

**Toolchain:** Rust **1.89** or newer for `mycelium` and `mycelium-core` (`rust-version` in `Cargo.toml`);
**1.94** for `mycelium-wasm-host`. The repository pins its own toolchain in `rust-toolchain.toml`.

The supported Rust dependency path is a git tag; do not substitute a similarly named registry
package. The copyable dependencies and feature choices live in the [integrator contract](building-on-mycelium.md#1-the-dependency).
For a reproducible example checkout:

```sh
# Set this to the tag selected from building-on-mycelium.md §1:
MYCELIUM_RELEASE_TAG="v2.31.0"   # a release anchor: moved by the release script with building-on-mycelium.md §1
git clone --branch "$MYCELIUM_RELEASE_TAG" https://github.com/RichardTathata/mycelium.git
cd mycelium
cargo run --example hello_mesh
```

The tag you choose is a known baseline, not a claim that it is the latest release; the integrator contract carries the current one and is updated at each release. Record the actual commit
(`git rev-parse HEAD`), toolchain, features and configuration with your results. SDK source installs
are documented in their READMEs; use them from the same selected checkout and verify the target
node's endpoint support. Companion releases have independent version lines; review their dependency
pins rather than assuming every tag is interchangeable.

## What to configure explicitly

An embedded node still needs network reachability, bootstrap discovery and a lifecycle owner.
A gateway introduces a process and an HTTP trust boundary; configure authentication and TLS before
exposing it. Default features do not enable `compliance`; authority and evidence require their
features, evaluator and configuration. A stem can place, activate and serve declared artifacts;
its activation command runs with the host's privileges. Offline unit checks do not turn declarations
into a sandbox or automatically install runtime mandate enforcement.

Continue to the [capability map](../capabilities.md), [developer guide](README.md), and
[readiness checklist](../operations/production-readiness.md). For measured coverage and remaining
limits use [what is proven](../operations/what-is-proven.md).
