# 2026-09-18 — item 2 PR 9: the release gate's choreography

**What shipped.** `GossipAgent::connected_peers()` (topology.rs; the connection table as the *traces* leg),
`GatewayPool::retire` + `FederationClient::retire_gateway`, the harness's two new legs (`consensus/` over keys
and values; connection tables) with a positive control in the non-vacuity test, and the choreography test
itself. Docs: record §13 (row 9 done, row 10 named, where the gate stands and what in-process leaves open),
security, guide 17, plan, testing, changelog, history.

**How the traces leg was chosen.** The record says *membership tables, consensus state and traces*. There is no
node-attributed span in the codebase and no global subscriber, so a captured tracing stream cannot say which
node emitted a line, and the log lines that name a peer name only the remote end; a per-process capture could
not distinguish "B's node accepted a B peer" from "A's node accepted a B peer". The per-node event ring records
detector events, not membership. What the transport itself keeps is `peer_writers` — the map of peers a node
holds a writer for — and that is the record a trace would show: whom bytes were written to. Exposed as
`connected_peers`, checked in the harness's third leg, and proved non-vacuous by the merged pair appearing in it.

**How the enforced profile was made real.** `DomainProfile::Enforced` requires `tls` and SWIM off. Two meshes
with two `auto_cert_dir`s get two auto-generated CAs; a node holding B's CA bootstrapped at A never appears in
A's tables or connection tables. That plant is timing-bounded (1.5 s); gw2 joining A with A's CA is the positive
control on the same mechanism.

**Findings.**
- *The pool keeps no health memory* (PR 5, by design: a silent gateway is a per-call outcome). After a gateway
  is replaced, repeatable calls fail over past the dead one each time and at-most-once calls are `DeliveryUnknown`
  each time. Retirement had to be explicit; `retire` removes the gateway and its slots.
- *TLS formation needs fast pings* — peer registration is on Ping receipt; defaults left the meshes unformed at
  8 s. The WS1 TLS test already knew; now testing.md says so where the next gateway test will look.
- *A secure gateway waits for the provider's caller-context marker* (item 7), which gossips like the
  capability. Polling only for the capability gave `-32021`.
- *An export needs a skill behind it.* A grant is not a service; `-32001 skill not found` is the gateway's
  answer, not a federation refusal.

**Not claimed.** Process isolation and a real network severance (the Docker two-mesh suite is row 10); a signed
catalogue reply; SDK verbs; anything under the `sim` kernel.
