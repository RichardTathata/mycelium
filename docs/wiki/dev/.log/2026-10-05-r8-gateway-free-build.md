## [2026-10-05] ingest | realignment repairs R8 — a gateway-free build refuses gateway settings, and is tested

**What:** `src/agent/lifecycle.rs` — `start()` refuses `http_port` / `gateway_tls` without `gateway`;
`src/agent/guarantee.rs` — `in_gateway_build`, wrapping `gw.not_open`, `gw.tls`, `gw.caller_profile`;
`mycelium-gateway-free-tests` (new test-only workspace crate, four tests); the CI gateway-free job and
`make check-full` run it; the testing page's paragraph; the field table's R8 note; the changelog; the
plan row; this log.

**Durable knowledge:**

- **A dev-dependency can make a feature impossible to test without.** `mycelium-tuple-space` with
  `gateway` is a dev-dependency of `mycelium`, so feature unification turns `gateway` on in every test
  build of `mycelium`. A `#[cfg(not(feature = "gateway"))]` test inside the crate is never compiled, and
  the plan's "no test runs in a gateway-free build" was literally true, for that structural reason. The
  first draft of R8's witnesses went inside the crate and "passed" 824 tests without running; checking
  that the witnesses appeared in the run is what found it.
- **A test crate that depends on the library with `default-features = false`** is the gateway-free test
  build, provided it is run alone (`-p`). Its first test asserts the build really lacks the feature,
  from the guarantee report's `features`, so a shared invocation fails instead of passing vacuously.
- **`cfg!` in the resolver, not `#[cfg]`.** `in_gateway_build` keeps every resolver compiled and linted
  in every build; only the answer depends on the feature.
