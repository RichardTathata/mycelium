## [2026-10-27] ingest | Y3: the Ops Console observes from the browser; `gateway_cors_origins`

Adopter-surface Y3 (D16, D28): `examples/ops_console` is a static page (`index.html`) reading each listed node's
existing routes directly and merging them on the page — three tabs added (Guarantees, Governors, Tuples); its `:8099`
server and `/api` proxy removed (`deprecations.md` §67). A gateway lets a page on another origin read it only through
`gateway_cors_origins` (off by default; exact origins; `GET`/`HEAD` on `CORS_READ_PATHS`, `src/agent/http/cors.rs`).
CI renders the page in Chrome against `examples/observed_fleet.rs`, tab by tab (`scripts/ops-console-render.sh`, job
`ops-console`). Every browser showcase and the coop bootstrap list `http://127.0.0.1:8099`. Pages:
[security](../security.md) (cross-origin reads); [examples](../examples.md) (the console);
[ui-example-contract](../ui-example-contract.md) (rule 2 and the classification, 13 hits); `operations/observability.md`
§ The Ops Console; `operations/rbac.md`; `reference/configuration.md`.
