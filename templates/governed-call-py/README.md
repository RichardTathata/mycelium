# governed-call-py

The published `mycelium` node, run from Python with no Rust toolchain. An SDK agent declares a unit file, writes a
key, and a second agent — holding a token without `kv:write` — is refused by the gateway's own scope rule.

```sh
pip install -e <checkout>/mycelium-py   # the SDK is installed from a checkout; it is not on PyPI
python main.py                          # MYCELIUM_BIN=/path/to/mycelium if the published node is not on PATH
```

What you will see: the declaration accepted with `not_enforced: ['[[mandate]]', '[[rule]]']`, `alice set demo/greeting:
PERMITTED`, and `reader set demo/greeting: REFUSED 403 — insufficient scope (the route needs kv:write)`.

## The layout

| Path | What it is |
|---|---|
| `node.toml` | the node's configuration: a gateway on loopback with two named tokens |
| `units/purchaser.toml` | the unit file alice declares; its mandate and rules come back as `not_enforced` (one tool waits for the treasurer's grant — `mycelium test --faults grant units/` drives it) |
| `main.py` | the program: starts the node, declares, writes, is refused, stops the node |
| `data/node.log` | the node's log from the last run |

## What this does and does not show

It shows the gateway governing an SDK agent by scope: a route a token's scopes do not cover is refused before any
handler runs, by name. It does **not** show a policy decided per call: the published node attaches no action
evaluator and no evidence journal, so the unit file's mandate and rule are a declaration here, not a check — which
the node says itself, in `not_enforced`. The per-call policy, enforced at the provider and recorded as evidence, is
the Rust template's (`mycelium init <dir> --template governed-call`), until a node can attach one from
configuration. `mycelium doctor node.toml` says exactly which guarantees this configuration enforces.
