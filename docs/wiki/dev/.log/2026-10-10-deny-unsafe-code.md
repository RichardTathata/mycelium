## [2026-10-10] ingest | `#![deny(unsafe_code)]` across the workspace's libraries (post-360 P3)

- `mycelium-core` and ten companion libraries gained `#![deny(unsafe_code)]`; the one production `unsafe`
  (`erasure.rs`'s `write_volatile` DEK wipe) is `zeroize` now, pinned by `wipe_zeroes_every_dek_byte`; test
  env mutation sits under one scoped allow per crate (`config.rs` `set_test_env`, a wasm-host `stem.rs` test).
- Pages touched: `dev/security.md` (new section: the census and the wipe's limit),
  `dev/companions/onboarding-checklist.md` (a new companion carries the attribute),
  `docs/operations/data-erasure.md` (what `destroy` wipes), `docs/analysis/ratings.md` (an addendum to
  Run 62, whose dim-8 row quoted the old count).
- The adversarial review of #596: a test through `destroy` (a test-only hook in `wipe`), `install_key`
  wipes the key it replaces (seen failing first), every un-wiped copy listed with `Zeroizing` storage as
  the path, `set_test_env` takes the lock guard and its SAFETY lines name the libc-`getenv` hazard, the
  census scoped to library crates with `examples/conway.rs` named.
