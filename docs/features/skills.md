# Skills

## What it is

A [skill](../guide/00-concepts.md#skill) is a capability with a handler behind it, reached over the mesh as an RPC of
kind `skill.invoke`. The `skillrunner` binary turns a `.skill.toml` manifest into one: a node that joins the mesh,
advertises its capability, answers each invocation by running its prompt against an OpenAI-compatible model, and
writes an audit record — into the node's signed, hash-chained trail in a build with `compliance` and `[tls]`; in a
`compliance` build without `[tls]` the record is not written, only warned about; and as a plain unsigned `audit/` entry
in a build without `compliance`. In that build only, the binary's dashboard reads the count and the ten newest of
those records from its own `GET /gateway/skillrunner/invocations`, guarded by the gateway's bearer and nothing finer
— the build has no scope table — so any bearer holder, or anyone on a gateway with no token, reads them; a
`compliance` build does not mount the route and serves `GET /gateway/audit` under `audit:read`. A skill can call other skills by name: the ones it lists in `tools` are resolved
against live advertisements when it runs, never by address. `skill.invoke` is a protected kind, so a gateway's action
evaluator decides a call that comes through that gateway; `skillrunner` itself attaches no evaluator and no provider
enforcement, so a call reaching it another way is not decided there.

## When to use it, and when not

- **Use it when** an agent role — a researcher, a writer, a triage step — should be a long-lived node other agents find
  by what it does, defined in a file rather than code.
- **Not when** what you need is a function or an API call: that is an [MCP tool](mcp-tools.md); and not when the
  prompt should live with the node that needs it, which is a [prompt skill](llm-prompt-skills.md).

## Turn it on

Build `skillrunner` from source — it is not among the published binaries — and give it a manifest:
`cargo build --bin skillrunner`, then `skillrunner --skill <file>.skill.toml`. Build with `--features a2a` to mount
`/a2a` on a skill node that has an `http_port`. The checked-in [`examples/skills/hello.skill.toml`](../../examples/skills/hello.skill.toml)
is a minimal manifest.

- The manifest's `[node]` (`bind_port`, `bootstrap_peers`), `[capability]` (`ns`, `name`, and a `policy`) and `[skill]`
  (the prompt, `tools`, and `[skill.llm]` with the endpoint and model)
  ([guide 5 § Reference — SkillRunner](../guide/05-skills.md#reference--skillrunner)).
- Call it from any node with `rpc_call(provider, "skill.invoke", …)` after resolving its capability, or through `/a2a`.
- `parse_minimal`, in [`src/bin/skillrunner/config.rs`](../../src/bin/skillrunner/config.rs): the minimal manifest
  parses to its capability, port and model.

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`community`](../../examples/README.md#the-capability-matrix): orchestrator, researcher, verifier and writer skills that
compose by name; CI runs it against a deterministic mock model in its demo-smoke job, whose assertions guard four
regressions a live run once found.

## What it guarantees, and what it does not

- No guarantee or rule id of its own: `skill.invoke` is protected, so its calls are
  [`ae.preflight`](../reference/rule-catalogue.md#aepreflight)'s at a gateway and
  [`provider.enforcement`](../reference/rule-catalogue.md#providerenforcement)'s at a provider that turns it on.
- **Not built:** a published `skillrunner` binary; configuration beyond the manifest — `skillrunner` starts from
  `GossipConfig::default()` and reads no `GOSSIP_*` overrides, and the manifest has no egress list, gateway token,
  `http_addr` or evaluator, so its egress allows every host, its gateway and `/a2a` take no credential and listen on
  the default loopback address only, and it runs no provider enforcement; credentials at egress for its model — the `api_key` sits in the manifest; long audit retention on its own — its records are
  ordinary KV entries, so retention needs an audit sink or an export ([audit](../operations/audit.md)).
- **Evidence:** the [what-is-proven](../operations/what-is-proven.md) ledger has no row for skills on their own.

## Go deeper

- **Guide:** [05, skills](../guide/05-skills.md); the full manual is
  [`reference/skillrunner.html`](../reference/skillrunner.html).
- **Runbook:** no runbook of its own; [rbac](../operations/rbac.md) for the scopes a gateway needs to reach one.
- **Design:** no design record; the guide is the record.
- **Code:** [`src/bin/skillrunner/`](../../src/bin/skillrunner/runner.rs) (`runner.rs`, `config.rs`, `llm.rs`, `audit.rs`).
