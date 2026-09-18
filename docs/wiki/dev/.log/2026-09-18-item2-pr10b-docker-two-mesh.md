# 2026-09-18 — item 2 PR 10b: the two-mesh Docker suite

**What shipped.** `examples/federation_node.rs` (one binary, four roles: `member`, `gateway`,
`probe`, `keys`), `docker/docker-compose.federation.yml`, `docker/Dockerfile.federation`,
`tests/integration/run_federation.sh`, `make test-federation` / `test-federation-clean` /
`federation-keys`, and a `federation` job in the cluster-suites workflow.

**Why it exists.** PR 9 ran item 2's release-gate choreography in one process and the record said
plainly what that left open: the meshes shared an address space, and "severing every link" was a
shut-down gateway. This suite removes both caveats — one container per node, and the link is cut
with `docker network disconnect`. It is the only place the gate is claimed without a caveat.

**The design decision worth recording: four networks, not three.** The obvious topology is
`alpha-net`, `beta-net` and an `edge` network carrying the federation path. But the runner severs
the link by disconnecting the probe from `edge`, and if the runner drove the probe over that same
network it would cut its own control channel in the same instant — the test would hang rather than
observe. So the probe also sits on `control`, which only the runner shares, and the runner is **not
on `edge` at all**. The harness's plane is deliberately not the path under test. This was caught by
reading the first draft, not by running it.

**The second thing the first draft got wrong.** The admission plant (a node holding beta's CA,
bootstrapped at alpha) was a compose `profile` the runner started with `docker compose` — but the
runner image has `docker-cli` and no compose plugin, and the compose file is not mounted into it.
The plant now runs with `docker run`, which is why the image (`mycelium-federation-node:test`) and
the CA volumes (`mycelium-fed-{alpha,beta}-ca`) have pinned names in the compose file. The plant
also asserts the rogue container is *up* before asserting it has no peers, so the negative is about
admission rather than about a container that died.

**The defect the suite found, which is the point of building it.** With the edge network
disconnected, four checks failed because the client **hung** rather than returning. A *refusing*
partner sends a TCP reset and `reqwest` errors immediately; a **blackholed** one — interface gone,
default route still present, which is exactly what a real severance looks like — sends nothing, and
an unbounded connect waits forever. PR 5 promises `DeliveryUnknown` for a silent gateway, and a
client that never returns cannot deliver that verdict, so this was a contract defect rather than a
test artefact. Fixed by bounding the client's HTTP: `DEFAULT_CONNECT_TIMEOUT` (5 s),
`DEFAULT_REQUEST_TIMEOUT` (30 s), and `FederationClient::with_timeouts`. Pinned in-process by
`a_blackholed_gateway_is_unknown_within_a_bound_rather_than_hanging`, which points a client at
`192.0.2.1` (RFC 5737 TEST-NET-1, reserved and unroutable) and asserts the **bound**, not the error —
on a host that answers `ENETUNREACH` the call fails fast and the bound still holds.

**Why the in-process test could not have found it.** There the "severance" is a shut-down gateway,
which refuses; a refusal fails fast, so the unbounded wait never showed. It took a real network
severance to produce a partner that is neither up nor refusing — which is the whole reason the
record asked for this suite rather than treating the in-process choreography as sufficient.

**Two more findings, smaller.** The node config set `health_check_interval_secs` equal to
`reconnect_backoff_secs`, which `validate()` warns about on every start (a peer can be evicted
mid-backoff and never reconnect); the interval is now 4 s, in the Docker suite and in the
in-process choreography that copied the value from the WS1 TLS test. And the runner's own script
had two bugs that cost a full run each: `${2:-{}}` closes the expansion at the first `}` and
appends a stray one, silently corrupting every request that carried a body (which is why `connect`
passed while `call` did not), and `sh -c "post …"` spawns a shell where the helper function does not
exist — 26 checks failed for that reason alone while the code under test was fine. The `check`
helper now prints the last probe body on failure.

**The keys are derived, not transcribed.** `FED_ROLE=keys` prints the public half of a seed, and
`make federation-keys` prints both. A mistyped public key would surface as `BadSignature`, which
reads like a defect in the thing under test rather than a wrong fixture.

**What the suite asserts.** Every check reads a table from a node's own `/fed-admin` surface — never
a log line. The three legs of non-merger (membership, the `cap/ grp/ sys/ consensus/` namespaces
over keys *and* values, and the connection table) run at steady state and again after the full
cycle. In between: discover (the signed catalogue is the grant, not the export list), invoke (the
provider is told `federation:beta.example/svc/billing`), an ungranted export refused before any
byte, a consensus round in each mesh that the other never learns, gw-1 stopped and failed over, the
edge severed (at-most-once `DeliveryUnknown` via gw-1 only; repeatable unknown having tried both;
discovery cannot refresh; link `Down`), both meshes still gossiping and committing, the grant
changed mid-partition, reconnect (refused until discovery refreshes; the refreshed catalogue *is*
the changed grant), retirement of the dead gateway, and the admission plant.

**The admin surface is test-only, and shaped so.** It mounts through `with_http_routes` outside
`/gateway/`, so it is unauthenticated by construction — which is the library's documented rule for
merged routers, not an oversight. It exists for a runner on a private test network. Nothing in
`src/` depends on it.

**What this still does not prove.** TLS on the *federation edge* (the gateways' HTTP is plain
inside the compose network; intra-mesh traffic is TLS under each domain's CA, which is what the
enforced profile requires). A hostile network between domains. More than two domains. Anything
under the `sim` kernel.
