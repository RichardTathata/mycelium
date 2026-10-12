## [2026-10-12] ingest | three external agent runtimes compared; plan rows K–P; an `a2a-sdk` probe

- Mycelium's agent boundary was compared with AWS's **Strands Box / Dogwood** (OS containment plus a policy language
  over an agent's history), **NVIDIA OpenShell** (a sandbox runtime with credential placeholders, fixed versus live
  policy, and a privacy router for inference) and **Strands Agents 1.0** (an agent SDK: `SessionManager`, swarms,
  graphs, A2A). Recorded as rows in `docs/plans/post-360-hardening.md`: **K** policy over history (the evaluator sees
  what an actor already did; per node until §5's fleet question is decided), **L** credentials attached at egress (an
  opaque placeholder resolved only at the bound endpoint), **M** activation commands confined (an OS profile; OpenShell
  a candidate backend), **N** A2A at the current specification, **O** a Strands session store on Mycelium, **P** a
  Strands fleet demonstration; and a §5 question on inference routing as policy.
- **The probe that made row N concrete** (vendored with its output at
  `docs/plans/external/2026-10-12-a2a-sdk-probe/`): `a2a-sdk` 1.2.2 cannot reach a Mycelium skill. The card is only at
  `/.well-known/agent.json` (the SDK reads `agent-card.json`); the card's `url` is the node's **gossip** address without
  `/a2a` (`src/agent/a2a.rs:445` — a defect for any A2A client); the methods are pre-0.2 `tasks/*` (`SendMessage` and
  `message/send` answer `-32601`); the skill is chosen by a Mycelium-only `skillId`. Also learned: the `mycelium`
  binary does not mount `/a2a` — it is the library opt-in `GossipAgent::with_a2a()`.
- The plan's new **§3.6** holds the state of play for the next iteration: open PRs (E #613, F #611 with F2), unstarted
  rows, and every follow-up recorded as not built in a delivered row.
