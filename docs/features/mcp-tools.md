# MCP tools

## What it is

An [MCP tool](../guide/00-concepts.md#mcp-tool) is a function a node registers with a JSON Schema; the registration is
written under `tools/{name}/{node}` and gossiped, so any node discovers the fleet's tools with a local scan and calls
one as `mcp.invoke` to the node that serves it. A node that starts or stops serving a tool changes what every planner
sees on its next cycle, with no restart. The gateway serves the same tools to an outside MCP client at `/mcp`
(`tools/list`, `tools/call`), and a node can pull an external MCP server's tools into the same namespace, so they are
discovered and called the same way. `mcp.invoke` is a protected kind: the gateway's action preflight decides each
`tools/call`, and a provider with enforcement on decides it again.

## When to use it, and when not

- **Use it when** an agent should call functions — an API, a lookup, a computation — that other teams or nodes supply,
  and the set changes over time.
- **Not when** the callee is itself a reasoning step with a role: that is a [skill](skills.md); and not when you need a
  tool decided per argument without an evaluator — a gateway with none runs any tool a token's `mcp:invoke` reaches.

## Turn it on

This feature is turned on in code; `/mcp` needs `gateway` and an `http_port`.

- [`register_mcp_tool`](../../src/agent/mcp_handle.rs): a name, a description, a JSON Schema and a handler;
  [`register_mcp_tool_with_principal`](../../src/agent/mcp_handle.rs) and
  [`register_mcp_tool_with_call`](../../src/agent/mcp_handle.rs) give the handler the verified caller or the whole call.
- [`connect_mcp_server`](../../src/agent/mcp_handle.rs) (`gateway`): bridge an external MCP server's tools; its URL and
  every redirect are checked against the [egress allow-list](egress-allow-list.md).
- `/mcp` under `mcp:invoke` — a scope checked only in a build with `compliance` (the published node has it); without it
  any valid bearer reaches `/mcp` — with an [action evaluator](action-authorisation.md) to decide which tools a caller
  may run.
- `test_mcp_tools_call_round_trip`, in [`src/agent/http/http_tests.rs`](../../src/agent/http/http_tests.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`three_node_demo`](../../examples/README.md#the-capability-matrix), the chat demo: tool nodes join live and the model
discovers their tools on its next message. The chat needs a real model, so CI runs this binary's nodes only in other
smokes, never the chat; CI does run [`mcp_tool_authority`](../../examples/README.md#the-capability-matrix), where an
evaluator decides which tools an agent may call through `/mcp`. And [`mcp_toolgrowth`](../../examples/README.md#the-capability-matrix)
grows a tool on demand ([hosted components](hosted-components.md)).

## What it guarantees, and what it does not

- No guarantee or rule id of its own: `tools/call` is [`ae.preflight`](../reference/rule-catalogue.md#aepreflight)'s at
  the gateway and [`provider.enforcement`](../reference/rule-catalogue.md#providerenforcement)'s at a provider that
  turns it on.
- **Not built:** a tool registration is a claim, not proof the tool does what its description says; a bridged external
  server's own behaviour is outside the node; discovery is as fresh as gossip.
- **Evidence:** the ledger row
  [after a revocation every door runs nothing](../operations/what-is-proven.md#revocation-every-door-runs-nothing), `/mcp`
  among the doors.

## Go deeper

- **Guide:** [06, tool discovery](../guide/06-tool-discovery.md), including
  [§ Bridging an external MCP server](../guide/06-tool-discovery.md#bridging-an-external-mcp-server);
  [20 § Which tools an agent may actually call](../guide/20-authorising-actions.md#which-tools-an-agent-may-actually-call).
- **Runbook:** [rbac](../operations/rbac.md) for `mcp:invoke`.
- **Design:** no design record; the guide is the record.
- **Code:** [`src/agent/mcp_handle.rs`](../../src/agent/mcp_handle.rs) and [`src/agent/mcp.rs`](../../src/agent/mcp.rs);
  the gateway's `/mcp` in [`src/agent/http/`](../../src/agent/http/mod.rs).
