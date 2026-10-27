# two-domains

Two fleets, each its own domain under `profile = "secure-multi-domain"`, calling each other across one edge. The food
bank (`foodbank-gw`, `foodbank-1`) serves `pantry/stock` and `pantry/reserve`; the community kitchen (`kitchen-gw`,
`kitchen-1`) serves `menu/today`. Each domain has its own CA, its certificates issued off-node; each gateway is HTTPS
behind a named-token table, runs an edge that signs its catalogue, binds every call's body and meters the partner, and
holds a client for the other, pinned to that gateway's key; each node's egress list names only what it may reach. A
call made at one gateway by its local caller reaches the other domain's provider carrying that caller's name, where
the provider's own policy decides it again.

```bash
export MYCELIUM=$(command -v mycelium)   # the published binary: `tls ca`, `tls issue`, `tls pin`
bash fleet.sh setup                      # CAs, certificates, domain keys, pins, tokens (ca/, certs/, secrets/: never commit)
bash fleet.sh start-all                  # four processes, logs in data/<node>.log; returns once each node is ready
bash fleet.sh connect kitchen-gw         # the kitchen fetches the food bank's signed catalogue
bash fleet.sh call kitchen-gw pantry/stock   # 200 … "for federation:kitchen.example/token:kitchen/planner: 40 crates …"
bash fleet.sh connect foodbank-gw && bash fleet.sh call foodbank-gw menu/today 'menu?'   # and the other way
bash fleet.sh guarantees kitchen-gw      # the running node's own report: the seven fed.* enforced
```

What each file is:

| File | What it holds |
|---|---|
| `nodes/<node>.toml` | the node's configuration: the profile, the enforced domain profile, `[tls]` naming the certificate `setup` issued, `[egress]`, `[persistence]`, the gateway's HTTPS |
| `federation.toml` | the edge: what each domain exports and grants, the partner key it trusts, where its client dials and the key it pins, and which caller each side's policy permits |
| `src/main.rs`, `src/fleet.rs`, `src/authority.rs` | the node program: the file, plus what it attaches in code before `start()` — an evaluator, an evidence journal, an execution authority, the edge and the client |
| `fleet.sh` | the operator's steps, one per line above |
| `exchange/` | what the two operators hand each other: each gateway's pin and each domain's public key — never a private key |

`ca/` is the issuer's: it holds each domain's CA key and no node is pointed at it. `certs/<node>/` holds that node's
certificate, its key and the CA's certificate, nothing else. A gateway's tokens reach it through
`GOSSIP_GATEWAY_NAMED_TOKENS`, which `fleet.sh` sets from `secrets/<node>.tokens`.

**Refused, each shown in CI.** A partner holding too many calls at once is refused, not queued — each gateway carries
at most two of its partner's (`max_in_flight_per_partner`); a third concurrent `bash fleet.sh call kitchen-gw
pantry/reserve` answers `already carrying 2 calls for kitchen.example`. A body rewritten after it was signed is refused
at the partner's edge: `cargo run -- tamper kitchen-gw pantry/stock` presents a credential the kitchen signed for one
body with another, and the food bank's edge answers that the credential binds a different body.

**One domain** is this scaffold with the second fleet deleted (D26): remove `nodes/kitchen-gw.toml`,
`nodes/kitchen-1.toml` and the two `[domains."kitchen.example"…]` tables; from the food bank's table remove `grants`,
`trust` and `permit_actor`, and its `[domains."foodbank.example".partner."kitchen.example"]` table; set
`profile = "secure-single-domain"` in the two node files left. `fleet.sh` reads the nodes and domains from `nodes/`, so
`setup`, `start-all`, `doctor` and `guarantees` then run the one domain.

**What this does not establish.**
- On one machine every directory is readable by every process: the separation `ca/` and `certs/<node>/` show is the
  one each real host has, where a node is given only its own directory.
- A credential presented a second time, with the same body, inside its window is served again: the edge checks the
  signature, the body, the window and the grant, and keeps no record of credentials it has seen.
- The policy is the reference evaluator over the rules `federation.toml` derives; a deployment attaches its own adapter
  in `src/authority.rs`. No mandate is presented here, so the execution authority is attached and decides nothing yet.
- `mycelium doctor nodes/<node>.toml` reads the file alone and cannot see what the program attaches in code, so it
  refuses on the evaluator, the journal and the edge; `bash fleet.sh doctor <node>` is the same doctor over the node as
  the program builds it.
- The tag `Cargo.toml` pins is the release `mycelium init` came from; a release before `secure-multi-domain` shipped
  cannot build this project.

Every node's doctor, and the call refused when the pin is wrong. The doctor attaches what the program attaches — the
node's evidence journal among them, which a running node holds — so it reads stopped nodes:

```bash
bash fleet.sh stop
for n in foodbank-gw foodbank-1 kitchen-gw kitchen-1; do bash fleet.sh doctor $n | grep -E '^(profile|start)'; done   # start(): would start
printf '%064d' 0 > exchange/foodbank-gw.pin && bash fleet.sh start-all
bash fleet.sh connect kitchen-gw   # 502 … "presented an unpinned key" — refused before any byte of a call is sent
```
