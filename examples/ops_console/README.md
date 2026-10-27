# Ops Console

## Objective

A read-only **dashboard over any gateway-enabled Mycelium nodes' existing routes**, in one place — so you
can watch a fleet (a demo, a showcase, or a real one) without modifying it. It is **a static page**: it
runs in your browser, reads each node you list **directly**, and merges their answers itself. There is no
console server and no proxy, and nothing polls the fleet for it. It is an example, **not a product** and
not a control plane: the library-not-platform line holds — a cluster is emergent from reachability + CA
admission, there is no central runtime, and a customer forks this or points Grafana at `/metrics`.

**The rule it keeps** (decisions D16, D28 in [`docs/plans/adopter-surface.md`](../../docs/plans/adopter-surface.md)):
if a panel ever needs a process that polls every node and holds the result, the answer is a new route on a
node, never that process.

## How to run

A browser reads another origin's answers only if that origin says it may, so each node lists the origin
the page is served from — **`gateway_cors_origins`**, off by default, an exact-origin allow-list, answered
only on the node's read-only routes ([`docs/operations/observability.md` § The Ops Console](../../docs/operations/observability.md#the-ops-console)).
The bearer, if the nodes require one, is entered in the page and still decides what it may read.

The three-node fleet this directory is tested against sets all of that:

```bash
cargo run --example observed_fleet --features metrics          # gateways :9061 :9062 :9063, origin http://127.0.0.1:8099
python3 -m http.server 8099 --bind 127.0.0.1 --directory examples/ops_console
# open http://127.0.0.1:8099/?nodes=127.0.0.1:9061,127.0.0.1:9062,127.0.0.1:9063
```

Any static file host works; the origin it serves from is what the nodes must list. To point the page at
your own nodes, set on each (TOML or environment):

```toml
gateway_cors_origins = ["http://127.0.0.1:8099"]       # GOSSIP_GATEWAY_CORS_ORIGINS=http://127.0.0.1:8099
```

Then list them in the **nodes** box (`host:port` for HTTP, or a full `https://…` URL), or in the URL:
`?nodes=a:9061,b:9061`, `&tab=Guarantees`, `&once` for one snapshot with no refresh (what CI's headless
browser dumps), and `#token=…` for a bearer (a fragment never leaves the browser). A showcase's **"⚙ Ops Console"** button opens `?target=host:port` — one node — and the showcases
list the console's origin themselves. A node that does not list the page's origin answers nothing the
page can read, and its panel says so (`blocked`) rather than going blank.

## What it reads

Every panel reads only routes the nodes already have. Each tab, and the routes it renders — this table is
the page's own `PANELS` list (a test holds them equal) and what CI renders, tab by tab, against the fleet:

| Tab | Route(s) | Shows |
|---|---|---|
| Overview | `/stats` | node runtime and tripwire counters; across the nodes, one row each |
| Fleet | `/gateway/fleet` | each node's view of the fleet (members, load, opacity, convergence) |
| Diagnose | `/gateway/diagnose` | the Legible-Emergence **fleet narrative**, as each node tells it |
| Guarantees | `/gateway/guarantees` | each guarantee's resolution on each node, and the profile each selected |
| Governors | `/gateway/govern` | the governor snapshot: control profile, tuning parameters, electorates seen |
| Tuples | `/gateway/tuple/overview` | the tuple-space companion's stages, from each node's gossip view |
| Integrity | `/stats` · `/gateway/kv/keys?prefix=grp%2F` · `/gateway/kv/keys?prefix=sys%2Fidentity` · `/gateway/fleet` | the preconditions of the refusals (below) |
| Audit | `/gateway/audit` | the tamper-evident **signed audit trail** (nodes built `--features compliance`) |
| KV | `/gateway/kv/keys` | the key set by prefix, per node — a column that differs is a view that has not converged |
| Metrics | `/metrics` | Prometheus series (nodes built `--features metrics`) |

The **"↗ label"** link reads `/gateway/kv?key=ui%2Fviz` and `ui%2Flabel` — the `ui/viz` convention a
showcase uses to advertise its own page. Each of these paths is on the gateway's cross-origin read list,
and no other path is ever read.

Each node's section carries one state: `ok` (rendered from the routes above), `blocked` (unreachable, or
the node does not list this page's origin — a browser cannot tell the two apart), `unauthorized` (401/403:
no bearer, or one without the route's scope), `absent` (404: a route this node's build or companions do not
serve, named on the panel) or `error`.

## The Integrity tab

The coordination and identity properties are **refusals** — the node declining to decide alone, to trust
transitively, or to accept an identity it cannot authenticate. *You cannot watch a thing not happen*, so
the tab shows their **preconditions**: electorates (would an election here work, or be refused?), sealed
identity coverage (does every node publish the record `require_identity_proofs` accepts? — three states,
because a fleet with no identities is running without TLS, not "not ready"), role concentration (P10), and
the tripwires. Trust non-transitivity and leadership fencing have no panel: they are demonstrated by
`federation_trust_is_not_transitive` and `coordination_integrity`. A tile claiming "no breaches" would
assert far more than one node can see.

## Dev notes

- **Read-only, and it cannot be otherwise:** the page makes one kind of request, a `GET` to a listed node's
  route, and a gateway answers cross-origin reads on read-only routes only.
- **Not built:** authentication of its own — the bearer is the gateway's, sent as entered; any packaging as
  a product.
- Source: [`index.html`](index.html). The fleet: [`../observed_fleet.rs`](../observed_fleet.rs). CI's
  rendering: `scripts/ops-console-render.sh`.
