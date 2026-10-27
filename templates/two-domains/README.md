# two-domains

Two fleets, each its own domain under `profile = "secure-multi-domain"`, calling each other across one edge. The food
bank (`foodbank-gw`, `foodbank-1`) serves `pantry/stock` and `pantry/reserve`; the community kitchen (`kitchen-gw`,
`kitchen-1`) serves `menu/today`. Each domain has its own CA, its certificates issued off-node; each gateway is HTTPS
behind a named-token table, runs an edge that signs its catalogue, binds every call's body and meters the partner, and
holds a client for the other, pinned to that gateway's key; each node's egress list names only what it may reach. A
call made at one gateway by its local caller reaches the other domain's provider carrying that caller's name, where
the provider's own policy decides it again.

**Until the next MINOR release, this project builds only against a checkout of the repository** (v2.38.2 is a security PATCH without it): `secure-multi-domain`,
`mycelium tls ca` and the doctor this project runs are newer than every published tag, so a `Cargo.toml` pinned to one
of them does not build. Point its `mycelium` dependency at a checkout (`mycelium = { path = "…", features = [...] }`)
and use that checkout's `mycelium` binary, as CI does; from the next MINOR release the pin `mycelium init` writes is enough.

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
certificate, its key and the CA's certificate, nothing else. Each domain has two keys: its **signing** key, which signs
its catalogue and its calls and is read by its gateway alone (`secrets/<domain>.seed`), and its **operator** key, the
root its own mandates verify under, whose seed no node reads (`secrets/<domain>.operator.seed`; every node reads
`exchange/<domain>.operator.pub`). Every key and token file is written owner-only (0600). A gateway's tokens reach it
through `GOSSIP_GATEWAY_NAMED_TOKENS`, which `fleet.sh` sets from `secrets/<node>.tokens`.

**Refused, each shown in CI.** A partner holding too many calls at once is refused, not queued — each gateway carries
at most two of its partner's (`max_in_flight_per_partner`); a third concurrent `bash fleet.sh call kitchen-gw
pantry/reserve` answers `already carrying 2 calls for kitchen.example`. A body rewritten after it was signed is refused
at the partner's edge: `cargo run -- tamper kitchen-gw pantry/stock` presents a credential the kitchen signed for one
body with another, and the food bank's edge answers that the credential binds a different body. A captured call sent
again is refused too: `cargo run -- replay kitchen-gw pantry/stock` presents one credential and its body twice, and the
food bank's edge serves the first and answers the second `-32003`, *replayed*.

**One domain** is this scaffold with the second fleet deleted (D26): remove `nodes/kitchen-gw.toml`,
`nodes/kitchen-1.toml` and the two `[domains."kitchen.example"…]` tables; from the food bank's table remove `grants`,
`trust` and `permit_actor`, and its `[domains."foodbank.example".partner."kitchen.example"]` table; set
`profile = "secure-single-domain"` in the two node files left. `fleet.sh` reads the nodes and domains from `nodes/`, so
`setup`, `start-all`, `doctor` and `guarantees` then run the one domain.

**What this does not establish.**
- On one machine every directory is readable by every process: the separation `ca/` and `certs/<node>/` show is the
  one each real host has, where a node is given only its own directory.
- A credential is refused a second time **at the gateway that served it**, until it expires: each gateway keeps its own
  record, in memory, so a domain with several gateways serves one credential once at each, and a restart forgets it.
- The policy is the reference evaluator over the rules `federation.toml` derives; a deployment attaches its own adapter
  in `src/authority.rs`. No mandate is presented here, so the execution authority is attached and decides nothing yet.
- `mycelium doctor nodes/<node>.toml` reads the file alone and cannot see what the program attaches in code, so it
  refuses on the evaluator, the journal and the edge; `bash fleet.sh doctor <node>` is the same doctor over the node as
  the program builds it — and, because it attaches what the program attaches, it cannot run beside the node itself:
  the running node holds its evidence journal, and the doctor says so and stops.

Every node's doctor, and a wrong pin. The doctor attaches what the program attaches — the node's evidence journal
among them, which a running node holds — so it reads stopped nodes. With a wrong pin the kitchen's gateway refuses the
food bank's key during the handshake, so the connection a call needs is never made: `connect` is refused by the pin,
and a `call` then answers that the link is down — neither sends a byte of a call:

```bash
bash fleet.sh stop
for n in foodbank-gw foodbank-1 kitchen-gw kitchen-1; do bash fleet.sh doctor $n | grep -E '^(profile|start)'; done   # start(): would start
printf '%064d' 0 > exchange/foodbank-gw.pin && bash fleet.sh start-all
bash fleet.sh connect kitchen-gw              # 502 … "presented an unpinned key", "sent":false
bash fleet.sh call kitchen-gw pantry/stock    # 409 … "link: Down", "sent":false
```

To restore the right pin — the food bank gateway's certificate is the source of it — and reconnect:

```bash
bash fleet.sh stop kitchen-gw && "$MYCELIUM" tls pin certs/foodbank-gw/*.cert.pem > exchange/foodbank-gw.pin
bash fleet.sh start kitchen-gw && bash fleet.sh connect kitchen-gw && bash fleet.sh call kitchen-gw pantry/stock
```
