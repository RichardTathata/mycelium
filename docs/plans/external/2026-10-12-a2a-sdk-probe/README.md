# The `a2a-sdk` probe (2026-10-12)

Evidence for plan row **N** (`docs/plans/post-360-hardening.md`): can a current A2A client — `a2a-sdk` 1.2.2, the
library Strands Agents uses — reach a Mycelium node's skills through `/a2a`? **No** (output in
[`probe-output.txt`](probe-output.txt)).

| File | What it is |
|---|---|
| `node/` | A loopback node that mounts `/a2a` (`with_a2a()`) and serves one skill, `depot/dispatch`. The `mycelium` binary does not mount `/a2a`, so the probe needs its own embedder. |
| `probe.py` | Fetches the card at both well-known paths, resolves it with the SDK's `A2ACardResolver`, and posts the three generations of method name (`SendMessage` — A2A 1.0; `message/send` — 0.3; `tasks/send` — what Mycelium serves). |
| `send.py` | The path Strands takes: build a client from the card and `send_message`. |

To re-run (`node/Cargo.toml` depends on the checkout by relative path):

```bash
cd node && CARGO_TARGET_DIR=/tmp/a2a-probe-target cargo build
PORT=53031 /tmp/a2a-probe-target/debug/a2a-probe-node &      # gateway on PORT+1
uv venv /tmp/a2a-venv && VIRTUAL_ENV=/tmp/a2a-venv uv pip install "a2a-sdk[http-server]" httpx
/tmp/a2a-venv/bin/python -I probe.py http://127.0.0.1:53032
/tmp/a2a-venv/bin/python -I send.py  http://127.0.0.1:53032
```

What it found: the card only at `/.well-known/agent.json` (the SDK reads `agent-card.json`); the card's `url` is the
node's gossip address, not the gateway, and omits `/a2a` (`src/agent/a2a.rs:445`) — so `send_message` failed with a
network error; `SendMessage` and `message/send` answer `-32601`; the skill is chosen by a Mycelium-only `skillId`.
Row N's last promise is to turn this into a CI test against the real SDK client.
