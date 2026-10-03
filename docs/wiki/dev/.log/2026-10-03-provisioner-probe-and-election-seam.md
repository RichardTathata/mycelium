## [2026-10-03] ingest | two provisioner defects found by I4's reconnaissance, fixed before the pilot

**What:** (1) `activation::hook` returned `Ok(Some(false))` for a failing initial probe — the install
completed, the capability was advertised and counted, the health pass withdrew it a round later; it is an
activation error now (`InstallError::Activation`, nothing advertised, retried next round). (2)
`Provisioner::self_elects` was raw `fastrand`; it draws from `sim_seam::rng_f32("select")` now, and the
seam lint refuses any random draw in `mycelium-wasm-host` (zero baseline). Both seen failing first.

**Durable knowledge:** the seam lint scanned `src` and `mycelium-core/src` only — a companion crate could
hold an unseamed draw for months (the stem's since 2026-09-28). RNG is now gated there; the host's ~140
fs/time sites are a baseline decision still open (plan §8). And a probe that *gates* a capability must gate
its first advertisement, not only its survival: "live, then withdrawn" is a miscount the trace would have
inherited.
