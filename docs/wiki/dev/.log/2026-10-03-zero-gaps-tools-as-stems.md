## [2026-10-03] ingest | zero gaps Z4 — the llm_agent tools as components

**What:** four fixture crates (`*-tool-component`, built for `wasm32-wasip2`, committed `.wasm`),
`tool-*.toml` descriptions, `n-0`/`n-1` host `wasm-component` with presence floors, the agent's unit
requires the four tools, the driver asserts each tool's own schema and calls two; the runtime bridge
asks a `tool/*` component for `describe` at install. Plan D4.

**Durable knowledge:** the format needed no tool section — `provides.ns = "tool"` on a component is
the declaration, and the bridge existed since X2. What was missing was the demo's four handlers as
components and the schema: a bridge that publishes `{"type":"object"}` tells an agent nothing about
the arguments, and the manifest line (signed) is the wrong place for a schema. Self-description by a
reserved request kind keeps the signature over the bytes and lets the component own its contract.
