# 7 · The first hour — from the published node toward a governed fleet, with no toolchain

↑ [Developer tutorials](README.md) · [Example portfolio](../../../examples/README.md)

**Question:** how far does a machine with no Rust toolchain get in an hour — a node running from a published
release, an agent in its own language declaring a capability, a call the gateway refuses by name, a grant that is
default-deny and holds across a restart, a lost acknowledgement recovered without a second effect, and a readiness
report naming what the fleet enforces?

**The claim is measured, or it is not made.** CI's `one-hour` job
([`.github/workflows/one-hour.yml`](../../../.github/workflows/one-hour.yml)) walks exactly this path on each
release it is run against — on v2.37.0 it measured steps 1, 2, 3 and 6; steps 4 and 5 join from v2.38.0, the first release
that carries their fault classes — in a plain Python container, from the first `curl` to the last `doctor`, and prints the seconds per step to
the job summary. The commands below are the job's — a test
([`tests/first_hour_tutorial.rs`](../../../tests/first_hour_tutorial.rs)) diffs each fenced block here against the
workflow's step, so a reader and the job walk one path and the job's green run is the proof that the commands do not
fail as written. The number the documentation prints for *one hour* is what that job measured on the most recent
release, rounded up to the next ten minutes; the plan's rule is in
[`adopter-surface.md` § 1.1](../../plans/adopter-surface.md). Nothing here needs a model or an API key.

**Before you start** — Linux x86_64 (the job's container; the release also ships Linux aarch64 and macOS aarch64,
whose tarball names differ), `bash`, `curl`, `python3` 3.10 or later with `pip`, and a release tag. **Steps 4 and 5
need a release that carries the `grant` and `recovery` classes** — 2.38.0 or later; on 2.37.0 they fail
with `unknown argument: test` (that release has no `test` command), and steps 1, 2, 3 and 6 run:

```sh
TAG=v2.38.3                       # the newest release; the job reads it from the Release that triggered it
REPO=RichardTathata/mycelium
```

**On another machine.** The commands are the job's, on Linux x86_64 as root in a clean container. Elsewhere:
substitute `linux-aarch64` or `macos-aarch64` for `linux-x86_64` in the tarball name; on macOS use
`shasum -a 256 -c SHA256SUMS` if `sha256sum` is missing; as a non-root user install to a directory on your `PATH`
you own (`install -m 0755 … ~/.local/bin/mycelium`); with a Python your OS manages (PEP 668:
`externally-managed-environment`), create a virtual environment first (`python3 -m venv .venv && . .venv/bin/activate`);
on Linux the binary needs `libssl3` and `ca-certificates`, which the job installs before its clock starts.

## Step 1 · The published node: download, verify the sum, run it

Every `v*` tag publishes `mycelium`, `mycelium-stem` and `mycelium-artifact` with `SHA256SUMS`
([feature page](../../features/published-node.md)). The sum is integrity, not a signature — the
[installation chapter](../installation.md#the-published-node) says what that does and does not establish.

```bash one-hour:1
base="https://github.com/$REPO/releases/download/$TAG"
ver="${TAG#v}"
curl -fsSLO --retry 5 --retry-delay 5 --retry-all-errors "$base/mycelium-$ver-linux-x86_64.tar.gz"
curl -fsSLO --retry 5 --retry-delay 5 --retry-all-errors "$base/SHA256SUMS"
sha256sum --ignore-missing -c SHA256SUMS
tar -xzf "mycelium-$ver-linux-x86_64.tar.gz"
install -m 0755 "mycelium-$ver-linux-x86_64/mycelium" /usr/local/bin/mycelium
got=$(mycelium --version)
[ "$got" = "mycelium $ver" ] || { echo "::error::mycelium --version printed [$got]"; exit 1; }
```

## Step 2 · A scaffold in your own language: the Python template, and its SDK from the same tag

`mycelium init` writes a project of its own from a template the binary embeds, pinned to the binary's release
([feature page](../../features/init.md)). `governed-call-py` is the published node under a named-token table, an SDK
agent that declares a unit, and a second agent whose token lacks the scope a write needs. The SDK is installed from
the same tag, and the step checks that the version it got is the one the tag declares.

```bash one-hour:2
mycelium init "$HOME/governed-call-py" --template governed-call-py
test -s "$HOME/governed-call-py/main.py" && test -s "$HOME/governed-call-py/node.toml"
pip install -q "https://github.com/$REPO/archive/refs/tags/$TAG.tar.gz#subdirectory=mycelium-py"
want=$(curl -fsSL "https://raw.githubusercontent.com/$REPO/$TAG/mycelium-py/pyproject.toml" | sed -n 's/^version *= *"\([^"]*\)".*/\1/p' | head -1)
got=$(pip show mycelium-py | sed -n 's/^Version: //p')
[ -n "$want" ] && [ "$got" = "$want" ] || { echo "::error::mycelium-py $got installed, the tag declares $want"; exit 1; }
echo "mycelium-py $got"
```

## Step 3 · A declared unit, a permitted write, and a write the gateway refuses by name

The scaffold starts the node from `node.toml`, declares `units/purchaser.toml` through the gateway (its mandate and
rules come back as `not_enforced` — the published node attaches no evaluator, so they are a declaration here, not a
check), writes a key as `alice`, and is refused as `reader`: `403 insufficient_scope`, the gateway's own rule
([`gateway.auth`](../../reference/rule-catalogue.md)), naming the scope the route needs. The *explained from the
records* half of this step — the refusal read back from an evidence journal — waits on a node that attaches an
evaluator from configuration, which the plan records as open.

```bash one-hour:3
cd "$HOME/governed-call-py"
MYCELIUM_BIN=mycelium python3 main.py | tee run.log
grep -q "alice declared \`purchaser\`" run.log
grep -q 'not enforced by this node' run.log
grep -q 'alice set demo/greeting: PERMITTED' run.log
grep -q 'reader set demo/greeting: REFUSED 403 — insufficient scope' run.log
grep -q 'the route needs kv:write' run.log
```

## Step 4 · A grant that is default-deny and holds across a restart

The scaffold's unit declares a tool the purchaser may call only under a grant the treasurer issues
(`requires_grant = true`, [unit file](../../reference/unit-file.md#rule)). `mycelium test --faults grant` builds a
fleet from that declaration and your configuration and drives it ([grants](../../features/grants.md),
[fault classes](../../features/test-faults.md)): a plain transition is refused and silence refuses within the window;
a principal no rule names is refused the issue by the evaluator, the treasurer's grant admits the call once; the
agent's node is stopped before it reads an answer, the treasurer answers the standing request, and after the restart
the request and the grant are back from the node's own WAL — the agent re-waits and does not ask twice. **Where it
is enforced:** by the evaluator the `grant` class builds from your declaration's rules, inside the class's own
in-process fleet — the published node you started in step 3 attaches no evaluator, so it enforces none of this. The
class needs `[tls]` (the node signs the grant) and `[persistence]` (the grant survives the restart), so the step appends
both to the scaffold's configuration.

```bash one-hour:4
cd "$HOME/governed-call-py"
{ cat node.toml; printf '\n[persistence]\nbase_path = "data/grant-persist"\n\n[tls]\nauto_cert_dir = "data/grant-tls"\n'; } > grant.toml
mycelium test --faults grant units --config grant.toml | tee grant.log
grep -q '^grant: passed' grant.log
grep -q '✓ a grant-required tool is default-deny' grant.log
grep -q '✓ issuing is the declaration'"'"'s rule' grant.log
grep -q '✓ the grant holds across a restart' grant.log
```

## Step 5 · A lost acknowledgement recovered without a second effect

The `recovery` class ([selective recovery](../../features/selective-recovery.md)): three nodes of your declaration's
fleet send effects to one mock destination that loses an answer; the node that lost it is restarted over its WAL,
finds its record *unknown*, probes before any retry, and the destination holds one effect; dependent work elsewhere is
recovered by its declared class, independent work untouched, and `mycelium explain --closure` renders why from the
records. Over the same configuration with `[persistence]`.

```bash one-hour:5
cd "$HOME/governed-call-py"
{ cat node.toml; printf '\n[persistence]\nbase_path = "data/recovery-persist"\n'; } > recovery.toml
mycelium test --faults recovery units --config recovery.toml | tee recovery.log
grep -q '^recovery: passed' recovery.log
grep -q '✓ the restarted node finds its lost-acknowledgement record unknown in its own ledger, probes before any retry, and the destination holds one effect' recovery.log
grep -q '✓ the destination saw each effect once' recovery.log
grep -q 'mycelium explain --closure' recovery.log
```

## Step 6 · A readiness report naming what the fleet enforces

`mycelium doctor` ([feature page](../../features/doctor.md)) runs `start()`'s own refusal chain and profile check
over a node built and never started, and lists what the configuration does **not** enforce — so the token table,
which it does, must not appear as unmet.

```bash one-hour:6
cd "$HOME/governed-call-py"
mycelium doctor node.toml | tee doctor.log
grep -q 'start(): would start' doctor.log
grep -q 'guarantees: enforced' doctor.log
if grep -q 'gw.token_tables:' doctor.log; then echo '::error::gw.token_tables is listed as unmet'; exit 1; fi
```

## What you have, and what you do not

You have a node from a release you verified, a project in Python that declares a capability and is refused by the
gateway's scope rule, a fleet of your declaration under two of the faults the substrate claims to handle, and a
report of what your configuration enforces. You do not have: a policy evaluator attached from configuration (the
scaffold's rules are a declaration until one is), a real destination behind the recovery class (its destination is a
mock in the test's process), or a fleet of more than one node outside the fault classes — each stated on
[what-is-proven.md](../../operations/what-is-proven.md). The next step with a toolchain is the Rust scaffold,
`mycelium init my-fleet --template governed-call`, where an evaluator **is** attached in code and the refusal is read
back from the evidence journal; the five-step path on the [guide's front page](../README.md) continues from there.
