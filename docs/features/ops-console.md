# The Ops Console

## What it is

The Ops Console is a static page that reads a fleet's nodes from the operator's browser. Each node listed answers the
page directly, and the page merges the answers itself: there is no console server, no proxy, and nothing polling the
fleet. Its tabs read only routes the nodes already have — `/stats`, `/gateway/fleet`, `/gateway/diagnose`,
`/gateway/guarantees`, `/gateway/govern`, `/gateway/tuple/overview`, `/gateway/audit`, `/gateway/kv/keys`, `/metrics` —
and each node's panel carries one state: `ok`, `blocked`, `unauthorized`, `absent` or `error`. A browser reads
another origin only if that origin allows it, so each node lists the page's origin. The rule it keeps: a panel that
would need a process polling every node gets a new route on a node instead.

## When to use it, and when not

- **Use it when** you want to watch a demo, a showcase or a real fleet without changing it, and read where nodes'
  views differ.
- **Not when** you need history, alerting or a control plane: it shows what each node answers now; point Grafana at
  `/metrics` for the rest. It is an example, not a product.

## Turn it on

Serve [`examples/ops_console/`](../../examples/ops_console/README.md) from any static host and list its origin on every
node.

- [`gateway_cors_origins`](../reference/configuration.md#gateway_cors_origins): exact origins, off by default,
  answered only on the gateway's read-only routes; a gateway with no credential model refuses the start unless every
  origin listed is a loopback one.
- The bearer, if the nodes require one, is typed into the page and still decides what each route answers; it goes only
  to the nodes added.
- The example `observed_fleet` ([its run](../../examples/ops_console/README.md#how-to-run)) is a three-node fleet
  configured for it.

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`ops_console`](../../examples/README.md#the-capability-matrix). It is not a binary, so CI renders it in a headless
Chrome against [`observed_fleet`](../../examples/README.md#the-capability-matrix), tab by tab, and checks each tab
reads `ok` and that a page served from an origin the nodes do not list reads `blocked`
(`scripts/ops-console-render.sh`).

## What it guarantees, and what it does not

- No guarantee or rule id of its own; what a tab shows is each node's own answer, under that route's scope.
- **Not built:** a server, a proxy or polling, by decision (D16, D28); history; a write of any kind; telling an
  unreachable node from one that does not list the page's origin — both read `blocked`.
- **Evidence:** the [what-is-proven](../operations/what-is-proven.md) ledger has no row for the console on its own.

## Go deeper

- **Guide:** [the console's README](../../examples/ops_console/README.md).
- **Runbook:** [observability § The Ops Console](../operations/observability.md#the-ops-console).
- **Design:** decisions D16 and D28, row Y3 of [`plans/adopter-surface.md`](../plans/adopter-surface.md).
- **Code:** [`examples/ops_console/`](../../examples/ops_console/README.md) and the gateway's CORS layer,
  [`src/agent/http/cors.rs`](../../src/agent/http/cors.rs).
