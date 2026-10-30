# The pilot kit

↑ [Operations](../README.md) · the runbook that names it: [customer-pilot.md](../customer-pilot.md)

**What it is:** everything a pilot needs on its first day, so that the counterparty is the only piece missing. You
start two fleets, each its own domain, and call across the edge between them. You run the substrate's fault classes
against the fleet's declaration. You get back a report whose sections are the guarantees the pilot exercised, written
from two outputs alone: `mycelium doctor` on each node and `mycelium test --faults` over the declaration. CI runs the
commands on this page as they are written, on every push, each block in a fresh shell (the `Scaffold` job, step
*Pilot kit*), so a block that fails here fails there first.

**What it is not:** the deployment. Everything below runs on one machine, under no one's name; the *named
deployment* line on [what-is-proven.md](../what-is-proven.md) stays open until a counterparty runs Mycelium on its own
hosts and signs the report. This kit is how that run starts, not a substitute for it.

| File | What it is |
|---|---|
| `README.md` | this walk; the blocks marked `bash pilot` are what CI runs |
| `env.sh` | the preamble every block sources first: it checks `MYCELIUM_REPO` and `PILOT` and derives `KIT` and `MYCELIUM` from them, so each block runs in a fresh shell |
| `units/pantry.toml`, `units/menu.toml` | the declaration `test --faults` builds its fleet from: what each domain's provider offers, the mandate it holds and the rules its policy decides by, one of them a grant-required reservation. The kit's own; the scaffold has no `units/` |
| `faults.toml` | the node configuration `test --faults` builds its fleet from, with the `[tls]` and `[persistence]` the restart, recovery and grant classes need |
| `report.py`, `report-template.md` | the report, from the two outputs; the template is the report's shape |

## 0 · The first checkpoint: the hour, on the published node

Before the two domains, the one-node path: [tutorial 7, *the first hour*](../../guide/tutorials/07-the-first-hour.md),
which CI's `one-hour` job walks on each release from the published binary, with no toolchain. It needs:
- the network, to download a release tarball and its `SHA256SUMS` from GitHub;
- `pip`, to install the Python SDK from the release's source archive;
- on macOS or as a non-root user, the tutorial's own *On another machine* notes. These cover the platform name,
  `shasum` in place of `sha256sum`, an install directory on your `PATH`, and a virtual environment where the OS
  manages Python.

If its six steps run on the pilot's machine as they do in the job, the machine, the network and the release are what
this kit assumes. If one fails there, stop and fix it there: everything below needs more than that path does.

## 1 · What you need

- A machine with `bash`, `git`, `python3` 3.10 or later and a Rust toolchain (1.96 or later: `rustup` installs one).
  Linux and macOS both work. Ports 7961, 7962, 7971, 7972, 8461 and 8471 on loopback must be free.
- A checkout of the repository's `main`. The kit is on `main` from the release after v2.38.3. **Until the next MINOR
  release, the two-domain scaffold builds only against a checkout**: `secure-multi-domain` and the doctor it runs are
  newer than every published tag (v2.38.2 and v2.38.3 are security PATCHes without them). So the walk builds
  `mycelium` from the checkout and points the scaffold at it, as CI does:

```bash
git clone --branch main https://github.com/RichardTathata/mycelium && cd mycelium
export MYCELIUM_REPO="$PWD"        # every block below reads it, through env.sh
export PILOT="$HOME/pilot"         # where the pilot's fleet is written; it must not exist yet
```

Each block below starts by sourcing `env.sh`, so it needs only these two variables; a block run in a new terminal
works once they are exported again. The walk writes the fleet under `$PILOT`; cargo also writes its build directory
(`$MYCELIUM_REPO/target`, or `CARGO_TARGET_DIR`; the scaffold's own build goes to `$PILOT/target` unless
`CARGO_TARGET_DIR` is set) and its download caches under `~/.cargo`.

## 2 · Build the binary and write the scaffold

The `mycelium` binary with the features the fault classes need, and the two-domain scaffold from it. `init` prints
`pinned to v<version>`: it pins the scaffold to the tag of the binary's own version, which is what a user of a
release gets. The `sed` line then replaces that pin with the checkout, because the tag cannot build this scaffold
yet (§ 1). Last, the kit's declaration and fault configuration are copied in beside the scaffold's own files.

```bash pilot
. "$MYCELIUM_REPO/docs/operations/pilot-kit/env.sh"
cd "$MYCELIUM_REPO"
cargo build --bin mycelium --features cli,tls,compliance,llm
"$MYCELIUM" init "$PILOT" --template two-domains
cd "$PILOT"
sed -i.bak "s|^mycelium = { git = .*|mycelium = { path = \"$MYCELIUM_REPO\", features = [\"tls\", \"a2a\", \"compliance\"] }|" Cargo.toml
rm Cargo.toml.bak
grep -q "path = \"$MYCELIUM_REPO\"" Cargo.toml
cp -R "$KIT/units" "$KIT/faults.toml" .
```

The scaffold's own [README](../../../templates/two-domains/README.md) says what each of its files is; `units/` and
`faults.toml` are this kit's.

## 3 · Two domains, one call each way

`setup` first compiles the scaffold's node program: the first time it builds the dependencies, which takes a minute
or more. Then it mints each domain's CA, issues every node's certificate off-node, and makes the domain keys, the pins
and the named tokens. `start-all` starts the four nodes in the background and returns once each is ready; each node's
log is `data/<node>.log` and its process id `data/<node>.pid`. Each gateway then fetches its partner's signed
catalogue and calls the partner's export as its named local caller, whose name reaches the provider in the other
domain:

```bash pilot
. "$MYCELIUM_REPO/docs/operations/pilot-kit/env.sh"
cd "$PILOT"
bash fleet.sh setup
bash fleet.sh start-all
bash fleet.sh connect kitchen-gw | tee connect.log
grep -q '"link":"ready"' connect.log
bash fleet.sh connect foodbank-gw
bash fleet.sh call kitchen-gw pantry/stock | tee call.log
grep -q 'for federation:kitchen.example/token:kitchen/planner' call.log
bash fleet.sh call foodbank-gw menu/today 'menu?' | tee call-back.log
grep -q "for federation:foodbank.example/token:foodbank/steward" call-back.log
bash fleet.sh stop
```

A `200` line naming the caller on each call is the edge working both ways. The refusals the scaffold shows — a wrong
pin, a rewritten body, a replayed credential, an exceeded slot cap — are in its README and in CI's `Scaffold` job.
**This step feeds nothing into the report:** the report is built from `doctor` and `test --faults` alone, so the edge
appears there only as `doctor` reports its `fed.*` guarantees, not as evidence from this call.

## 4 · `doctor` on every node

`doctor` says what `start()` would say about each node as the scaffold's program builds it, attachments included.
It names one guarantee per line, and each line names its row of the
[shared-responsibility matrix](../shared-responsibility-matrix.md#from-a-doctor-line-to-its-row) (`→ matrix: …`,
or `no row` with the reason). It reads stopped nodes (a running node holds its evidence journal), which is why
step 3 ends with `stop`:

```bash pilot
. "$MYCELIUM_REPO/docs/operations/pilot-kit/env.sh"
cd "$PILOT"
mkdir -p doctor
for n in foodbank-gw foodbank-1 kitchen-gw kitchen-1; do bash fleet.sh doctor "$n" > "doctor/$n.txt"; done
grep -h '^start(): ' doctor/*.txt
```

Each node should read `start(): would start` under `profile: secure-multi-domain rev 1`. A node that reads anything
else names the unmet guarantee on its own line; fix that before going on — the report would only repeat it.

Two lines worth knowing before you read them:
- `id.revocation_authority` reads `not_configured` on every node while `id.ca_key_off_node` reads `enforced`. That is
  `doctor`'s limit, not the node's: whether a revocation of this node's keys can be trusted is judged from the node's
  certificate, which loads only at `start()`. A node `doctor` builds is never started. A running node's own report
  (`bash fleet.sh guarantees <gateway>`) judges it.
- `build features` are the scaffold program's: the features its `Cargo.toml` names, with what they pull in. They are
  not the `mycelium` binary's from step 2, which is why `llm` is absent.

## 5 · The fault classes against the declaration

`test --faults` builds an in-process fleet from `units/` and `faults.toml` and runs every class this build has:
`revoke`, `restart`, `partition`, `recovery` and `grant` run and must pass; `replay` reads `not_in_build` (it needs a
`sim` build) and `policy-diff` reads `not_configured` (it needs a second revision to compare), and neither fails the
run because neither was asked for by name. It takes about a minute, most of it the partition class waiting out the
offers' evaporation window:

```bash pilot
. "$MYCELIUM_REPO/docs/operations/pilot-kit/env.sh"
cd "$PILOT"
"$MYCELIUM" test --faults units --config faults.toml --format json > faults.json 2> faults.log
python3 -c "import json,sys; r=json.load(open('faults.json')); bad=[c['class'] for c in r['classes'] if c['state']['state'] not in ('passed','not_in_build','not_configured')]; sys.exit('not passed: %s' % bad if bad else 0)"
```

The fleet runs under `dev` whatever the node files declare: a profile is `doctor`'s to check, and the classes test
what holds under fault. It has no gateway port and no federation edge. It writes nothing of yours: its persistence and
TLS material sit under a temporary directory the run removes. `faults.log` holds the nodes' own log lines;
`--format text` prints the same run for a person.

## 6 · The report

```bash pilot
. "$MYCELIUM_REPO/docs/operations/pilot-kit/env.sh"
cd "$PILOT"
python3 "$KIT/report.py" --doctor doctor/*.txt --faults faults.json > pilot-report.md
grep -c '^### ' pilot-report.md
```

`pilot-report.md` has these parts:
- **The fleet** as `doctor` read it, and the classes as they ran.
- **The exercised guarantees, in two groups.** First, those a class exercised where its own fleet enforced them; the
  class's checks are evidence for these. Then those a class touched where its fleet read them `not_applicable` or
  `not_configured`; the class passing is not evidence for these. Each section gives:
  - the guarantee's promise;
  - its state on each node, with `doctor`'s reason where it is not enforced;
  - its state in the class's fleet;
  - the matrix row its `doctor` line names.
- **The rest:** the guarantees `doctor` reports that no class exercised, and those no node can verify about itself.

The states are the guarantee report's words, never a paraphrase. Each section has one line for the pilot to write,
*Observed in the pilot*: what the counterparty saw on their own hosts. That line, signed, is what moves the
named-deployment line on the proof page. The kit writes the rest.

## Starting over

The scaffold's `setup` runs once: to begin again, run `bash fleet.sh stop` in `$PILOT`, remove `$PILOT`, and run
steps 2 to 6. One domain instead of two is the scaffold with the kitchen's files deleted, as its README says; the
kit's steps then run with the kitchen's lines left out.
