# 1 · Your first stem fleet

↑ [Tutorials](README.md) · Next: [Code to declarations](02-code-to-declarations.md)

## Objective

Give three generic hosts one capability to maintain: `demo/echo`, with two live
providers and a third host available to replace one. The component is checked in;
there is no model, remote download or domain application to understand first.

## How to run

Prerequisites: the repository's Rust toolchain and permission to bind loopback
sockets. From the repository root:

```sh
cargo run -p mycelium-wasm-host --example first_stem_fleet
```

Look for four `PASS:` lines: two hosts install; a discovered provider echoes
`hello stem`; providers map to declared hosts; the survivors restore the floor.
Assertions and bounded waits make an unsuccessful run exit nonzero.

## What it demonstrates

Read [first_stem_fleet.rs](../../../mycelium-wasm-host/examples/first_stem_fleet.rs)
from top to bottom:

1. The librarian publishes a signed catalogue entry for a tiny WASM component.
2. The printed unit declaration permits that publisher and runtime kind, and
   declares the requirement and requests a presence band of exactly two
   (`min_providers = 2`, `max_providers = 2`). Each host starts with the same declaration.
   All three self-elect, so the band can overshoot to three; the host ranked beyond the ceiling
   withdraws (`prov.shed`), which is why the example waits for providers to settle —
   [capability-lifecycle.md § Presence ceilings](../../operations/capability-lifecycle.md).
3. Hosts discover the entry and pull, verify and install it. The observer resolves
   `demo/echo`; it does not choose a preconfigured service address.
4. An RPC checks the actual component result. A static wiring report is joined to
   observed providers using the example's explicit node-to-unit mapping.
5. One hosting stem stops gracefully. The available host installs the component
   and the observer again sees two providers.

The seed address, runtime kind, trust key and desired floor are configured.
The example driver starts the processes; it does not assign providers.
Which eligible hosts serve the capability is decided at runtime. The floor is an
intent under eventual convergence, not an instantaneous global cardinality guarantee.

## Try a change, then a refusal

Change the RPC payload and its expected echo together; rerun to demonstrate that
installed code, rather than a catalogue entry alone, answers the request.

For a trust failure, change the unit declaration's trusted publisher to a different
Ed25519 public key while leaving the signing key unchanged. The floor assertion
must time out: seeing an artifact is not permission to install it. Diagnose the
signer/trust mismatch before increasing a timeout. Restore the original key after
the exercise. The focused regression checks an unsigned-entry refusal and trusted-signature
installation without waiting for a fleet:

```sh
cargo test -p mycelium-wasm-host require_provenance_refuses_unsigned_but_installs_trusted_signed
```

## Dev notes

This is one process with real loopback sockets and a **graceful removal**, not a
crash, partition or multi-machine recovery benchmark. The embedded signing seed
is a public test fixture, never a production credential. An accepted publisher
is not evidence that arbitrary code is safe or semantically correct.

Normal completion stops agents and removes its own temporary artifact library.
After an assertion failure, process exit stops the tasks; a temporary directory
named `mycelium-first-fleet-<pid>-<port>` may remain. Remove only that run's directory.
CI runs the example and the observation consumer in the WASM-host job.

For your application, replace the echo component and artifact description together,
set your publisher trust and budgets, and retain assertions on useful output.
The [unit-file reference](../../reference/unit-file.md) and
[artifact guide](../../operations/artifacts.md) cover the full fields.
