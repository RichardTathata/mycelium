# WASM host examples

↑ [Example portfolio](../../examples/README.md)

## Objective

Learn generic stem hosting without a model or external artifact download.

## How to run

From the repository root, with the pinned Rust toolchain and loopback sockets:

```sh
cargo run -p mycelium-wasm-host --example first_stem_fleet
```

## What it demonstrates

Three generic hosts, a signed echo component, a declared floor of two providers,
a useful invocation, and replacement after graceful removal. Four `PASS:` lines
confirm the assertions. This is not a crash or partition recovery benchmark.

## Dev notes

Follow [the full tutorial](../../docs/guide/tutorials/01-first-stem-fleet.md) for
an exercise, a trust refusal, cleanup and adaptation. An optional new output
directory exports declarations and a real observation snapshot for
[the consumer tutorial](../../docs/guide/tutorials/06-declared-versus-observed.md).
The embedded signing seed is a demo fixture. CI runs the example and consumer.
