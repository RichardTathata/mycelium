# 08 — A2A Interop: LangChain and AutoGen on the mesh

## Concept

The A2A (Agent-to-Agent) protocol is a simple HTTP standard for agent
discovery: a well-known endpoint (`/.well-known/agent.json`) describes what
an agent can do, and a task endpoint (`/a2a`) accepts work. Mycelium
implements both sides of this protocol via `--features a2a`, which means any
framework that speaks A2A — LangChain, AutoGen, or a hand-rolled client — can
discover and call Mycelium skills without knowing anything about gossip,
capabilities, or the mesh topology.

> **What `/a2a` does and does not check.** The route is public by design: a peer needs no Mycelium
> credential. No bearer → the caller is `anonymous`; an *unrecognised* bearer → **401**; a federation
> credential, if presented, is the caller's identity. A malformed request answers JSON-RPC `-32700`.
> What decides whether a skill *runs* is an attached `ActionEvaluator` (chapter
> [20](20-authorising-actions.md)) or a federation edge ([17](17-federation.md)); with neither, an
> anonymous caller reaches skill dispatch and `with_a2a()` warns about exactly that. Building with
> `--features a2a` alone (no `tls`) has no evaluator by construction. The methods the handler knows
> are `tasks/send`, `tasks/sendSubscribe`, `tasks/get` and `tasks/cancel`.

```mermaid
sequenceDiagram
    participant LC as LangChain Agent<br/>(Python)
    participant GW as SkillRunner<br/>HTTP Gateway :9050
    participant O as orchestrator<br/>skill
    participant R as researcher<br/>skill
    participant W as writer<br/>skill

    LC->>GW: GET /.well-known/agent.json
    GW-->>LC: {skills: [llm/orchestrator, llm/researcher, llm/writer]}
    LC->>LC: wrap each skill as a native LangChain tool
    LC->>GW: POST /a2a tasks/send {skill: llm/orchestrator, input: {topic: "..."}}
    GW->>O: rpc_call skill.invoke
    O->>R: rpc_call (mesh-internal)
    O->>W: rpc_call (mesh-internal)
    W-->>O: article
    O-->>GW: article
    GW-->>LC: {status: completed, result: {...}}
```

From the Python agent's perspective it made one tool call and got back a
finished article. The mesh-internal routing — orchestrator calling researcher
calling writer — is completely invisible. Mycelium is the routing layer;
the Python agent is just a consumer.

This pattern is useful for:
- Giving existing LangChain/AutoGen workflows access to Mycelium skills
  without rewriting them in Rust
- Letting the mesh absorb the complexity of multi-skill pipelines while
  exposing a simple A2A interface to orchestrators
- Mixing Mycelium skills with non-Mycelium agents in a single workflow

---

## The Example

`examples/a2a_langchain/` provides two agents: a LangChain ReAct agent and an
AutoGen v0.4 agent. Both connect to a running community skills cluster,
auto-discover available skills, and use them to answer a question.

**Prerequisites**

```bash
# Build SkillRunner with A2A support
cargo build --bin skillrunner --features a2a

# Start the community skills cluster
cd examples/community && ./start.sh && cd -

# Install Python dependencies
cd examples/a2a_langchain
pip install -r requirements.txt
```

Set your LLM API key (for the Python agent's own LLM — not the skills):

```bash
export OPENAI_API_KEY=sk-...        # to use gpt-4o-mini
# or set OLLAMA_BASE_URL for local Ollama
```

**Run — LangChain agent**

```bash
cd examples/a2a_langchain
python langchain_agent.py
```

**Expected output** (`langchain_agent.py`; the shape — descriptions and the model's text vary)

```
Connecting to Mycelium at http://localhost:9050 ...

  Connected to: Mycelium cluster
  Discovered 4 skill(s):
    · llm/orchestrator                …
    · llm/researcher                  …
    · llm/writer                      …
    · llm/verifier                    …

Query: …

  [tool call]   llm_orchestrator({'topic': …})
  [tool result] {"title": …, "article": …}...

============================================================
<the model's answer>
```

**Run — AutoGen agent**

```bash
python autogen_agent.py
```

---

## How It Works

**`/.well-known/agent.json`** is served by the node's gateway when the node is built with
`--features a2a` and enables it (`with_a2a()`). It lists the skills `/a2a` can call, scanned from the
capability KV entries at request time. Since 2.27.0 it leaves out what `tasks/send` cannot reach, and
`tasks/send` refuses the same set (`-32001`):

- the fleet's own plumbing — a provisioning tier (`{ns}/loading`, `{ns}/installable`), a stem's shed mark
  (`prov-shed/*`), model metadata (`llm-meta/*`), the artifact librarian, the reason blob cache, and a
  companion's election and role marks (`{ns}/{x}.primary`, `.secondary`, `.candidate`, `.curator`);
- **prompt skills** (`register_prompt_skill` — `mycelium-reason`'s `llm/{model}`, a stem's `[[serve]]`):
  they answer `llm.invoke`, not the `skill.invoke` `/a2a` sends, and are called through
  `POST /gateway/llm/call` under `llm:invoke`. The refusal names that route.

```json
{
  "skills": [
    {
      "id":          "llm/orchestrator",
      "name":        "llm/orchestrator",
      "description": "Coordinates research and writing to produce articles",
      "inputSchema": { "type": "object", "properties": { "topic": {"type": "string"} } }
    }
  ]
}
```

**`/a2a tasks/send`** accepts a task, resolves the target skill from the mesh,
and dispatches via RPC. The HTTP response waits for the RPC result:

```python
# What A2aClient.send does (mycelium-py) — JSON-RPC 2.0, the input as a text part
import json
from mycelium import A2aClient

client = A2aClient("http://localhost:9050")          # token="…" on a protected gateway
text = client.send("llm/orchestrator", json.dumps({"topic": "gossip protocols"}), timeout_secs=120)
# on the wire: {"jsonrpc":"2.0","id":1,"method":"tasks/send",
#   "params":{"id":"<task>","skillId":"llm/orchestrator",
#             "message":{"role":"user","parts":[{"type":"text","text":"…"}]}}}
```

**LangChain tool wrapping** (`langchain_agent.py`):

```python
from langchain.tools import StructuredTool

def make_tool(skill):
    def call(**kwargs):
        return client.send(skill["id"], json.dumps(kwargs), timeout_secs=120.0)
    return StructuredTool.from_function(
        func=call,
        name=skill["id"].replace("/", "_"),
        description=skill.get("description", ""),
        args_schema=_args_model(skill["id"].replace("/", "_"), skill.get("inputSchema")),  # a pydantic model built from the card's JSON Schema — see the file
    )

tools = [make_tool(s) for s in client.fetch_card().get("skills", [])]
```

---

## Dev Notes

**Build flag required.** A2A routes are compiled in only with `--features a2a`:

```bash
cargo build --bin skillrunner --features a2a
```

Without the flag the gateway starts but `/a2a` and `/.well-known/agent.json`
return 404.

**Gateway port.** The A2A gateway uses the `http_port` in the skill TOML.
The orchestrator's default is 9050 (`http_port = 9050` in
`orchestrator.skill.toml`). Any skill node can serve A2A — point clients at
whichever gateway has `http_port` set.

**Which skill to expose.** You typically expose the orchestrating skill
(the outermost node) via A2A, not the sub-skills. Clients call the
orchestrator; the mesh handles internal routing transparently.

**Timeout management.** The A2A task endpoint waits for the full RPC chain.
For a 3-skill pipeline on CPU inference this can take 60–90 s. Set
`requests.post(..., timeout=120)` in Python clients accordingly.

**OpenAI vs Ollama for the Python agent.** The Python agent uses its own LLM
for reasoning about which tool to call. Set `OPENAI_API_KEY` for gpt-4o-mini,
or set `OLLAMA_BASE_URL=http://localhost:11434/v1` and `OLLAMA_MODEL=llama3.2`
for local inference. The Mycelium skills use their own separate LLM config.

**Extending to custom clients.** Any HTTP client that can `GET
/.well-known/agent.json` and `POST /a2a tasks/send` can use Mycelium skills.
The protocol is intentionally minimal — a JSON description and a task endpoint.
No SDK required.

**AutoGen v0.4 specifics.** `autogen_agent.py` uses the `FunctionTool` API.
AutoGen's tool interface requires `name` to be a valid Python identifier — the
agent strips `/` from skill ids when registering tool names.
