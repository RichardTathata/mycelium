## [2026-10-17] ingest | The activation egress proxy (post-360 rows L and M, the items they left not built)

Three items rows L and M recorded as *not built* close together: an `[[activation]]` command's credential (it had no
path to the egress gate and inherited the stem's environment), limits by hostname (both backends limit a port), and a
network allow-list that changes while the stem runs. One mechanism covers all three: `[activation.confine.egress]`
makes the stem run a forward proxy on loopback for that activation, confine the command to the proxy's port, and
start it in a cleared environment that names the proxy and a per-proxy token.

The design choice worth keeping: **the hop that carries a credential is plain HTTP to loopback, and the proxy
originates the TLS.** A command writes `http://api.example.com/…`; the proxy matches the host to an `https://` entry
and sends it on through `EgressCredentials::send_once`, which attaches the value only where the binding authorises.
`CONNECT` stays for `https://` URLs the command dials itself, allow-list only. TLS interception was rejected: a
minted CA is a key able to impersonate every host. The list lives in the proxy behind one lock (lock-order row 64),
replaced by `Stem::set_activation_egress_hosts` or `mycelium-stem`'s `SIGHUP`; KV (fleet-writable) and a gateway
route (a door for a node-local concern) were rejected. The proxy follows no redirect, so every hop meets the list.

Found on the way: the cleared environment's pass-through list deduplicated only adjacent names, so `PATH` named
twice was passed twice (`the_command_environment_is_cleared_and_names_the_proxy`, seen failing first).

Stated, not closed: under Landlock the proxy's port is reachable on any address (`stem.activation_egress` reads
`not_configured` on Linux); names, not resolved addresses; no credential inside `CONNECT`; no path limit; request
bodies buffered to 16 MiB; `credentials` and `env` fixed for the stem's life.

Pages: `dev/security.md`, `dev/concurrency/lock-order.md` (row 64). Repo: `docs/reference/unit-file.md`
§`[activation.confine.egress]`, `docs/operations/crown-jewel.md` § Credentials attached at egress, guide 09, the
concepts glossary, `deprecations.md` §51, the plan's rows L and M (§3.5, §3.6).
