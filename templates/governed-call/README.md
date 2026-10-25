# governed-call

Two nodes, one governed call. Node A serves a tool under a policy that is enforced **at the provider** — not at a
gateway in front of it — and records every decision in an evidence journal (node-local, append-only, fsynced). Node B, a member of the same
fleet, calls the tool directly: once inside the remit (permitted, the tool runs) and once outside it (refused, the
tool never runs). Then B's prompt skill answers from the text in `skills/demo/greeting.md`.

```sh
cargo run
```

What you will see: `ledger=petty-cash: PERMITTED`, `ledger=capital: REFUSED — "action_denied" (…the capital ledger
is outside the remit)`, the skill's echoed answer, and the evidence records — one `decided` record per call, each
carrying its execution outcome (`attempted` or `none`) — printed from `data/evidence.log`. A second run appends to
the journal and overwrites the trace.

## The layout

| Path | What it is |
|---|---|
| `node.toml` | node A's configuration; node B derives its own from it (`bind_port + 1`) |
| `units/purchaser.toml` | the unit file: the capability A advertises, the mandate and rule that say who may call it, and a tool that waits for the treasurer's grant |
| `skills/demo/greeting.md` | the prompt skill's text (`skills/<ns>/<name>.md`, the layout `mycelium declare` reads) — edit it, run again, and the answer changes |
| `src/policy.rs` | the policy, an `ActionEvaluator`: permit one ledger, deny another, refuse the rest as undecided |
| `src/main.rs` | the program |
| `data/` | written by a run: `tls/` (the fleet CA and both node certificates, reused on the next run), `evidence.log`, `decisions.jsonl` |

## Read what the fleet decided

```sh
mycelium wire-check units/            # the declaration: what would bind, and the authority overlay
mycelium explain data/decisions.jsonl # the node's own decisions (admission, membership), in the catalogue's words
mycelium doctor node.toml             # what start() would say about this configuration, and why
mycelium test --faults units/ --config node.toml   # this declaration's fleet under a revoked key, a restart, a node gone dark, a grant that holds across a restart
```

The authority decisions — permit, deny — are in `data/evidence.log`, a length-prefixed journal (not lines: read it
with `mycelium::read_evidence_journal`, as the run does when it prints them).
A busy port fails the run with the address in the error: change `bind_port` in `node.toml` (B uses the next one).

## What this does and does not show

It shows a call governed where the work happens: node B reaches node A as a member, with no gateway in the way,
and the policy still decides before the tool runs. It does not show a human approval (that is a grant, a later
row), nor a real model: the skill answers through the echo backend, so no model is downloaded. The development
posture is on: both nodes share one CA whose key is on both, which `mycelium doctor` reports as `id.ca_key_off_node`
unmet.
