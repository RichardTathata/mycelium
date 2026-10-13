## [2026-10-13] ingest | post-360 row F2: the unsigned-frame refusal made unconditional

- **A TLS node refuses an unsigned KV `Data` frame and a bare KV signature, whatever `kv_require_signed_frames` says**
  (`mycelium-core/src/connection.rs`, the `Data` and `SignedData` arms). Row F's one-release window (§35) is closed;
  the upgrade note is `deprecations.md` §40 (upgrade from 2.33.x, not 2.32).
- **The setting's fate: retired as a no-op**, not removed (nothing is removed on 2.x): `#[deprecated(since = "2.34.0")]`,
  still parsed (TOML and env), `start()` warns when it is `true`. An explicit `false` is indistinguishable from the
  default and restores nothing.
- **The window counters stop rising** — `kv_unsigned_frames_accepted` and `kv_untagged_signatures` stay on `GET /stats`
  at zero; a bare signature is counted as refused (`kv_untagged_signatures_refused`).
- **`mesh.signed_writes` rev 2** resolves on `[tls]` alone; **`secure-single-domain` rev 5** requires it — a free
  addition, because it resolves on exactly what `mesh.tls` does.
- **Anti-entropy restated, not closed:** a `StateResponse` entry carries no author or signature; authenticating it needs
  a wire change. Pages: `deprecations.md` §35/§40, threat model Boundary A, `what-is-proven.md`, `configuration.md`,
  `cert-rotation.md`, `revocation-authority.md`, `production-readiness.md`, guide 20, wiki `dev/security.md`,
  `dev/operations.md`, `dev/history.md`, the plan's F2 row, §3.5 and §3.6.
- Lesson: a window's tests are written against the window — every multi-node fleet set the flag to `true`, so none of
  them exercised the default. F2's first move was to delete the flag from every test fleet, which turned the exemption
  test into a regression test that failed on the unfixed code.
