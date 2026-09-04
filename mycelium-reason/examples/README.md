# Reasoning mesh — example suite

## Objective

The **Rust mesh side** of the reasoning / LangGraph-on-Mycelium work: five layer-IV
(capability/agent) examples that stand up a real Mycelium mesh serving models (through Ollama
or any OpenAI-compatible engine), routing inference, exposing it as an OpenAI-compatible
endpoint, and rehealing a model dependency across a node failure — all on the **public API**,
no private hooks. The Python ladder that *drives* two of them lives next door in
[`../../examples/langgraph/`](../../examples/langgraph/README.md); the concept walkthrough is
guide [ch. 15](../../docs/guide/15-reasoning-and-langgraph.md).

Every example runs against an **echo/mock model by default** (`EchoBackend` — output is
`echo: {input}`), so no API key is needed. To serve a real backend, run Ollama (or any
OpenAI-compatible endpoint) per [shared setup](../../examples/README.md#shared-setup); the
`reheal_node` header notes that streaming *real* GGUF weights is the later `model_deploy`
variant, not this echo fixture.

## How to run

All five share the [repo setup](../../examples/README.md#shared-setup) (Rust toolchain;
Ollama only if you want a real model — required for `ollama_serve`; `openai_serve` runs against
the mock engine with nothing installed). `fleet_reasoning` is a one-shot CLI that exits 0;
`reason_node`, `reheal_node`, `ollama_serve`, and `openai_serve` run a **Mycelium gateway** and
**stay running** (Ctrl-C to stop) so the Python side and the Ops Console can reach them — each
prints its HTTP gateway port on startup.

### `fleet_reasoning`

**Objective.** All three Tier-3 reasoning wedges — ① load-aware routing + failover,
② replayable traces, ③ model-dependency readiness — in one in-process, three-node mesh. A
neighbourhood food-redistribution co-op: a coordinator agent reasons about surplus-to-pantry
matching while two worker nodes serve the model.

**How to run.**
```bash
cargo run -p mycelium-reason --example fleet_reasoning --features llm
```
Runs to completion and exits 0 (its printed markers are asserted by `ci_smoke.sh`); no
gateway, no key.

**What it demonstrates.** The coordinator declares a model dependency *before any provider is
up* (not-ready → wedge ③); workers come up and the dependency resolves; the coordinator routes
three calls load-aware across the workers while recording a trace (wedges ① + ②); a worker dies
mid-run and the next call **fails over** to the survivor (wedge ①); the run is then replayed
and narrated from the coordinator's KV view (wedge ②). Source:
[`fleet_reasoning.rs`](fleet_reasoning.rs).

### `reason_node`

**Objective.** The long-running gateway node the Python LangGraph rungs drive — one
gateway-carrying mesh node exposing the full `/gateway/reason/*` surface plus an echo model,
configured entirely by environment so you can start two of them as a mesh.

**How to run.**
```bash
BIND_PORT=7101 HTTP_PORT=8101 BLOB_DIR=/tmp/blobs-a \
  cargo run -p mycelium-reason --example reason_node --features llm,gateway
```
Required env: `BIND_PORT` (gossip port + node id), `HTTP_PORT` (HTTP gateway), `BLOB_DIR`
(content-addressed blob dir). Optional: `BOOTSTRAP=host:port` (join a peer), `MODEL` (default
`fable-mini`). Prints `reason node ready on <http_port>` once its gateway answers `/health`,
then parks until Ctrl-C / SIGTERM.

**What it demonstrates.** Mounting the reason router before `start`, serving blobs to peers,
and serving `MODEL` as a prompt skill (capability `llm/{model}`) via `EchoBackend` — so a
call's output is `echo: {input}`, which the Python `call_typed` rung extracts JSON from. Point
the [Ops Console](../../examples/README.md#ops-console) at its gateway port to watch it live.
Source: [`reason_node.rs`](reason_node.rs).

### `reheal_node`

**Objective.** The deploy/reheal flagship (LangGraph **rung 6**, echo variant): *a graph's
model dependency follows it across a node failure.* Extends `reason_node` with the one story
that beats a commodity checkpoint store on non-commodity terms.

**How to run** (node A serves, node B reheals):
```bash
SERVE_MODEL=1 BIND_PORT=7301 HTTP_PORT=8301 BLOB_DIR=/tmp/reheal-a \
  cargo run -p mycelium-reason --example reheal_node --features llm,gateway
REHEAL=1 BIND_PORT=7302 HTTP_PORT=8302 BOOTSTRAP=127.0.0.1:7301 BLOB_DIR=/tmp/reheal-b \
  cargo run -p mycelium-reason --example reheal_node --features llm,gateway
```
Same required env as `reason_node`, plus one role flag (`SERVE_MODEL=1` / `REHEAL=1`); `MODEL`
defaults to `reheal-demo`. Both nodes carry a gateway and stay running (Ctrl-C to stop).

**What it demonstrates.** Node A serves the model *and* publishes it as a content-addressed
"model artifact" blob, advertising its id in KV. Node B declares the demand (`require_model` →
a gossiped `req/`), structurally polls for A's advert, **fetches the artifact over the mesh**
(SHA-256 verify), and **bridges** it into a live prompt skill via `serve_model` — so once A
dies, routed inference lands on B. This touches consensus (layer III) alongside the
capability/agent layer. The blob here is a tiny echo fixture, not real weights — the honest
seam (demand → mesh fetch + content-address verify → `serve_model` bridge → routed resume) is
what's real; see the source header for the caveat. Source: [`reheal_node.rs`](reheal_node.rs).

### `ollama_serve`

**Objective.** One binary, PAIR-shaped (0.6.0): serve a **local Ollama model** into the mesh
with a live `llm-meta` ad and expose the **OpenAI-compatible façade** — so any OpenAI-speaking
client, pointed at `http://127.0.0.1:HTTP_PORT/gateway/reason/v1`, has its calls routed across
every node running this for the same model id. No proxy process; each node routes from its
own view.

**How to run** (two machines or two terminals; needs a running Ollama with `MODEL` pulled):
```bash
BIND_PORT=7201 HTTP_PORT=8201 cargo run -p mycelium-reason --features llm,gateway,ollama --example ollama_serve
BIND_PORT=7202 HTTP_PORT=8202 BOOTSTRAP=127.0.0.1:7201 cargo run -p mycelium-reason --features llm,gateway,ollama --example ollama_serve
curl -s http://127.0.0.1:8201/gateway/reason/v1/models
curl -s http://127.0.0.1:8201/gateway/reason/v1/chat/completions -H 'content-type: application/json' \
  -d '{"model":"llama3","messages":[{"role":"user","content":"Name three root vegetables."}]}'
```
Env: `OLLAMA_URL` (default `http://127.0.0.1:11434`), `MODEL` (default `llama3`), `BIND_PORT`,
`HTTP_PORT`, optional `BOOTSTRAP`, optional `GOSSIP_GATEWAY_AUTH_TOKEN` (then clients pass it as
their API key).

**What it demonstrates.** The `llm_meta` vocabulary filled from the daemon (`engine=ollama`,
`warm`, `vram_used_mb`, `family`, `ctx_window`, `param_size`, `quant`) and kept current by
`spawn_meta_refresher`; the router's load + reservation rank across the nodes; the drop-in
adoption path. Manual — not a CI example. Source: [`ollama_serve.rs`](ollama_serve.rs).

### `openai_serve`

**Objective.** The **stacking** example — Mycelium *over* any OpenAI-compatible engine: NVIDIA
PAIR's proxy, LM Studio, vLLM, Ollama's `/v1`, a cloud API. The engine (or PAIR) places the GPU
work; Mycelium routes the *capability* `llm/{MODEL}` across every node running this (load +
local reservations, failover) and exposes the OpenAI-compatible façade. Unlike `ollama_serve`
the `llm-meta` ad is **static** (declared by env), so it works against endpoints that are not
Ollama.

**How to run** (deterministic, nothing installed — the repo's mock engine stands in on `:11434`):
```bash
python3 examples/community/mock_llm.py &
BIND_PORT=7211 HTTP_PORT=8211 ENGINE=pair CTX_WINDOW=8192 \
  cargo run -p mycelium-reason --features llm,gateway --example openai_serve
BIND_PORT=7212 HTTP_PORT=8212 BOOTSTRAP=127.0.0.1:7211 \
  cargo run -p mycelium-reason --features llm,gateway --example openai_serve
curl -s http://127.0.0.1:8211/gateway/reason/v1/models          # "providers": 2
curl -s http://127.0.0.1:8211/gateway/reason/v1/chat/completions -H 'content-type: application/json' \
  -d '{"model":"llama3","messages":[{"role":"user","content":"Name three root vegetables."}]}'
```
For a real engine set `OPENAI_BASE_URL` (PAIR: `http://<pair-host>:11434/v1`; LM Studio:
`http://127.0.0.1:1234/v1`) and `OPENAI_API_KEY` if it wants one; `MODEL` (default `llama3`),
optional `ENGINE` / `CTX_WINDOW` / `FAMILY` for the ad, `GOSSIP_GATEWAY_AUTH_TOKEN` to protect the
gateway (clients then pass it as their API key). Each node may point at a *different* engine.

**What it demonstrates.** The division of labour the PAIR comparison settled on
(`docs/plans/mycelium-reason.md`, 2026-09-04 addendum): no proxy process of Mycelium's own —
every node routes from its own view; the reply's `mycelium.provider` names the node that
answered. Note what a *sequential* run shows: with equal load every call lands on the lowest
node id (the deterministic tiebreak) — reservations spread **concurrent** calls, not serial ones;
run several clients at once to see both providers answer. Verified 2026-09-04 against the mock
engine: two nodes, both providers listed, calls routed from either node's façade. Manual — not a
CI example (the façade and router it composes are CI-gated in the crate's suites). Source:
[`openai_serve.rs`](openai_serve.rs).

## CI

`fleet_reasoning` is Docker-free and asserted on its printed markers by the reasoning
`ci_smoke.sh`. `reason_node` and `reheal_node` are driven end-to-end by the Python LangGraph
suite ([`../../examples/langgraph/`](../../examples/langgraph/README.md)).
