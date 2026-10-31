# First Customer Pilot — readiness & de-risking checklist

↑ [Operations](README.md) · pairs with [production-readiness.md](production-readiness.md)

> **A fleet that grows the capabilities it lacks, under rules it can show it kept.**
>
> Mycelium is an embedded library for agent fleets with no coordinator. Generic nodes install signed capabilities when demand goes unmet and re-heal what they declared when a provider dies. Where you configure an enforcement point, it checks authority before work runs and records what it decided; a recorded run replays. That is what a pilot is sold on ([`docs/positioning.md`](../positioning.md)); the
> order it is told in — what an agent *may* do · what it *did* · a fleet that fills itself · how ·
> what is proven — is the buyer deck's, and the last item is one dated page,
> [what-is-proven.md](what-is-proven.md), which a pilot is the way to move lines off.

A pilot is the validation the project cannot supply itself. Every strong signal so far is **self-assessed**: the
project's own audit method ([`docs/analysis/ratings.md`](../analysis/ratings.md), M2) caps any dimension at 9 without
**external validation**, and calls a 10 "unreachable from inside this loop." This page is how to run the first one so
that it de-risks the engagement rather than stress-testing the newest code in production. It starts from a fleet you
can run today, the two-domain scaffold, and ends with a report the counterparty signs.

> **The engagement itself** — roles, workbooks, the pinned pack, the acceptance script, handover
> and closeout — is [engagement-kit.md](engagement-kit.md). This page is the *why* and the shape;
> that one is the *what you carry on site*.

## Day one: walk the scaffold with the pilot kit

The kit is the directory [`pilot-kit/`](pilot-kit/README.md). Its README is a walk of six steps, and CI runs its
commands as written on every push, so none of them is untried. On the first day you and the counterparty run it
together, on one machine, before anything touches theirs:

| Step | What you run | What it tells the pilot |
|---|---|---|
| 0 · the hour | [tutorial 7](../guide/tutorials/07-the-first-hour.md), the `one-hour` job's path from the published node | the machine, the network and the release do what the job shows; stop here if they do not |
| 1–2 · the scaffold | `mycelium init $PILOT --template two-domains`, pointed at a checkout until the next MINOR | two fleets of two nodes, each its own domain under `secure-multi-domain`, with the kit's declaration beside them |
| 3 · the edge | `bash fleet.sh setup`, `start-all`, `connect`, `call` each way | certificates issued off-node, a pinned key each way, one call across the edge carrying the local caller's name |
| 4 · `doctor` | `bash fleet.sh doctor <node>` on all four | what `start()` would say, one guarantee per line, under the profile; each line names its [matrix row](shared-responsibility-matrix.md#from-a-doctor-line-to-its-row), or says it has none and why |
| 5 · the faults | `mycelium test --faults units --config faults.toml` | revocation, a restart over the WAL, a partition, a lost acknowledgement recovered and a grant across a restart, each against the declaration |
| 6 · the report | in `$PILOT`: `python3 "$KIT/report.py" --doctor doctor/*.txt --faults faults.json > pilot-report.md` (the kit's `env.sh` sets `KIT`) | one section per guarantee a class exercised, split by whether the class's fleet enforced it, in the guarantee report's words, with a line for the counterparty to fill |

What changes on the counterparty's own hosts is what the scaffold's README says one machine cannot show:
- each node gets only its own `certs/<node>/` directory;
- `ca/` stays on the issuer's host, and only `exchange/` crosses between the two operators;
- the policy in `src/authority.rs` becomes their adapter rather than the reference evaluator.

The kit's report is the shape of what comes back: the same sections, regenerated from their `doctor` and
`test --faults` output, with *Observed in the pilot* written by them.

The kit is not the deployment. The *named deployment* line on [what-is-proven.md](what-is-proven.md) moves only when a
counterparty runs it on their own hosts and signs the report.

## The framing (say this out loud with the customer)

- The **substrate** (Layers I–III, security, SDKs) is mature and heavily tested.
- Some **companions are newer than the substrate** — the `mycelium-wiki` crate and its access broker
  shipped in v2.4.0 (2026-08-16) and have been audited once, by the team that built them. A pilot is
  their first real-world exercise.
- Therefore: **bounded scope + engineering in the loop + explicit success criteria.** Treat findings as
  the point, not a surprise.

## 1 · Scope it to proven ground

- ☐ **Pick a use case a shipped companion already models**, not a greenfield stress of the newest
  surface. The worked examples are the template:
  - work distribution / pipelines → `mycelium-tuple-space` ([`fluid_pipeline`](../../examples/fluid_pipeline)),
  - opportunistic content-routed claiming → `mycelium-blackboard` ([`microgrid`](../../mycelium-blackboard/examples/microgrid.rs)),
  - durable curated knowledge (org-twin / council) → `mycelium-wiki` ([`wiki_chat`](../../mycelium-wiki/examples/wiki_chat.rs)),
  - autonomic code mobility → `mycelium-wasm-host`, federation edge → `mycelium-agentfacts`.
- ☐ **Bound the scale** to a node count you have *rehearsed formation at on the real target network*
  (not Docker-bridge — see the known 100-node CI ceiling in [tuning.md](tuning.md)). Start well inside
  the envelope; grow after the first success.
- ☐ **One coordination primitive first.** Don't debut the wiki + consensus + cross-group + WASM in the
  same pilot. Add surfaces once the first is boringly stable.

## 2 · Production gates (inherited)

- ☐ **Complete [production-readiness.md](production-readiness.md) for the deployment** — TLS, gateway
  auth, persistence + restart rehearsal, sizing profile, metrics/alerts, `cargo audit`. The pilot does
  not get to skip these because it is "just a pilot." `doctor` answers most of them by guarantee id on each of the
  counterparty's nodes, and the [matrix's doctor-line table](shared-responsibility-matrix.md#from-a-doctor-line-to-its-row)
  says which side holds each one.
- ☐ **If the pilot uses the wiki:** provision the node-independent store (shared FS / S3 / doc store),
  set the curator's `Membership` access policy, and confirm `Wiki::shutdown` is called on teardown
  (the curator's background tasks are otherwise long-lived). → [companions/wiki.md](../wiki/dev/companions/wiki.md)

## 3 · Keep engineering in the loop

- ☐ **Named escalation path** — a substrate engineer reachable during the pilot window, not just docs.
- ☐ **Diagnosis rehearsed together** — the customer's operator has run `/gateway/diagnose` +
  `/gateway/explain` against a *deliberately perturbed* staging fleet, so the first real incident isn't
  the first time they read a fleet narrative. → [diagnostics.md](diagnostics.md)
- ☐ **Feedback loop** — a lightweight channel to file what the pilot surfaces; expect it to feed a
  `/mycelium-analysis` run and possibly a fix, same as Run 32 caught the wiki lifecycle leak.

## 4 · Define success before you start

- ☐ **Functional criteria** — the specific coordination outcome the use case needs (e.g. "N work items
  complete exactly once with no coordinator", "the council corpus answers M queries grounded in curated
  facts").
- ☐ **Operational criteria** — mesh forms and stays converged at the target scale; a killed node fails
  over within the expected window; the fleet stays diagnosable; no unbounded resource growth over the
  pilot duration (watch `gossip_store_entries`, task counts, memory).
- ☐ **Non-goals stated** — no SLA/uptime guarantee (Support/SLA is commercial-track, out of engineering
  scope); this is a validation engagement, and both sides agree on that.

## 5 · What to watch (the pilot's own tripwires)

- ☐ **Convergence & liveness** — `gossip_peers_connected`, anti-entropy activity, no reconnect storms.
- ☐ **Back-pressure** — `gossip_frames_dropped_total`, the `sys/load/` opacity signals; is anything
  shedding admission?
- ☐ **Growth** — `gossip_store_entries` and process memory over days, not minutes (the class of leak
  Run 32 found is invisible in a short test).
- ☐ **The emergent-fleet questions** — periodically run `/gateway/diagnose`; a healthy pilot should read
  as boring.

## Exit — turning the pilot into evidence

- ☐ **Write it up** — what held, what surfaced, what was fixed, in the kit's report regenerated from the
  counterparty's own `doctor` and `test --faults` output, each *Observed in the pilot* line theirs. A successful pilot
  is the first external data point the M2 audit series has ever had; record it in the calibration ledger's spirit
  (scores that predicted reality) and, named with consent, on [what-is-proven.md](what-is-proven.md).
- ☐ **Decide the next envelope** — more nodes, a second primitive, or a second use case — from evidence,
  not optimism.

---

**Bottom line:** the honest readiness statement is *"ready for a bounded first customer-led project,
with engineering engaged, treated as the external validation the internal loop cannot self-supply."*
Run it that way and the second engagement starts from evidence instead of self-assessment.
