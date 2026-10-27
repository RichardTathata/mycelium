# The adopter surface — a first run, one trace, a grant, and a side effect that can be retried

**Status:** agreed with the maintainer on 2026-10-23, rev 0.13 (rev 0.12 decided D28; rev 0.13 records Y7 adding `fed.domain_enforced`), written 2026-10-23. The first wave is delivered: X0, X1, X2 and X10 released in v2.37.0, X3–X9 and X11 in v2.38.0, each row's evidence in §8; the second wave (Y1–Y10) has started: Y1 (#683), Y4 (#684) and Y7 (#685) merged, each with one follow-up PR — Y4's by-hash read route (#692, which delivers the row) and Y7's two-mesh suite under the profile (#691, merged), the row closing with #695 (maintainer, 2026-10-27); Y8, the two-domain scaffold, in review (#695); Y3, the observing dashboard, in review (#693) (§8). Rev 0.11
answers PR #663's independent adversarial review: two premises corrected (X4's fields ride in the signed application
payload, never in the caller envelope; a replayed run re-executes an external call unless the destination sits behind an
`Input`-class seam, so X5 and X9 put theirs behind one), the front-door gate described as structural, the companion
limits sections as they are, and the smaller corrections, each marked *(rev 0.11)*. Rev 0.2 folds in a second discussion of the
same day on recovery as the management of an accumulating causal history (§1's last four rows, X4's parent hash, X5's
effect classes, X9 and D9–D10). Rev 0.3 answers a consistency check against the philosophy's litmus tests and the hot
invariants: five corrections, each marked *(rev 0.3)* where it lands — the closure is authoritative over the journal and
best-effort over the lossy trace; the parent rides inside the signed body, never a header; the replay class of
`test --faults` is a `sim` build; the grant's expiry reads `decision_now_ms`; the scaffold template is embedded from the
directory CI runs. Rev 0.4 answers a check against the developer-loop lessons of the compared projects, marked
*(rev 0.4)*: X1 prints the effective configuration; X2 defines the project layout and loads skill text from files; X3's
pending grant is durable across a restart; a new row X10 watches the declaration directory; trace export and a secure
template are deferred in §6 with reasons; behavioural evals and a deploy command are declined by name in §7. Rev 0.5
decides the five deferred questions (D14–D18) and turns §6 into a second wave of sketched rows, Y1–Y5, each shaped by
its decision and still deferred behind the first wave. Rev 0.6 adds the first-hour rows the comparison with Eve
exposed — X0 publishes the node as a binary and an image, X2 ships the Python template beside the Rust one — and
records the plan's one claim, *one hour to a governed fleet*, with the measurement that is allowed to make it (§1.1).
Rev 0.7 adds the closing row, X11: the documentation and examples pass over the whole surface the plan changed, run
through the instruments the repository already has, with a tutorial that is the `one-hour` job in prose. Rev 0.8 is a
last read of the whole plan: seven holes closed, each marked *(rev 0.8)* — the supervisor's door and the `grant/`
namespace, the argument rendering a human can read, the journal as the closure's precondition and the federation
edge as its leaf, a bound on the unknown loop, `test --faults` never running an activation, X9 in two forms, and the
Python template's CI target — with the no-model line, grant revocation and receipts, and the payload-compatibility
sentence added. Rev 0.9 adds Y6 to the second wave: an Eve agent directory run unchanged on a fleet, as a
demonstration with its non-mapping rows stated, never as a compatibility promise. Rev 0.10 turns the comparison
around: the weaknesses an enterprise multi-fleet reader would find in Mycelium become second-wave rows — a
multi-domain profile (Y7), a two-domain scaffold (Y8), a scale run that runs on a runner the workflow provisions (Y9),
and the pilot kit that is as far as code can go toward a named deployment (Y10). Sequenced **after** the feature-pages
plan's F2 ([`feature-pages.md`](feature-pages.md), PR #655, merged 2026-10-23), because every row here lands a feature
page and a registry entry under the fifth policy rule that F4 introduces.

## 1 · Where this plan comes from

On 2026-10-23 an outside comparison of Mycelium against Eve, the OpenAI Agents SDK, LangGraph, Mastra and Temporal
proposed seven usability enhancements and five further ideas. Checked against the repository at `a10c5890`
(v2.36.1 on `main`), about half of them recommend building what already ships. The check is recorded here so the
plan builds only the remainder.

| Proposal | What the repository has | What it lacks |
|---|---|---|
| `init` / `dev` / `doctor` | `mycelium wire-check`, `rules`, `explain`, `tls issue` and node start (`src/main.rs`); the denied-action demos (`examples/procurement_authority`, `guardrail_wedge`, `a2a_skill_authority`, `mcp_tool_authority`) run in CI | a scaffolder; a readiness check that runs without starting a node |
| a filesystem-first fleet manifest | the unit file, eleven sections ([`unit-file.md`](../reference/unit-file.md)), its JSON schema, `wire-check`'s findings by name, stems that consume it, SDK agents that load it (`POST /gateway/units/declare`) | a per-unit `runtime` for SDK agents, read by `wire-check` |
| an interactive approval | mandates bound to the action, expiring, re-verified at the provider ([guide 21](../guide/21-mandates.md), 2.15.0); the evaluator's `Indeterminate` fails closed; a live policy-revision check | **a positive grant.** The shipped `require_approval_for` (`src/agent/state_machine.rs:122`) is veto-based: it emits `AGENT_APPROVE`, waits up to 30 s for `AGENT_VETO`, and **silence is approval** |
| a unified run trace | the decision trace (`DecisionRecord`, `mycelium-core/src/decision.rs`), `mycelium explain`, the evidence journal (`AeEvidence`), receipts with a stable `operation_id`, replay bundles, the reason trace keyed by `run_id` (`mycelium-reason/src/trace.rs`) | one id carried through all of them; `DecisionRecord::parent_seq` is unused ([guarantees plan](guarantees-and-rule-catalogue.md) I5) |
| evals and fault injection | the replay scenarios, the bypass matrix (C7), `examples/authority_drain`, revocation at every verifier (row Q), the two-mesh federation suite with the link cut; the guarantee report refuses a generic badge | a one-command run against an adopter's own declaration — *(2026-10-24)* `mycelium test --faults` (X6) |
| an integration registry | companions register guarantees through the one registry (`GossipAgent::register_guarantee`); *(rev 0.11)* three companion READMEs carry a limits section under three names (`mycelium-commitment` *What it does not do (yet)*, `langgraph-checkpoint-mycelium` *Honest limits*, `strands-session-mycelium` *Claims*), the other seven have none | a table of what each runtime adapter carries: sessions, cancellation, tracing, identity, grants, provider-side enforcement; one limits section under one name in every companion README (X7) — *(2026-10-24)* `docs/reference/adapter-contract.md` and *What this does not establish* in every companion README (X7); the "ten" counted READMEs that do not exist — seven companions have one, the rest are documented on `companions.md` |
| a readiness scorecard and dashboard | `guarantee_report()`, `GET /gateway/guarantees`, the start-time log block, `ConfinementReport`, the profiles; three visualisation examples | a dashboard (deferred, §6) |
| Temporal's side-effect contract | `operation_id`, `AlreadyCurrent`, `Conflict`, `DeliveryUnknown` ([contracts-receipts](../design/contracts-receipts.md)); `mycelium-effects` as rung 4 (`EffectDestination`, `apply_composed`) | the same contract for an **external** mutation: a dedup scope, a reconciliation probe for the unknown outcome, retry only where declared safe |
| LangGraph's task lifecycle | A2A task states (`src/agent/a2a.rs`), the commitment companion, leased leadership, locks, the tuple space | a task API; **deferred** (§6): it belongs in a companion on tuple-space claims, never in the substrate |
| Mastra's memory | the knowledge layer (`src/knowledge/`), `mycelium-wiki` | a provenance-aware context selector; deferred (§6) |
| schedules | every periodic loop ticks through the timer seam; gateway timers with bounded periods (2.33.0) | declared recurring work with ownership, a lease and missed-run handling; deferred (§6) |

The second discussion asked how far the repository is from recovery that reconstructs only the causally affected part of a
fleet's history, rather than rolling the fleet back. Its premise is one the project already holds: `ROADMAP.md` D33
names three scopes of undo and rules the third, an effect another participant has acted on, not undoable; compensation
is a new promise. The evidence journal already corrects a record by citing its content hash
(`AeEvidence`, `src/agent/action_evaluator.rs:1110`), never editing in place.

| Capability | What the repository has | What it lacks |
|---|---|---|
| replay separated from recovery | the kernel calls no destination; the seams replay a filesystem write by design (`mycelium-core/src/sim_seam.rs`, *Why the effect is not suppressed in replay*) | *(rev 0.11)* an `Input`-class seam (`ChoiceKind::Input`) in front of an external destination — no production code uses one today, so a replayed run re-executes an LLM, MCP or HTTP call it made; X5 puts its destination and its probe behind one, and X9's recording is asserted in replay |
| honest uncertainty about an effect | `DeliveryUnknown`, `Indeterminate`, and an unrecognised `Execution` read as *we did not look* | nothing |
| causal links between records | `DecisionRecord::parent_seq` within a node; the install token across install, activation and probe; a correction's hash | a parent carried from one agent's decision into another agent's record (X4) |
| an effect's recovery contract declared before execution | the tuple-space discipline (single-owner claim, idempotent ack, requeue after a deadline); the composed effect's legs; *(2026-10-24)* the destination's class, declared before execution and recorded on the prepared record (`mycelium-effects::external`, X5) | nothing code-shaped; a real provider (`what-is-proven.md`) |
| selective recovery | the fluid pipeline's one-stage requeue | the dependency closure of one record, read across nodes; recovery of that closure alone (X9) |
| a causal exposure policy | history-limited rules count what an actor has done (rows K, K2); the stability record holds speculation under uncertainty | a rule whose input is the unresolved effects in an actor's cone; deferred until X9 gives it an input (§6) |

### 1.1 · The claim, and what is allowed to make it *(rev 0.6)*

The compared frameworks' sentence is *minutes to an agent*. This plan does not compete for it: a library with no
hosted runtime will not win the first ten minutes, and the rows that follow are not designed to. The plan's one claim is
**one hour to a governed fleet**: starting from a machine with no Rust toolchain, within one hour a developer has

1. a fleet of nodes running from a published binary or image (X0),
2. an agent in their own language and SDK declaring a capability from a scaffold (X2),
3. a call refused by policy and explained from the records (X2, X4),
4. a grant that is default-deny and holds across a restart (X3),
5. a lost acknowledgement recovered without a second effect (X9),
6. and a readiness report naming what the fleet enforces and what it does not (X1).

**The claim is measured, or it is not made.** A CI job (`one-hour`) runs the whole path from a clean container with no
toolchain, from `curl` of the release artefact to the last `doctor`, and records wall-clock time per step in the job
summary. *(rev 0.8)* Neither template needs a model: the governed call runs against `EchoBackend` (`src/agent/llm.rs`, the CI job's
*mock LLM*), so the hour is not spent downloading one; a demo that wants a real model says so and is outside the clock. Steps 4 and 5 are run as
the shipped examples on the published binary (X9's live form), not built by the developer. The job grows as rows land:
with X0–X2 it measures steps 1–3 and 6; with X3 and X9 it measures all six. *(2026-10-23, #670)* The job exists:
`.github/workflows/one-hour.yml`, on the successful completion of a Release workflow run for a tag push (a Release
created with the workflow's own token fires no `release` event) and by dispatch against a tag, in
`python:3.12-slim-bookworm`, against the published artefacts only; it prints the per-step seconds and the total, from
the first `curl` to the last `doctor`, to the job summary, prints no number on a red run, and fails past 3 600 s.
**What it measures today, exactly:** step 1 as one node from the published binary (not a fleet), step 2 as the Python
scaffold and its SDK from the tag, step 3 as a declared unit, a permitted write and a write the gateway refuses by its
scope rule (the "explained from the records" half waits on a node that can attach an evaluator from configuration — the X3
question; X4's reader shipped in v2.38.0), step 6 as `doctor`. **Measurements, by release:** `v2.37.0` (run 40011730290,
2026-10-23, dispatched by hand after #670 merged; Debian 12 bookworm, Python 3.12, no toolchain): step 1 **1 s**, step
2 **6 s**, step 3 **1 s**, step 6 **0 s**, total **9 s** from the first `curl` to the last `doctor`, the container's
own setup 3 s outside the clock — printed as **10 minutes** by the rule above. The sentence still does not enter
`positioning.md`: steps 4 and 5 are not yet measured on a release. *(2026-10-25, X9)* Step 5 is in the job — `mycelium test --faults
recovery` over the scaffold's declaration with `[persistence]`, on the published binary — and is measured from v2.38.0, the
first release that carries the class; until that tag runs, the row prints as the measurement of the release it ran on.
*(2026-10-25, X3's fourth PR)* Step 4 is in the job too — `mycelium test --faults grant` over the scaffold's declaration
with `[tls]` and `[persistence]`, on the published binary: the scaffold's grant-required tool refused on silence, issued
by the declared issuer, holding across a restart — so the job measures all six steps from the first release carrying
both classes; a tag before them fails the step and prints no number. The number the documentation prints is the
job's **measured** time on the most recent release, rounded up to the next ten minutes, never the aspiration — if the
measurement comes in above an hour, the page says what it measured. *Two hours* was the earlier estimate for a
developer building the path by hand with their own agent; one hour is the target for running it from the scaffold.
The sentence enters `docs/positioning.md` (gated by `check-positioning.sh`) only once the job has measured it on a
tagged release, and `/publication-lint` reads it like any other claim on the persuasion surface. *(2026-10-26)* **All
six steps measured:** `v2.38.0` (run 40269648703, dispatched by hand — the tag went in one push with five companion
tags, which fires no workflow; `RELEASING.md` § 7 now says to push the `v*` tag alone): steps 1–6 **2 · 5 · 1 · 3 · 1 ·
0 s**, total **12 s**; `v2.38.1` (run 40368580893, started by the Release): **2 · 6 · 1 · 4 · 0 · 0 s**, total **13 s**;
both printed as **10 minutes**. The sentence may now enter `docs/positioning.md`; that is the maintainer's decision, not
yet taken.

## 2 · What binds every row

1. **A library, not a platform.** No row adds a daemon, a control plane, or anything a node needs in order to run.
   `doctor`, `init` and `test` are commands over files; the only one that starts nodes starts them in-process or in
   the existing Docker suites.
2. **A declaration never claims a guarantee.** A scaffolded project, a manifest key or an adapter row says what is
   *configured*; whether it is *enforced* is answered only by the guarantee report, as today.
3. **The substrate is unchanged.** Wire v12 stays. New behaviour lives in `src/` above Layer II or in a companion
   crate. A struct that gains a field gets a `deprecations.md` entry.
4. **The trace reads no clock and makes no draw.** A correlation id is *carried* from the envelope that
   started the work, never generated inside a decision. `the_trace_makes_no_extra_draw_under_the_replay_seams` and
   `the_trace_changes_no_decision_and_says_what_the_round_decided` stay green on every row that touches the trace.
5. **The verification policy, all five rules.** Each row enumerates its doors before fixing (rule 1), closes on
   quoted evidence (rule 2), is collected by discovery (rule 3), is reviewed adversarially (rule 4), and lands its
   feature page and registry entry (rule 5 — written by F4; *(rev 0.11)* a row that lands before F4 follows the
   feature-pages template under F2's gate, which exists, and rule 7 below holds the reviewer to the same check
   regardless). Every generated file that a row documents is run in CI **by a job that executes it**; the front-door
   gate (`scripts/check-front-doors.py`) checks structure and link targets, never prose, and is not that job.
6. **Fail-first.** Every regression test is seen failing on the unfixed code and the commit says so.
7. **Every row lands its documentation with its code, and the plan ends with a pass over the whole surface.** A row
   is not closed until its feature page (rule 5), its glossary entries, its example-index row and its guide or runbook
   section exist and resolve; the independent reviewer checks them as code. X11 then audits the surface as one thing,
   because rows written apart drift apart: a term defined twice, a door that routes to the old page, an example that
   runs but no longer demonstrates.
8. **A record links by content hash, and no node holds the graph.** A parent reference is the hash of the record it
   depends on, carried in the envelope by the caller, the way a correction cites what it corrects. Journals stay
   node-local and un-gossiped. The graph is assembled by a reader from the journals it is given, so the dependency
   closure is an offline reading, never a runtime path and never a service.

## 3 · The rows

Each row is one PR, or a short stack. The exit is what an independent reviewer checks. *Doors* is rule 1's
enumeration, written now so the review has it.

### X0 · Publish the node *(rev 0.6)*

**What:** a release workflow, triggered by the tag the runbook already pushes (`RELEASING.md` §7–§8), that builds the
`mycelium` and `mycelium-stem` binaries for Linux x86_64 and aarch64 and macOS aarch64, attaches them with SHA-256
sums to the GitHub Release the runbook's §8 creates, and pushes a container image to `ghcr.io`, tagged by version and
`latest`. *(X0's PR)* The existing `docker/Dockerfile` builds the three-node demo example, not the node, so the image
has its own `docker/Dockerfile.node`: a bookworm builder (so the Linux binaries link glibc 2.36 and `libssl3` and run
on anything at least that new), an `export` stage the tarballs come from, and a non-root runtime stage — the tarball
and the image carry the same bytes. Today nothing publishes either: the workflows build and test, and
the Python SDK's README sends the reader to `cargo build --bin mycelium`. A Python or TypeScript developer needs a
Rust toolchain before the first line of their own code, which is the largest first-hour cost the comparison found.

**Why first:** every later row's scaffold, demo and timing starts from this artefact.

**Doors:** `RELEASING.md` §8 (the release body stays the annotated tag; the artefacts are attached to it, so the step
that silently stopped after v2.4.4 cannot stop again without the workflow failing); the `cargo audit` gate (*(X0's
PR)* the Rust side of the image is audited as the lockfile is, through `--locked`; the Debian runtime layer —
`libssl3`, `ca-certificates` — is **not** audited by anything here, stated); the version anchors (*(X0's PR)* the
artefacts need no new anchor: the download block reads `MYCELIUM_RELEASE_TAG`, already in `RELEASING.md` §6's list,
and `check-positioning.sh` learns nothing — the earlier promise is withdrawn, not built); `installation.md`,
`README.md` §§ Build and Run, both SDK READMEs, the wasm host's README and `capability-lifecycle.md` (the published
artefact becomes the path, the toolchain the fallback).

**Exit:**
- a dry run — on the pull request itself, since GitHub resolves `workflow_dispatch` only on the default branch, and
  by dispatch once merged — produces the three tarballs and `SHA256SUMS` as a workflow artifact, and the smoke passes
  on every platform (*(X0's PR)* the earlier wording asked the sums to match a local build, which
  is reproducibility, which the same sentence disclaimed; the sums are integrity — they match the files they ship with);
- a clean container with no Rust toolchain downloads the binary, runs `mycelium --help` and `mycelium doctor` on the
  scaffold's config;
- the `one-hour` job's first step is this download, timed.

**Not built, stated:** no signing of the binaries (Sigstore or GPG) — the sums are integrity, not provenance, and the
page says so; no Windows build; no package-manager formula.

### X1 · `mycelium doctor` — the readiness report without a node

**What:** `mycelium doctor <config.toml> [--units <dir>]` loads the configuration the way `start()` does, resolves the
guarantee report and the confinement report against it, runs `wire-check` over the units, and prints what `start()`
would print and whether it would refuse, naming each unmet guarantee of the profile. It never binds a port or opens
a persistence directory.

**Why early:** it is the smallest code row, it is what X2's scaffold runs last, and it answers the
"production-readiness scorecard" proposal with the report that already exists.

**Doors:** `parse_args` / `GossipConfig::from_toml` and every env override; `guarantee_report()`
(`src/agent/mod.rs:1903`); `confinement_report()` (`src/agent/confinement.rs:109`); the profile refusal in `start()`;
`wire_check_cli`. The report must be the same function the node calls, not a copy.

**Exit:**
- for a configuration that `start()` refuses under `secure-single-domain`, `doctor` names the same unmet guarantees in
  the same words, shown by one test that runs both;
- for a token table, `[oidc]`, `[tls]` or `[gateway_tls]` in a build without the feature, `doctor` reports
  `not_in_build` naming the feature (the v2.18.1 refusals), without starting;
- a persistence directory that does not exist is reported, not created (R7's refusal stays with `start()`);
- a planted fault in each of the four: shown failing.

*(rev 0.4)* **It prints the effective configuration**: every field after defaults, the file and the env overrides,
each with its source named (`default` · `file` · `env`), *(X1's PR)* in the configuration's own key order — a profile
requires fields, it sets none, so it is not a source; and the binary cannot read `configuration.md` at run time, so
the order is the serialised form's (alphabetical) and a test holds every top-level field to a row in
[`configuration.md`](../reference/configuration.md)'s table instead. This is the "what will actually run" view: the unit files are
already shown by `wire-check`'s overlay; the configuration had no such view. Secrets are printed as `set` or `unset`,
never as values. Exit: a test sets one field three ways and reads the source column; a token is shown as `set`.

**Not built, stated:** `doctor` cannot say `enforced` for `not_verifiable_here` guarantees; it reports them as the
node would. It cannot report unreadable persisted state without opening the directory, which it does not do; that
refusal stays with `start()` (R7, 2.20.0).

### X2 · `mycelium init` — a scaffold that CI builds and runs

**What:** `mycelium init <dir> --template governed-call` writes a project: a `Cargo.toml` depending on `mycelium`
by version, a `node.toml` under `profile = "dev"` with the comment lines that name each setting the secure profile
would require, one unit file with a `[[capability]]`, a `[[mandate]]` and a `[[rule]]`, a `main.rs` that starts two
in-process nodes with a `ReferenceEvaluator`, makes one permitted call and one denied call, and prints where the
decision trace and the evidence journal landed. Its README's last line is `mycelium explain` over that trace and
`mycelium doctor` over that config.

The template's source of truth is not a string in the binary. It is a directory under `templates/` that CI builds and
runs as a project of its own. *(rev 0.3)* An installed binary has no `templates/` beside it, so the binary embeds that
directory at compile time (`include_dir`, or `include_str!` per file), and `init` writes the embedded copy. The
directory and the binary cannot differ because one is built from the other; a template that drifts from the API fails
CI before it fails a user.

*(rev 0.4)* **The template defines the project layout**, and `init` is where the layout is documented:

```
node.toml          the node's configuration
units/*.toml       the unit files wire-check reads and the node declares
skills/*.md        prompt-skill text, one file per skill
src/main.rs        the program
```

A prompt skill today is registered through a unit's `[[serve]]` section and its text lives under `prompts/` in the
KV, written by the program. The template's `main.rs` reads `skills/*.md` at start and writes each under its
`prompts/` key, so the skill text is a file an author edits. Whether the unit file itself gains a
`[[skill]] text = "skills/<name>.md"` reference, so that the stem can do the same without a program, is a design
question for X10, not for the template.

**Doors:** `init`'s target directory (refuse a non-empty one by name); the template's `Cargo.toml` version against
the workspace version (a check in `scripts/`, so a release bump cannot leave the template behind); the copied
README's commands (run by X2's CI job; the front-door gate checks only that its links resolve); the generated unit file through `wire-check` (zero findings).

*(rev 0.6)* **Two templates ship together: `governed-call` in Rust and `governed-call-py`.** The Python one starts
the published node (X0) rather than embedding it, declares its unit through `declare_from`, makes the same permitted
and denied calls through `mycelium-py`, and reads the same trace. It needs only the one adapter row it uses, so it does
not wait on X7's table; X7 fills the row in when it lands. The comparison's audience is the SDK audience, and a plan
that sequenced their template last would have answered the wrong developer first. The TypeScript template follows
X7. *(rev 0.8)* The Rust template attaches an evidence journal and an evaluator, because without them a node enforces
and records nothing (2.6.0's warning) and X4's closure has nothing to read; both run against `EchoBackend`, so no model
is downloaded on the path the hour measures. *(X2's PR)* **The Python template cannot attach either:** the published
node binary attaches nothing before `start()` and there is no configuration that attaches an evaluator, so its unit
file's mandate and rule come back `not_enforced` (the node says so) and its refusal is the gateway's own scope rule
(`gateway.auth`), counted, not recorded. The per-call policy from Python waits on a node that can attach a policy from
configuration — recorded as a question for X3, whose grant would need the same door. Two more things the row found:
the reference evaluator declares no security-relevant arguments, so a `requires_values` rule cannot see the value it
tests through the seam (the Rust template's policy is a twenty-line `ActionEvaluator` of its own, which is also the
clearer teaching); and a prompt-skill call under provider enforcement is refused for naming no resource, so the
template's skill lives on the member node and the provider calls it.

**Exit:**
- CI job: `mycelium init`, `cargo run` in the result, assert the permitted call's receipt and the denied call's
  reason, then `mycelium explain` on its trace and `mycelium doctor` on its config, each with the expected words;
- `init` into a non-empty directory refuses by name; a version mismatch fails the script;
- the template's `main.rs` is under 80 lines, and the Python template's `main.py` under 60, each counted by the test;
- *(rev 0.8)* the Python template runs in CI against the node it is built with: on a pull request, the local build;
  on a release branch and on `main` after a tag, the published image from X0 — a template that needs an API landing in
  the same release cannot pass against the previous tag, and the job says which target it ran *(X2's PR: the job
  prints its target and runs the local build on every event; the published-image half is **not built** — it waits on
  X0's first tag, and is a one-step addition to the job then)*;
- editing a file under `skills/` and restarting changes the skill's answer, shown by the CI job.

**Not built, stated:** the TypeScript template (after X7). No `mycelium dev` watcher: the project is `cargo run`, and
X10 watches declarations.

### X3 · A positive grant — approval that is default-deny

**What:** a second approval mode beside the veto one. For a tool in `require_grant_for`, the `Invoking` transition
waits for a **grant** and is refused on silence. A grant is a scoped mandate ([guide 21](../guide/21-mandates.md))
issued by a supervisor principal, bound to the tool, a digest of the arguments, the requesting agent's identity and
the deployed policy revision, with an expiry. It is verified where the action is enforced, so a client's button is
never the boundary: the agent's state machine checks it before `Invoking`, the gateway's preflight checks it, and a
provider with `with_provider_enforcement()` checks it again.

The veto mode stays as it is, documented as *veto, not grant*, and `strength_report()` labels each. *(rev 0.11)*
`PolicyViolation::ApprovalTimedOut` (`src/agent/state_machine.rs:144`) has no producer — `await_approval` returns `Ok`
on silence — so the enum reads as if a timeout refused; X3 retires it with a `deprecations.md` entry, or documents it as
unreachable, and the grant's own refusal is `GrantNotPresented`.

*(rev 0.3)* **The expiry check reads `Hlc::decision_now_ms()`**, never `hlc.current()`. Three expiry checks once read the
HLC and never consulted the wall clock, so on a quiet or partitioned node a mandate never expired (2.15.0); the grant is
a fourth member of that family, and `scripts/check-hlc-current.sh` must see its read classified. A test holds the HLC
still and shows the grant expire on the wall clock. The wait itself adds no lock: the state machine's rows 9–11 in the
lock-order table stand, and the review confirms no new `Mutex` or `RwLock` field.

**Why a mandate and not a new signal:** mandates already travel to the provider, expire, and are refused by name
when revoked; a grant that is a mandate inherits every door the bypass matrix enumerated. A new signal kind would
need the matrix re-run.

*(rev 0.8)* **The supervisor's door.** A grant needs someone to see the request and the authority to answer it; the
plan had neither. Three pieces:
- **A namespace a route owns.** Pending requests live under `grant/{agent}/{request}` in the KV (gossiped, so a
  supervisor on another node sees them; `agent/{node}/state` is the precedent). Under the 2.27.0 rule the raw KV routes
  refuse the prefix 403 `protected_key` naming its route, and `src/lib.rs`'s ownership table gains the row.
- **The routes and verbs.** `GET /gateway/grants/pending` and an SSE stream of new requests under a new scope
  `grant:read`; `POST /gateway/grants/{request}` issues or refuses under `grant:issue`; both SDKs gain the verbs; a gauge
  `grants_pending` and a counter of issued, refused, expired and revoked grants.
- **A rule about rules.** Issuing is an action the evaluator sees: *may this principal grant this tool for this agent*
  is a policy question, decided by the same evaluator with the issuer as the actor, so a client holding `grant:issue`
  is not thereby an approver of everything. A grant issued without that permit is refused and recorded.

**A human reads arguments, not a digest.** The request record carries a rendering of the arguments under the
data-class policy (row R's classes decide what may be written), beside the digest; the grant binds to the digest, and a
rendering that the policy withholds is shown as withheld, never as empty. The rendering is bounded by the KV write
cap, and an argument set over it is summarised with its digest and the size.

**Revoking a grant, and the grant's receipt.** A grant is revoked the way a mandate is: by its issuer or by a principal
the rule names, through the mandate route, refused by name afterwards at every door; an issued grant returns a
`KvReceipt` naming its rung like any write, so a supervisor knows whether the grant has reached the agent's node or
only their own.

**Doors:** `AgentPolicy` and `transition()` (`src/agent/state_machine.rs`); `ae_preflight` and
`ActionEvaluator`'s mandate check; `with_provider_enforcement`; `/mcp` and `/a2a` (both paths); `POST /gateway/rpc/call`;
the Python and TypeScript SDKs' mandate verbs; the evidence journal (a grant's hash on the decision record, as
`AeReference` does today); the guardrails `Policy` and its tier table.

**Exit, each seen failing first:**
- silence within the window refuses `GrantNotPresented`, never proceeds;
- a grant for a different argument digest, a different agent, an older policy revision, or past its expiry is
  refused at the state machine **and** at the provider when the state-machine check is skipped;
- a revoked grant stops a call already admitted but not yet executed (the `authority_drain` bound applies);
- the bypass matrix's plant: each door shows the grant check ran, by its own counter;
- the evidence record for the execution carries the grant's hash, and `prove_denials` reconstructs a refused grant.

*(rev 0.4)* **A pending grant request is durable.** The state machine keeps a mesh-visible copy of its state under
`agent/{node}/state` and has a checkpointed state (`src/agent/state_machine.rs:46`). A request for a grant is written
as a durable record with its argument digest, policy revision and expiry before the agent waits. An agent restarted
while waiting re-reads the record and waits on the same request; it does not ask twice, and a grant issued against the
first request is honoured after the restart. An expired request is not re-waited. Exit: a test crashes the agent while
it waits, restarts it, issues the grant, and sees one execution; the same with the request expired sees a refusal.

**Design question, decided in the PR:** whether an A2A task waiting for a grant reports `input-required` (the
specification's state; `Step` in `src/agent/a2a.rs:1005` has no such arm). Proposed: yes, with the pending grant's
request id in the status message and nothing else, so an A2A client can be the supervisor's surface.

**Not built, stated:** an operator UI. The grant is issuable from the SDKs and the gateway; what shows it to a human is
the adopter's.

### X4 · One id, and one parent, through the records

**What:** two fields carried from the envelope that starts a piece of work into every record that work produces, and
minted by none of them:

- `correlation: Option<String>`, which groups the records of one run. The reason trace's `run_id` is the same value.
  The gateway accepts it on `/mcp`, `/a2a`, `/gateway/rpc/call` and the reason routes and refuses one that is not one key
  segment (the `run_id` rule of 2.33.0). *(rev 0.3)* **Both fields travel inside the application body, never as a
  header.** The gateway caller's signature covers a digest of the application payload
  (`signed_fields`, `src/agent/gateway_caller.rs`); a header sits outside that digest, so an intermediary could re-point
  causal attribution under a signature that still verifies — v2.12.0's lesson, *authenticating the caller is not
  authenticating the call*. *(rev 0.11)* **The placement is the application payload at every door, and only there**:
  for an RPC, two reserved keys in the request's JSON params — the same place `presented_mandate` reads a presented
  mandate from (`src/agent/gateway_caller.rs:638`); for `/mcp` and `/a2a`, the request body. **Never the caller
  envelope.** Its optional fields `m` and `r` sit *outside* `signed_fields` (`:671-676` against `:930-947`), so a field
  added beside them would be exactly the unsigned header this rule forbids; the envelope is not extended and its v2
  signature is unchanged. No frame changes; wire v12.
- `parent: Option<Sha256>`, the content hash of the record this work depends on, the same hash `AeReference::journal_sha256`
  cites and a correction uses. A caller that acts on a decision passes that decision's hash to whatever it starts. A
  record with no parent is a root, and the reader says so.

The decision trace (`DecisionRecord`) gains both, with `parent_seq` kept for the chain within one node; the evidence
record (`AeEvidence`), the receipt (beside `operation_id`, not replacing it), the reason trace and the replay bundle's
`decisions.jsonl` gain both.

Then `mycelium explain --correlation <id> <journals…>` renders one chain across the files, and
`mycelium explain --closure <hash> <journals…>` renders the dependency closure of one record: every record whose parent
chain reaches it, across the journals given. Each line is marked **captured**, **reconstructed** or **unattributable** in
the trace's own vocabulary, and a closure computed from fewer journals than the fleet has nodes says so in its first line.

*(rev 0.3)* **The closure is authoritative over the evidence journal and best-effort over the decision trace.** The
journal is fsynced and never drops a record. The decision sink is lossy by design: it takes its lock with `try_lock` and
drops the newest record when contended or at either bound, counted and never evicted
(`mycelium-core/src/decision.rs`). A parent hash that points at a dropped decision record is a hole. The reader prints
the sink's drop counts beside any hole it finds, says which of the two sources each edge came from, and never
promises completeness over the trace. The KV receipt has no content hash of its own; it is joined by `operation_id`
and reported as such.

**Doors, the whole cost of the row:** a closure is only as complete as the doors that write to it, so every door that
starts dependent work must carry the parent, or the closure lies about independence. The enumeration, with its grep, goes
in the PR: signal emit and the wired emit; `rpc_call_framed` and `bulk_call`; the tuple-space `take` and the blackboard;
`/mcp`, `/a2a` (send and stream), `POST /gateway/rpc/call`, scatter, mailbox, shard and overlay emits; both SDKs; the
stem's activation command (the install token becomes a parent hash where the trace has one); every constructor of
`DecisionRecord` and `AeEvidence`. A **door registry**, in the shape of the fuzz reachability registry, lists each door
and the test that shows it carries the parent, and a door absent from the registry fails the gate. The bundle codec and
its fuzz target gain the two fields; the trace comparison ignores them (two recordings of one run must not diverge on an
id alone).

**Exit:**
- X2's scaffold explained end to end from one id: discovered, called, evaluated, executed, with the receipt's rung;
- a closure over a three-node run, read from the evidence journals, names every dependent record and no independent
  one, shown by a plant on each side; the same run with a saturated decision sink shows the hole named and counted,
  with the journal-side closure unchanged;
- *(rev 0.11)* a parent carried as a header, or placed in the caller envelope beside `m`, is never read: the record's
  parent stays empty and a counter names the door, shown by a plant on each; a parent inside the body is covered by the
  payload digest, shown by a plant that alters that key after signing and is refused as the digest already refuses it;
- the door registry complete, and a planted door that drops the parent caught by its test;
- the two trace-equivalence tests unchanged and green; the replay corpus replays byte-identically with the fields absent;
- a fuzz target for the join's reader, registered and asserted reachable;
- a record that cannot be attributed is printed as such, shown by a planted record with no id and by a closure read from
  one journal short.

*(rev 0.8)* **Three limits the row states up front.**
- **A closure exists only where a journal does.** A record's hash is the evidence journal's; an agent with no evaluator
  and no journal attached produces no record and no parent, so its work is a root with no cause and a leaf with no
  consequence. A fleet without journals has no closures. The templates attach one; the feature page says so first.
- **A closure stops at a federation edge.** A partner domain's journals are private and trust does not compose, so the
  edge's own record is the leaf; the reader prints the edge as a boundary, never as the end of the chain.
- **Retention and erasure make holes, not errors.** The journal's retention is the exporter's cursor (`operations/audit.md` §10), so a record the operator pruned, and a record crypto-shredded under
  `SubjectKeyRegistry`, are each a named hole with the reason.

**Payload compatibility, so a reviewer need not ask** *(rev 0.11)*: the two keys live in the request's JSON params,
which every handler reads as `serde_json::Value` and from which it takes the keys it knows, so an older handler ignores
them and a newer one reads them when present; the compatibility is the handlers' and the SDKs', not the envelope's. The
envelope and its signature are untouched. No frame changes, no rolling-upgrade window, wire v12.

**Not built, stated:** no clock is added; `at_ms` stays absent on the decision trace. Ordering across nodes is by HLC
where a record has one and by file order where it has none, and the output says which. The closure is as complete as
the journals handed to the reader; a node whose journal is lost leaves a hole the reader names, never fills. Over the
decision trace, completeness is not claimed at all (above).

### X5 · The external side-effect contract in `mycelium-effects`

**What:** a destination for a mutation outside the fleet, with the receipt vocabulary extended one rung. An
`ExternalEffect` carries `operation_id`, a **dedup scope** (the destination's own idempotency domain), the prepared
record (rung-1 checkpoint before), and the destination's **declared class**, stated before execution and recorded on
the prepared record:

| Class | Declared | On `DeliveryUnknown` | On a wrong outcome |
|---|---|---|---|
| **idempotent** under the key | retry is safe within the dedup scope | probe, then at most one retry | a compensation, if one is also declared |
| **compensatable** | a named compensating effect | probe; never retry | the compensating effect, a **new** record whose parent is the hash of the effect it compensates |
| **irreversible, unobservable** | neither | the uncertainty is kept; the record stays unknown | preserve the evidence; escalate (`Execution` stays *we did not look*) |

Applying it returns the existing rungs plus `DeliveryUnknown`. On unknown, the contract requires a **reconciliation
probe** (`observe(key) -> Done | NotDone | Unknown`) before any retry; a retry without a probe, against a destination
that did not declare itself idempotent, or a second retry, is refused by name and counted (`RefusalCounts` gains the
reasons). A compensation is never a reversal: it is an effect of its own, authorised like any other (the evaluator sees
it), with the compensated record's hash as its parent (X4), so the closure reader shows both. A settled outcome is
written as an execution record in the evidence journal with the receipt's rung, the way the composed effect is.

*(rev 0.8)* **The unknown loop terminates.** A probe can answer `Unknown` for ever; the contract bounds it. The
destination declares a probe budget (a count and an age); past either, the record is marked **unresolved and
escalated**, counted, and no further probe or retry is attempted without a new authorisation. That age is the input
Y5 later reads. **A probe is egress.** It goes through the same egress allow-list and credential binding as the effect
it probes (rows L and M), and its answer is a captured record with the effect's hash as its parent, so the closure
shows what was asked and what was answered.

*(rev 0.11)* **Behind an `Input`-class seam.** The apply path and the probe go through `ChoiceKind::Input`
(`mycelium-core/src/sim_seam.rs`), so a recording of a run that reached the destination replays without reaching it
again; no production code uses that seam today, and this is its first use, added to the seam baseline in the open.

The reference destinations: the existing sqlite one, extended, and a mock HTTP destination whose test harness injects
lost responses so the unknown path is the one exercised.

**Doors:** `EffectDestination` and `apply_composed` (`mycelium-effects/src/lib.rs`); the tuple-space consumer; the
`Counting<D>` wrapper; `CompositionLeg` (a new leg, *reconciled*, or the existing legs re-used, decided in the PR);
the evidence record's `Execution` enum (a new variant is `#[non_exhaustive]`-safe, 2.11.0's note).

**Exit, each seen failing first:**
- a lost response yields `DeliveryUnknown`, and a retry without a probe is refused;
- a probe answering `Done` produces no second effect and settles the record;
- a probe answering `NotDone` permits one retry under the same key, and the destination sees one effect;
- a destination that declares itself non-idempotent is never retried, whatever the probe says;
- a compensation is refused without the evaluator's permit, and when recorded carries the compensated record's hash;
- an irreversible, unobservable effect on unknown stays unknown: no retry, no probe, the record readable as such;
- a probe past its budget marks the record unresolved and escalated, and a further probe is refused; a probe to a host
  the egress list denies is refused by the gate, not by the destination;
- the composed legs are still enforced on the external path (the `composed.rs` tests pass against the new
  destination);
- *(rev 0.11)* a recorded run that reached the destination once is replayed, and the destination is reached zero
  times in replay, shown by the mock's counter.

**Not built, stated:** no real payment or cloud provider. The mock stands where S3Mock stands for the object store,
and `what-is-proven.md` gets the line.

### X6 · `mycelium test --faults` — the existing proofs against an adopter's declaration

**What:** a command that takes a declaration directory and a configuration, builds a fleet of in-process nodes from
them, and runs the fault classes the repository already proves, each against *this* fleet: `revoke` (a mandate and a
key, the row-Q verifiers), `restart` (persistence replay, the regression floor's fixtures), `partition` (the
membership governor under a cut, in-process via the scheduler seam), `replay` (record the run, replay it, report the
first divergence), and `policy-diff` (two policy revisions over the recorded decisions, reporting any call that the
new revision permits and the old refused). The report lists the guarantee ids exercised, by name, and the ones it
could not exercise and why, in the report's vocabulary (`not_configured`, `not_in_build`, `not_verifiable_here`).

**Why after X4:** the report is the explain join over the recorded run. Without X4 it is prose.

*(rev 0.8)* **It never runs an activation.** A unit file can hold `[[activation]]` commands; a test command that ran
them would be a command runner fed by a file. `test --faults` reports that class `not_applicable` and starts no
command, confined or not. **`policy-diff` is exact only where the record carries every input the evaluator read**;
the trace may cut inputs (`inputs_dropped`), and a decision whose inputs are incomplete is reported *unattributable*,
never as unchanged.

*(rev 0.3)* **The `replay` class needs the `sim` build.** The seams are off in every shipped build by rule
(`mycelium-core/src/sim_seam.rs`), and the `mycelium` binary has them only when built `--features cli,sim`, as CI does.
On a binary without them, `test --faults replay` reports the class `not_in_build` naming `sim`, in the guarantee
report's vocabulary, and exits non-zero only if the class was asked for by name. The other four classes run in any
build. The developer-build line is in the command's help and on its feature page.

**Doors:** `mycelium-sim`'s kernel and seams; the scheduler seam's two arms; the guarantee registry; the bundle writer;
the unit-file loader and `wire-check` (a declaration with findings is refused before the run).

**Exit:**
- run against X2's scaffold in CI, every class exercised, the report checked against expected ids;
- a planted policy regression (a revision that permits a previously refused call) is reported by `policy-diff`;
- a planted revocation that does not reach one verifier fails the `revoke` class, shown by removing one verifier's
  check in a throwaway worktree;
- a declaration that `wire-check` refuses is refused by `test` with the same finding;
- a declaration with an `[[activation]]` runs no command, shown by a plant whose command would write a file;
- on a binary without `sim`, `replay` reports `not_in_build` and the other classes still run, shown by a test that
  builds the binary both ways.

**Not built, stated:** a cross-process partition needs Docker and is the existing suites' job; `test --faults` says so
when asked for it. No load class (V1 remains what it is on `what-is-proven.md`).

*(rev 0.12, 2026-10-24, from #675's review)* **What was built against this text.** `policy-diff`'s *recorded decisions*
are the decisions this command's own enforcement point makes over the declaration's call set — every `[[rule]]` as
written and without its values, every wired edge's two call shapes — recorded under one correlation id in a journal
of the run's; no provider or gateway door is driven, and an evidence record carries no argument values, so a journal
written elsewhere (`--decisions`) is re-decided from subject, operation and resource alone and a record a proposed
rule reads values for is *unattributable*. "The explain join over the recorded run" is the correlation id the report
names, over the journal `--keep` leaves. `restart` replays the fleet's own WAL, not the regression floor's golden
fixtures (CI replays those on their own). `partition`'s cut is a dropped runtime on wall time, not the scheduler
seam. `replay` records each node alone, two governor periods, because frame arrival between nodes is outside the
seams. The declared profile is not applied to the test fleet (it needs a gateway and code attachments the fleet has
not); the fleet runs under `dev` and the report says what was declared.

### X7 · The adapter contract table

**What:** `docs/reference/adapter-contract.md`, one row per runtime adapter (`mycelium-py`, `mycelium-ts`,
`langgraph-checkpoint-mycelium`, `strands-session-mycelium`, `mycelium-reason`, the A2A door, the MCP door) and one
column per property: session persistence, cancellation, tracing (X4's id), identity propagation, grants (X3),
provider-side enforcement, receipts. Each cell is *carried*, *not carried* or *not applicable*, and a *carried* cell
links the test that shows it. The companion-registered guarantees are the generated part; the property cells are
hand-written and gated: the F2 gate gains a check that every adapter has a row and every *carried* cell names a
test CI ran (the `--executed` mode).

*(rev 0.11)* **One limits section under one name.** Three companion READMEs carry one under three names and seven
carry none (§1); X7 gives every companion a *What this does not establish* section, and the table's *not carried*
cells cite it.

**Exit:** the table exists, is gated, and every *carried* cell's test is in the coverage universe; a planted cell
naming a test that did not run fails the gate; every companion README has the section, checked by `check-materials.py`.

**Not built, stated:** the table records; it does not make an adapter carry anything.

### X8 · A per-unit `runtime` for SDK agents

**What:** the unit file's `principal` section gains an optional `runtime = "python" | "typescript" | "strands" |
"langgraph"` key for a unit an SDK agent will load with `declare_from` / `declareUnits`. `wire-check` reads it and
reports, per unit, which X7 properties the runtime carries, so the overlay says what a fleet of mixed runtimes
enforces where. No new engine, no fleet-level file: a directory of unit files is the fleet file, as the stem suite
already treats it.

**Exit:** `wire-check` on a directory mixing a Rust unit and a Strands unit prints the carried properties per unit;
an unknown runtime is a finding by name (a unit file with an unknown key is already refused, 2.35.0 §49).

### X9 · Selective recovery — the decisive demonstration

**What:** the plan's exit, in the shape of §12.1's gallery: one runnable demonstration, in CI, that the rows compose.
Three nodes under the replay seams. One external effect succeeds at the mock destination and its acknowledgement is
lost (`DeliveryUnknown`). Dependent work starts on the other two nodes, carrying the parent. Independent work runs
beside it. One node crashes and restarts from its WAL. The fleet then:

1. probes before it retries, and the destination sees **one** effect;
2. reads the closure of the unknown record from the three journals and names every dependent record and no independent
   one;
3. resumes, compensates or leaves each member of the closure according to its declared class, each as a new record;
4. explains every recovery decision from the records with `mycelium explain --closure`, and `mycelium test --faults`
   reports the guarantee ids exercised.

The same run under a partition (the membership governor under a cut, in-process) and under two concurrent retries.

*(rev 0.8)* **Two forms, one set of assertions.** The CI form runs under the replay seams (`sim`), deterministic, with
the plants, and is the recording in the corpus — *(rev 0.11)* replayed in CI as well as recorded, with the assertions
made on the replay and the destination reached zero times during it. The live form runs on the published binary with no seams, as an
example the `one-hour` job executes (§1.1 step 5), with the same assertions and a bounded wait where the CI form has
a seam tick. The live form is what a developer runs in their hour; the CI form is what the repository proves.

**Doors:** none of its own; it exercises X4's registry and X5's classes. Its plants are the independent work (must be
untouched), a second effect at the destination (must not happen), and a journal withheld (the closure must say so).

**Exit:** the demonstration in CI with the plants; a recording of it in the replay corpus; its page under
`docs/features/`; a line on `what-is-proven.md` stating what it shows and what it does not (one destination, mocked;
three nodes; the closure read offline). `docs/philosophy.md` already states the principle (rev 0.2 added the
*Rollback → compensation* bullet under *How Promise Theory handles coordination problems*, with the relational-arrow
reading cited as motivation and the closure reader marked *planned, not built*); X9's PR changes that parenthesis to
*built* and nothing else on the page.

**Not built, stated:** a real destination; more than one destination; a closure read at runtime; any policy over the
closure's size (§6).

*(2026-10-25, delivered)* The demonstration is the **`recovery` class of `mycelium test --faults`** — the published
binary is its live form, which is what let the one-hour job run it — and to get there **the side-effect contract
moved into the substrate** (`mycelium::effects`; the companion re-exports it unchanged; the ledger is the node-local
journal, `rusqlite` staying quarantined in the companion), decided by Richard between the three options (move · keep
in the companion and leave step 5 unmeasured · a fourth published binary). Beside the row's story the run found that a
withheld journal hides what hangs below it (a dependent two steps down through the dark node's record leaves the
closure with it, and the reader names the hole), so "names every dependent" is the complete fleet's property and the
incomplete one says so — the honest reading of the plant. The partition and the two concurrent retries are in the
one run (phases B and D, the node dark between them; `claim_retry` under the ledger's lock). Evidence in §8.

### X10 · `mycelium declare --watch` — the declaration loop *(rev 0.4)*

**What:** a watch over a project's `units/` and `skills/` directories. On a change it runs `wire-check` and, if there
are no findings, re-declares the units through the gateway's `POST /gateway/units/declare` and rewrites the changed
skill text under its `prompts/` key. A finding stops the re-declare and is printed; the fleet keeps the last good
declaration. No dev server, no restart of the node: the loop is over data the node already accepts live. The stem's
SIGHUP re-read of its egress allow-list (2.36.0) is the precedent for a live re-read of a declaration file.

**Doors:** the `units/declare` route and its scopes; the `prompts/` write path (the raw KV routes refuse it since
2.27.0, so the write goes through the route that owns it); the filesystem watch (*(rev 0.11)* a new `Input`-class seam for the
event source — none exists for one today — added to the seam baseline in the open, so a recording of a watched session
replays); `wire-check`'s exit codes.

**Exit:** editing a unit file under the watch changes the overlay the fleet reports within one declare, shown in CI
with the scaffold; a unit edit that `wire-check` refuses leaves the previous declaration in place, shown by a plant;
the watch adds no sim-seam baseline entry outside the seam *(X10's PR: **not built** — the seam is not built, the
watch's seven file and timer sites are in the baseline, in the open, with the inventory row that says why)*.

**Not built, stated:** it declares; it does not build or restart the program. A change to `src/` is `cargo run`.
*(X10's PR)* It applies to a node with a gateway: the Python scaffold's node has one, the Rust scaffold's nodes do
not (their program declares in-process through `run_capability_probes`). The seam for a filesystem event source is not
built; the watch polls modification times and sizes and runs no node, so there is nothing to replay.

### X11 · The documentation and examples pass *(rev 0.7)*

**What:** the closing row. Every instrument it uses exists; the row is their run over the surface X0–X10 changed, and
the fixes that run finds. In order:

1. **The front doors.** `README.md`'s intent router, the capability compass and the five steps route to the new pages
   (`scripts/check-front-doors.py` keeps structure and targets resolving; the prose is the row's). The `Developer`
   route in `docs/README.md` starts at the published node and the scaffold, not at a toolchain. The README's *See it
   work* examples stay the examples page's chooser examples, exactly, which the gate checks.
2. **The concepts chapter.** A glossary entry with an `<a id>` for every term the plan introduced — grant, correlation,
   parent, closure, effect class, reconciliation probe, declaration watch, effective configuration — each linked from
   the feature page that owns it, and none defined a second time elsewhere (`/wiki-lint`'s duplicate-definition check,
   and F2's gate on glossary rows).
3. **The examples.** Every new example (`init`'s two templates, `authority_grant`, `selective_recovery`, the
   `declare --watch` scaffold run) has a row in the example index following its doc template, a README that says what
   it demonstrates and what it does not, and a CI column entry; `check-materials.py` resolves its links. An example
   that runs but no longer demonstrates its feature is the failure this step looks for by reading, since no script
   can.
4. **A tutorial: `docs/guide/tutorials/07-the-first-hour.md`.** The `one-hour` job in prose, step for step, with the
   same commands, so a reader and the job walk one path; *(rev 0.11)* a test diffs its fenced blocks against the workflow's steps,
   and the job's run is the proof that they do not fail as written (the front-door gate checks only that links resolve).
5. **The guide and the runbooks.** Guide 20 gains the grant; guide 18 the effect classes and the probe; guide 19 the
   closure reader; guide 05 skills as files; `operations/diagnostics.md` gains `doctor` and `test --faults`;
   `operations/deployment.md` starts from the published artefact; `production-readiness.md` cites `doctor` for each
   check it can answer. The SDK READMEs route to the Python template and drop the toolchain instruction to a fallback.
6. **The feature pages.** One per row, in the registry, gated (rule 5); the capability map's rows list them.
7. **The three operator-run audits, run and their findings closed:** `/doc-coverage` (the WHAT · WHY · HOW × Dev · Ops
   matrix, so each new concept has all six landings or a stated gap), `/wiki-lint` (doc-versus-code first), and
   `/publication-lint` (the persuasion surface against shipped reality — where the one-hour sentence is checked
   against the job's measurement before it is printed anywhere).
8. **The wiki ingest.** The architecture and testing pages cite the new code; a dated entry in the section's `.log/`;
   `dev/history.md` gains the delivery line.

**Doors:** the front-door gate, `check-materials.py`, F2's feature-pages gate, the examples matrix check, the
coverage universe (every new example's tests listed), the three skills above.

**Exit:** every gate green on the closing PR; the three audits' reports attached with zero open findings or each
remaining one recorded as a stated gap on `what-is-proven.md`; the tutorial's commands equal the job's, shown by a
test that diffs the fenced blocks against the workflow's steps; an independent reader, given only `README.md`, reaches
a refused-and-explained call by following links and the tutorial, without the plan.

**Not built, stated:** the pass is a point in time; the gates and rule 5 are what keep it true afterwards.

## 4 · Order and dependencies

| Row | Depends on | Size | Release |
|---|---|---|---|
| X0 publish the node | nothing (CI and the runbook only) | small | the next tag after it merges; no API change |
| X1 doctor | F2 merged (registry entry) | small | a MINOR, additive |
| X2 init, both templates | X0, X1 | medium | same MINOR as X1 |
| X3 grant | nothing; the largest review | large (a stack: state machine · preflight and provider · doors and SDKs · A2A) | its own MINOR; `AgentPolicy` gains a field (§ in `deprecations.md`) |
| X4 one id, one parent | nothing | large (the door registry is the cost) | its own MINOR; `DecisionRecord`, `AeEvidence`, the receipt gain two fields |
| X5 side effects | nothing | medium | companion release, `mycelium-effects` |
| X6 test --faults | X4, X2 | large | after X4; the `replay` class under `sim` only |
| X7 adapter table | F2 merged; X3 and X4 for two columns (cells read *not carried* until then) | small | docs, any time |
| X8 runtime key | X7 | small | with X7 or after |
| X9 selective recovery | X4, X5, X6 | medium (a demonstration over built parts) | the plan's exit; released with whichever MINOR follows X6 |
| X10 declare --watch | X2 | small | with X2 or after |
| X11 documentation and examples pass | every other first-wave row | medium (reading, not code) | the closing PR of the wave; the release that carries it prints the measured hour |

Parallel tracks: **X0** alone and first, **{X1 → X2 → X10}**, **X3**, **X4 → X6**, **X5**, **X7 → X8**, then **X9**
when X4, X5 and X6 have landed, and **X11 last**. The `one-hour` job (§1.1) is extended by each of X2, X3 and X9 as
they land, and X11's tutorial is its prose. Four sessions can run without touching the
same files, except `deprecations.md` and the CHANGELOG, where the keep-both rebase rule applies.

## 5 · Decisions proposed (D1–D13 to confirm before X3 starts; D14–D18 before the second wave)

| # | Decision | Reason |
|---|---|---|
| D1 | A grant is a mandate, not a new signal kind. | Every door that checks a mandate checks the grant; the bypass matrix holds without a re-run. |
| D2 | The veto mode stays, labelled *veto*. | Removing it breaks the guardrails tier table and the fleet demos; a label is honest and cheap. |
| D3 | The correlation id is carried, never minted. | Minting needs a draw or a clock inside a decision; the trace's rule forbids both. A caller without one gets *unattributable*, not a generated id. |
| D4 | No fleet-level manifest; a directory of unit files is the fleet. | The philosophy page: no target-state graph held anywhere. The stem declaration directory already is this. |
| D5 | The scaffold template is a built project, not a string. | A string drifts silently; a project fails CI. |
| D6 | `test --faults` runs in-process only. | A library runs no Docker for the user; the cross-process suites remain CI's. |
| D7 | The external-effect probe is required before a retry, not optional. | An optional probe is the exactly-once lie the receipts ADR exists to refuse. |
| D8 | The dashboard waits. | `doctor` and the guarantees route first; a dashboard that becomes necessary is a control plane. |
| D9 | A parent is a content hash, carried by the caller; the graph is read offline from node-local journals. | A name can be forged or re-pointed; a hash cannot. A node that held the graph would be a coordinator, and a journal that gossiped would disseminate evidence the design keeps local. |
| D10 | A compensation is a new effect under the evaluator, never a reversal. | `ROADMAP.md` D33: an effect another participant has acted on is not undoable. A reversal that bypassed the evaluator would be the laundering chapter 21 describes. |
| D11 | The causal exposure policy waits for X9. | Its input is the size and age of a closure; until the closure can be read, the rule would count nothing. |
| D12 *(rev 0.3)* | The closure claims completeness over the evidence journal only. | The decision trace drops under contention by design, and a plan that promised completeness over it would claim what the sink refuses to promise. |
| D13 *(rev 0.3)* | Correlation and parent ride inside the signed body. | A header is outside the caller signature's digest; causal attribution must be as hard to forge as the records it links. |
| D14 *(rev 0.5)* | A context summary is a selection; a condensed text is a new signed record, linked to its sources, scoped by the caller's mandate. | Judging is not recording: a blend across verdicts is a record nobody signed. |
| D15 *(rev 0.5)* | A schedule's tick is a seam tick; a missed run is a catalogued decision; the holder is a presence band of one, selected by rendezvous, leased by evaporation. | The voluntary singleton, not a scheduler; a missed run has typed outcomes and so is explained, not replayed. |
| D16 *(rev 0.5)* | A dashboard reads one node's routes from the browser; any read that would need an aggregating process becomes a node route instead. | A process that holds the fleet's truth is the coordinator through the side door (`scoped-mandates.md`). |
| D17 *(rev 0.5)* | A span carries a record's identity and outcome and its journal hash, never its inputs or chain links; the exporter is labelled *not an evidence path*. | The journal is node-local on purpose; a span store must not be readable as what was enforced. |
| D18 *(rev 0.5)* | The exposure policy holds only new dependent work, on two thresholds (size, oldest age), counted as K2 counts, shadow first. | The stability record's decisive rule; K2's coverage semantics; the profile ladder. |
| D19 *(rev 0.6)* | The plan's claim is *one hour to a governed fleet*, printed only as measured by the `one-hour` job on a tagged release. | A claim on the persuasion surface is true only with a gate that would fail if it were not (litmus 5). |
| D20 *(rev 0.6)* | The node is published as a binary and an image; the toolchain becomes the fallback path. | The first-hour cost the comparison found; a library an SDK developer cannot start is not embedded, it is absent. |
| D21 *(rev 0.7)* | A row closes with its documentation, and the wave closes with one pass over the whole surface. | Rows written apart drift apart; the gates catch structure, only a pass catches a page that is true and routes nowhere. |
| D22 *(rev 0.8)* | Issuing a grant is itself an evaluated action; a pending request is a route-owned KV record with a readable rendering. | A token is not an approver; a digest is not something a human can approve. |
| D23 *(rev 0.8)* | `test --faults` runs no activation; the unknown loop has a declared budget; X9 ships in a seamed and a live form. | A test command is not a command runner; an unbounded probe is an unbounded promise; the hour runs the binary the developer has. |
| D24 *(rev 0.9)* | An Eve agent directory runs on the fleet as a pinned demonstration, never as a tracked compatibility surface. | A preview API moves; a demonstration says what mapped on one day, a promise would have to keep moving with it. |
| D25 *(rev 0.10)* | A `fed.*` guarantee family and a `secure-multi-domain` profile; the seven node-resolvable guarantees required — the six of rev 0.10 and `fed.domain_enforced` (`domain_profile = enforced`; *rev 0.13, agreed with the maintainer 2026-10-26*) — the partner-side ones listed as `not_verifiable_here`. | A profile says what this node checked; a multi-fleet posture with no profile is a posture nobody refuses `start()` for. |
| D26 *(rev 0.10)* | The secure template is the two-domain scaffold; a single domain is that scaffold with one fleet. | The multi-fleet reader is the one the secure template exists for; one template, not two, drifts less. |
| D27 *(rev 0.10)* | The scale job provisions its own runner and fails by name when it cannot; its figures move to the proof page's first table with the platform and the cost. | A criterion that depends on a box being on has never been met; a measured number with its platform is a claim, an unmeasured one is not. |
| D28 *(rev 0.12, 2026-10-25)* | Y3 extends the Ops Console rather than adding a fourth page; the console's server and proxy go, and a node gains an **opt-in** CORS setting — off by default, an origin allow-list — so the browser reads each node's routes directly. | Two dashboards over the same routes drift apart; and a proxy that fetches for the browser is the aggregating process D16 forbids, however stateless. Cross-origin reads widen what a gateway exposes to browsers, so the setting is off unless an operator names the origins. |

## 6 · The second wave — deferred rows, their questions decided *(rev 0.5)*

Each row below was deferred in rev 0.1–0.4 behind a first question, or added by the Eve comparison (Y6) and the
multi-fleet reading of this repository (Y7–Y10, §6.1). The questions are decided (D14–D18 and D24–D27, §5), and the
rows are sketched to the decision so the second wave can start without re-opening them. They stay behind the first
wave: none is in §4's table, and the plan is agreed as a whole; each second-wave row still starts only when the maintainer calls it. Four of the five resolve into mechanisms
the repository already has — a record, a seam, a route, a counted rule; only Y1 adds a component, and its output is an
ordinary record.

### Y1 · A provenance-aware context selector (D14)

**What:** a selector over the knowledge layer and the wiki that returns records for a run, ranked by capability and
task, each with its verdict, standing and issuer intact. A summary is a selection, not a synthesis. Compression is
allowed only within one verdict class, never across `InsufficientEvidence` and `Rejected`, and never folding a
`Retracted` or `BasisWithdrawn` record into live ones. If an LLM condenses the selection, the condensed text is written
back as a **new record with its own issuer**, linked by the existing link kinds to every record it drew on, so it stands
or falls as a claim of its own. The selection is scoped by the caller's mandate: an agent cannot read into context what
it could not read directly.

**Depends on:** nothing in the first wave; the knowledge layer's link kinds, verdicts and issuer binding as they are.

**Exit:** a test shows a condensed record refused without an issuer, and refused when it cites a record across a verdict
boundary; a mandate-scoped selection excludes a record the mandate does not cover, shown by a plant.

**Not built, stated:** no vector store of its own; ranking is the adopter's, the selector only enforces the boundaries.

### Y2 · Declared recurring work (D15)

**What:** a unit-file section in the shape of `[[presence]]`: a band of **one holder per schedule**, selected by
rendezvous among the hosts that declare it, with the holder's record evaporating on TTL as the lease. That is the
voluntary singleton of the Promise Theory section, not a scheduler. The tick that fires a run goes through the timer
seam like every periodic loop, so a recording replays it. **A missed run is a decision:** what the holder does about a
run that did not happen — skip, run late, or run once for the gap — is a typed outcome with reasons, in the rule
catalogue and traced. Duplicate suppression is X5's contract: a run's effect carries the schedule id and the slot as its
idempotency key, so two holders in a partition produce one effect.

**Depends on:** X5 (the idempotency key) and the rule catalogue's generator.

**Exit:** two holders under a partition, one effect at the destination, shown with the seams; a missed-run decision in
the trace with its reason; the schedule section refused by `wire-check` without a declared holder band.

**Not built, stated:** no cron-expression language beyond a period and a phase; no fleet-wide calendar.

### Y3 · A dashboard that observes (D16, D28)

**What:** *(rev 0.12, D28)* the existing **Ops Console** (`examples/ops_console`), extended rather than a fourth page
beside the `*_viz` examples: a page that runs in the browser against the nodes the operator points it at and reads only
their existing routes — beside the console's `/stats`, `/gateway/fleet`, `/gateway/diagnose`, `/gateway/audit` and
`/metrics`, the guarantees report, the tuple overview and the governor snapshot. Fan-out across nodes is the browser's:
the page takes a list of nodes and merges their answers itself. The dependency closure stays the offline reader (X4)
and is not a panel.

**What changes on a node:** an opt-in CORS setting on the gateway — off by default, an origin allow-list, read-only
routes only — so the page needs no server of its own. The console's `:8099` server and its `/api?host=…&path=…` proxy
are removed; the page is a static file.

**The rule:** if a panel ever needs a process that polls every node and holds the result, the answer is a new route on
a node, never that process.

**Depends on:** X1 for the guarantees vocabulary it renders.

**Exit:** the console runs in CI against a three-node fleet and renders each panel from a route named in its README;
a test asserts the example opens no listener of its own; a gateway with the CORS setting unset answers a cross-origin
read with no `Access-Control-Allow-Origin`, and with it set answers only the origins listed, on the read-only routes.

**Not built, stated:** it is an example, not a product; no authentication of its own beyond the gateway's.

### Y4 · Trace export as spans (D17)

**What:** an exporter under the existing `otel` feature, from the node as SkillRunner already exports. A span carries
the identity of a record and its outcome — correlation, parent, rule id and revision, outcome kind, reason, and the
record's journal hash as an opaque reference — and **never** its inputs, subjects, mandate contents or the chain's links,
which are the parts kept node-local on purpose. A holder of a span asks the node for the record by hash, under the audit
scope, and verifies it there. The guarantee report labels the exporter *not an evidence path*.

**Depends on:** X4 (the two fields).

**Exit:** a span's field set pinned by a golden; a test shows an input snapshot never reaches the exporter; the
guarantee report's line present.

**Not built, stated:** no span store is read by anything in the repository.

### Y5 · The causal exposure policy (D18)

**What:** a `[[rule]]` whose input is an actor's unresolved closure — its size and the age of its oldest unresolved
record, two thresholds, since the second grows while nothing is done. Counted as K2 counts history: a consensus seat
for an exact bound where an action warrants it, a gossiped tally with the staleness bound otherwise, unknown coverage
failing closed. **It may hold only new dependent work** — a call, a take, an emit that would extend the closure. It
never holds a reconciliation probe, a compensation, a protective shed, or a rescue from zero: the stability record's
decisive rule applied unchanged. It starts in **shadow** under the profile ladder, counting what it would have held,
before it holds anything.

**Depends on:** X9 (the closure as an input), K2's counters.

**Exit:** a plant shows a compensation admitted while a new dependent call is held; the shadow profile counts and holds
nothing; the aged-record threshold fires with the size threshold unmet.

**Not built, stated:** the same limit K2 states — unsigned tally entries.

### 6.1 · The multi-fleet weaknesses, and the row for each *(rev 0.10)*

Asked whether Eve is limited for an enterprise multi-fleet strategy, the honest answer was *yes, by design*, and the
same question turned on this repository found four things it should say about itself. Each gets a row below, and
the first-wave row that already covers the fifth is named.

| Weakness, as a multi-fleet reader finds it | Row |
|---|---|
| the only enforced profile is `secure-single-domain`; nothing makes `start()` refuse unless the federation posture holds, and the guarantee catalogue has no federation family | Y7 |
| no scaffold walks two domains; the runbooks describe a federation deployment that no template produces | Y8 |
| the nightly scale runner has never had a runner registered, so the V1 criterion has never been met and there is no end-to-end load figure | Y9 |
| no named third-party production deployment | Y10, and the counterparty it still needs |
| Rust at the node | X0 |

### Y6 · An Eve agent directory, unchanged, on a fleet *(rev 0.9)*

**What:** a loader in `mycelium-ts` that reads an Eve `agent/` directory as published by the `eve` package (a
preview; the loader pins one version and says which) and declares it as a unit of the fleet. The demonstration is
Eve's own tiny example, the weather assistant with one tool, run unchanged: a grant that holds across a restart on its
tool, and a denial proved from the records. It is a comparison sharper than any table, and it is **a demonstration,
not a compatibility promise**: Eve's runtime is Eve's product, and the loader follows one pinned version, not the
preview as it moves. A companion with the limits section X7 establishes, first.

How each folder of an Eve agent lands, and the two that do not:

| Eve | On the fleet | Status |
|---|---|---|
| `instructions.md`, `skills/*.md` | prompt skills loaded from files (X2's layout) | maps |
| `tools/*.ts` (`defineTool` with a zod schema and `execute`) | MCP tools served through the TypeScript SDK, the schema carried as the tool's | maps, through the loader |
| `subagents/<id>/agent.ts` | another unit, found by capability — discovery, not a parent calling a child | maps, differently; the README says how |
| `connections/` (MCP servers) | tool discovery (guide 06) | maps |
| `channels/` HTTP | the gateway and the A2A door | maps; Slack does not |
| `schedules/*` | declared recurring work (Y2) | after Y2 |
| approval policies | the grant (X3), verified at the provider | maps, and is stronger |
| `sandbox/`, `getSandbox()` | confined activations (row M) stand in for the confinement; the Vercel Sandbox API has no counterpart | **does not map** |
| Workflow hooks (`start`, `resumeHook`, `createHook`) | nothing; the task lifecycle companion is still open below | **does not map** |
| `hooks/` | nothing | **does not map** |

**Depends on:** X7 and the TypeScript template; Y2 for schedules; X3 for the grant it demonstrates.

**Exit:** the weather example's directory, unmodified and pinned by version, runs in CI on a three-node fleet from the
loader; its tool is called through `/mcp` once permitted and once refused, the refusal explained from the records; a
grant on the tool survives a restart (X3's test, run here); the README's table above is checked by a test that walks
the directory and names each folder's status; a folder the loader does not map is refused by name, not ignored.

**Not built, stated:** the three *does not map* rows; any Eve version but the pinned one; Slack and the other channel
helpers; the `agent.ts` model string, which the fleet routes through its own inference router (row R) rather than a
gateway Eve names.

### Y7 · `secure-multi-domain` — the profile, and the guarantee family it needs *(rev 0.10, D25)*

**What:** a federation guarantee family in the catalogue, `fed.*`, resolved by the same report as the rest, and a
second profile, `secure-multi-domain`, that requires `secure-single-domain`'s twenty (rev 5, `production-readiness.md` §2) and the federation ones on
top. Today the catalogue mentions federation only inside the two egress guarantees' descriptions; a node federating with a
partner reports nothing about the edge it opened.

**The first question, answered:** which of the federation guarantees resolve on this node, and which only at the
partner. On this node, and therefore required by the profile: the partner's SPKI is pinned in the `TrustBundle`
(`fed.pinned`); `require_body_binding` is on (`fed.body_bound`, the 2.12.0 window closed); the per-partner in-flight
cap is set (`fed.partner_slots`); the credential names the local caller and never the node (`fed.caller_named`); the
signed catalogue is verified before an export is served (`fed.catalogue_signed`); every partner host is on the egress
list (`fed.partner_egress`). Verifiable only with the partner, and therefore reported `not_verifiable_here`, listed,
never counted: the partner's own posture, non-merger (two meshes that do not become one is a fact about both),
and clock agreement across the edge. The profile is **rev 1** when it ships, with the same revision discipline as
the single-domain one.

**Depends on:** nothing in the first wave; X1 so `doctor` reports the new family.

*(rev 0.13, 2026-10-26; agreed with the maintainer 2026-10-26, D25)* The delivery adds a seventh node fact, `fed.domain_enforced` — `domain_profile = enforced`,
TLS required and SWIM off — since SWIM's unauthenticated datagrams at a federation boundary are a posture this node
can check and the profile should not admit; §8 has the evidence.

**Exit:** a node under `secure-multi-domain` with one required guarantee unmet refuses `start()` naming it, shown for
each of the seven (six before rev 0.13); the two-mesh Docker suite runs under the profile; `doctor` prints the family; the catalogue golden
regenerated; `deprecations.md` gains the entry.

**Not built, stated:** nothing here makes the partner honest; the profile says what this node checked.

### Y8 · The two-domain scaffold *(rev 0.10, D26)*

**What:** the secure template, grown to the shape a multi-fleet reader needs: `init --template two-domains` writes
two fleets of two nodes each, from the published binaries (X0), each under `secure-multi-domain` (Y7), with
certificates issued off-node by `mycelium tls issue`, named tokens, egress lists naming the partner, a pinned SPKI
each way, and one exported capability called across the edge with a credential naming the local caller. The
README's last lines are `doctor` on each node and a call that is refused when the pin is wrong. It subsumes the
secure single-domain template proposed in rev 0.4: one domain is the two-domain scaffold with the second fleet
deleted, and the README says so.

**Depends on:** X0, X2, Y7, X7 (so the template can say what each adapter carries across the edge).

**Exit:** the scaffold runs in CI from the published binaries; the cross-edge call succeeds once and is refused under
a wrong pin, a replayed body and an exceeded slot cap, each shown; every node's `doctor` is green under the profile; and,
**carried from Y7 (#691)**, a node holding a client shows `fed.pinned`, `fed.caller_named` and `fed.partner_egress`
`enforced` in its own `GET /gateway/guarantees` while it runs — not only `doctor` green, which cannot see a client attached
in code — since the two-mesh suite's consumer is a bare `FederationClient` and those three read `not_applicable` on
every node there. If Y8 grows that suite rather than starting fresh, its beta nodes sit on `beta-net` only: a beta
node holding the client must also be attached to `edge`, where alpha's gateways are.

**Not built, stated:** three or more domains stay the test `three_domains_compose_without_trust_composing`, not a
template; no cloud provisioning.

### Y9 · A scale run that runs *(rev 0.10, D27)*

**What:** V1 as the proof page records it: the hosted `scale-nightly.yml` waits on a `[self-hosted, mycelium-scale]`
label that has never had a runner, so it queues silently, and the local launchd runner's last rows failed on the
environment. The row makes the criterion independent of any box being on: the workflow **provisions its own runner**
for the run — an ephemeral cloud VM or a large hosted runner, created by the job, registered, used, and torn down
by the same job, with the cost stated on the proof page — and records the measured figures the page lacks: a
100-node convergence time and an end-to-end throughput and latency under sustained load, as rows in
`what-is-proven.md`'s first table rather than its third.

**The first question, answered:** where it runs and who pays. A runner the workflow provisions, so a missing runner
fails the job instead of queuing it; the budget is a number on the page, reviewed at each release like a dependency.

**Depends on:** nothing; a credential for the provider, held as a repository secret.

**Exit:** the nightly job completes green on three consecutive nights on a runner it provisioned; the proof page's V1
row moves to the first table with its figures and the date; a missing credential fails the job by name within a
minute, never queues.

**Not built, stated:** the figures are the run's platform's; a different network is a different number, and the page
says which platform.

### Y10 · The pilot kit *(rev 0.10)*

**What:** the one weakness code cannot close is a named third-party deployment: it needs a counterparty, as the
proof page says of AE4. What the plan can build is everything a pilot would need on the first day, so that the
counterparty is the only missing piece: the two-domain scaffold (Y8) as the pilot's starting fleet, the `one-hour`
job as its first checkpoint, `customer-pilot.md` rewritten to walk the scaffold rather than describe one, the
shared-responsibility matrix cross-referenced from each `doctor` line it covers, and a pilot report template whose
sections are the guarantee ids the pilot exercised, in the report's vocabulary.

**Depends on:** Y8, X11.

**Exit:** the kit is a directory the pilot runbook names; a reader with no prior contact runs the scaffold and
produces the report from `doctor` and `test --faults` output alone, shown by the independent-reader check X11 uses;
`what-is-proven.md` keeps the *named deployment* row open and says the kit is not the deployment.

**Not built, stated:** the deployment.

### Still open

- **A task lifecycle companion** (cancellation, deadlines, resumable states) on tuple-space claims and mandates. First
  question, not yet decided: what is the one durable record of a transition, and who may write it when the claimant is
  dead? The lock-lifecycle work (row A) is the precedent for the answer's shape.
- **A secure single-domain template** is no longer a separate item: it is Y8 with one fleet (rev 0.10).

## 7 · Not in scope

Content-safety filters (prompt injection, toxicity); a `mycelium dev` process watcher or dev server (X10 watches
declarations, not code); a second template before X7; any change to the wire; any change to the default posture.

*(rev 0.4)* Two lessons from the compared projects are declined by name rather than left implied:
- **Behavioural evals** — whether an agent chose the right capability, answered well, or stayed on task. They are the
  adopter's, judged by the adopter's own criteria, and a substrate that scored them would be claiming a judgement it
  cannot make. `test --faults` exercises guarantees, not behaviour.
- **A deploy command.** A library has no target to deploy to. The deploy half of DevOps is the runbooks, the release
  upgrade notes and the profiles; the secure template in §6 is as far as the plan goes toward a first deployment.

## 8 · Evidence (rule 2; filled as rows close)

| Row | Promise | Evidence | Reviewer |
|---|---|---|---|
| X0 | binaries for three platforms, sums, the Release created from the tag, the image by version and `latest`; a clean-container smoke; a dry run | `.github/workflows/release.yml`, `docker/Dockerfile.node`; `version_tests::version_line_names_the_crate_version` (`src/main.rs`); dry run 39934199532 on #664 (the bundle verified by the review). **Since:** the first tag, `v2.37.0` (run 39988446683), exercised the Release creation and the image push — and found the checkout peeling the tag (#669); `doctor` in the clean container is X1's, delivered; the `one-hour` job is #670's. The Doors' anchor promise is withdrawn (above). | #664's review, 2026-10-23; v2.37.0 |
| X1 | the same refusal as `start()`, in its words; `not_in_build` naming the feature; a persistence directory reported, not created; the effective configuration with sources; secrets as set/unset; a planted fault in each of the four shown failing | `src/doctor.rs` and its seven tests (`doctor_names_the_refusal_start_would_give` runs both and compares the strings); `preflight`/`profile_check` in `src/agent/lifecycle.rs`, called by `start()`; `validate_does_not_create_the_persistence_base_path` in `mycelium-core/src/config.rs` — **found by the row:** `validate()` created the directory, fixed (`deprecations.md` §57). **Plants, each run in the PR and restored:** a trailing character on doctor's refusal text fails the comparison; the token-table refusal removed from `preflight` fails the `not_in_build` test; `create_dir_all` restored in `validate()` fails both persistence tests; `Env` and `File` swapped fails the sources test. The profile and token tests are pins of a new feature and cannot fail against `main`; the persistence and validate tests did. The field order is the serialised form's, with a doc-table test, not `configuration.md`'s (above). | #665's review, 2026-10-23 |
| X2 | `init` writes a project CI builds and runs; a Rust and a Python template; `init` refuses a non-empty directory; the tag is the binary's version; `main.rs` under 80 lines and `main.py` under 60; a skill edit changes the answer; the Python template runs against the node it is built with | `src/init.rs` and its four tests (`every_template_file_on_disk_is_embedded` holds the directory and the embedded list equal; `init_writes_the_binarys_own_tag_and_no_placeholder`; `init_refuses_a_directory_that_is_not_empty`; `the_programs_stay_short`); `templates/governed-call` (72 lines of `main.rs`) and `templates/governed-call-py` (42 of `main.py`); CI job `Scaffold` (init, wire-check, run with the permitted and refused call asserted, explain, doctor, the skill edited and re-run, the pin checked; the Python run against the node built in the job, the target printed). **Not built:** the published-image run on a release branch (waits on X0's first tag). **Deviations, above:** the Python template's refusal is the gateway's scope rule; the policy is a custom evaluator, not `ReferenceEvaluator`; the skill is served by the member node. **Found by the review:** a `[patch]` of a git source still fetches the pinned tag, so the job rewrites the dependency to a path instead; `init ""` wrote into the current directory, refused now. | #666's review, 2026-10-23 |
| X3 | a grant mode beside the veto one, default-deny; a grant is a scoped mandate bound to tool, digest, agent and policy revision with an expiry, verified at the state machine, the gateway and the provider; the supervisor's door (namespace, routes and scopes, a rule about rules); a human reads arguments; revocation and the receipt; the doors; the exits; a durable request; the A2A question | **First PR:** `AgentPolicy::{require_grant_for, grant_window}`, `AgentStateMachine::transition_invoking` (`src/agent/state_machine.rs`); the grant as a mandate in its own scope with its binding in its signed operations (`src/mandate/grant.rs`: `GRANT_SCOPE_PREFIX`, `grant_operations`, `grant_binding`, `entitle_grants`; the verifier keeps no epoch for grants); `assess`'s grant path and `MandateRefusal::{BoundToOtherArguments, BoundToOtherPolicy}` (`src/agent/gateway_authority.rs`, `src/mandate.rs`); the preflight's policy-revision check (`authz.rs`); `MandateBinding::{digest, bound_policy_revision}`, `AeEvidence::mandate_digest`, `Rule::requiring_grant`; the routes `GET /gateway/grants/pending`, `POST /gateway/grants/{agent}/{request}` under `grant:read`/`grant:issue` with issuing decided by the evaluator (`src/agent/http/grants.rs`); `grant/` route-owned; `mycelium-guardrails` 1.1.0's clause and the veto label; `ApprovalTimedOut` deprecated (§59). Tests: `a_grant_binds_in_its_signed_operations_and_two_grants_never_conflict`, `a_grant_is_bound_to_its_arguments_and_expires_on_the_wall_clock`, `a_plain_transition_to_a_grant_required_tool_is_refused`, and the three door tests in `src/agent/http/grant_tests.rs`. **Decided in the PR:** a grant cannot be a plain `Mandate` in the resource authority's scope — the appointment model allows one holder per scope and epoch — so each grant has its own scope, skips the resource gate's epoch rule and keeps revocation; the binding rides in the signed operations rather than new `Mandate` fields, so no existing mandate, signature or struct literal changes. **Found by #673's review and fixed:** a grant's standing read the gate's freshness, so a still-valid grant stopped verifying after `freshness_ms` (`RevocationView::grant_standing`: the issuer's checkpoint, not freshness); `BoundToOtherPolicy` was refused after the permit was admitted (now before, and a two-revision evaluator sees it refuse); nothing consumed a grant at the agent (the request is marked consumed by its commit); the request record was not on the WAL (it is); the agent asked before its own guards (guards first); `refuse` skipped the evaluator (both answers are actions); `Completed` was recorded whatever the write did; the gauge moved only when polled. **Second PR (2026-10-25):** single use at the doors (`claim_grant_use` + `…/used/{door}`; `a_grant_admits_one_call_at_each_door`, seen failing first: the second direct call ran the tool again); the revocation route and counter (`gw_grant_revoke`, `mycelium_grants_revoked_total`) with the drain bound shown — a slow call under a grant stopped by the revocation inside 1.5 s against a 100 ms sweep (`a_revoked_grant_is_refused_at_every_door_and_stops_admitted_work`), which found and fixed the sweep cancelling granted work at its first tick; issuing across nodes (`is_member_agent`, `entitle_grants_from_fleet`, the checkpoint feeder; `a_grant_is_issued_at_one_node_for_an_agent_on_another_and_revoked_there`); the restart re-wait and the expired re-ask (`a_restarted_agent_re_waits_on_its_pending_request_and_an_expired_one_asks_again`, over `[persistence]`); the doors a node-held grant reaches each refusing a revoked grant and the raw route its protected kind; `prove_denials`' basis — a used grant's refusal sealed as a denial whose reference names the evidence record with the reason (asserted on the audit stream); the SDK verbs since #676. **Found:** an unknown mandate standing carried none of the mandate's facts, so a revoked grant's record named neither the grant nor the standing — the hash is on it now (the unknown binding carries the facts a refused one did); the standing's words are still only in the door's answer and the decision's missing facts, not on the record — a legibility gap stated on the feature page, not closed. **Found by #678's review and fixed:** a grant issued before a node's restart was refused after it for ever — the revocation view's start rule refused the grant's one checkpoint, so `grant_standing` read `Unknown` (a grant's checkpoint is exempt now: it carries no freshness; `a_grants_checkpoint_from_before_the_start_is_held_and_a_resources_is_not`, and the restart test now issues before the restart); the single-use claim sat inside the history-view retry loop, where a retry met its own marker and a same-door refusal after it spent the grant — moved to settling, after the seats; an unknown standing was final at the agent, so a grant landing before its checkpoint was refused — waited out now; the `grant/` watcher woke on every key and re-verified every checkpoint — it wakes on checkpoints and skips those the view holds; the claim keyed without the holder and grew without bound — keyed by holder and pruned by the sweep; the revoke route read the tool from the unsigned request record — from the signed grant now; the SDKs lacked a revoke verb — `grant_revoke` / `grantRevoke`. **Deviation, stated:** revocation is a grant route (`…/revoke`), not "the mandate route" the row named — no generic mandate route exists. **Third PR (2026-10-25):** `GET /gateway/grants/stream` (`gw_grants_stream`; `the_grants_stream_reports_a_request_once_and_then_its_answer`), `grants_stream` / `grantsStream` with stub tests; A2A `input-required` on the streaming send (`Step::InputRequired`, a watcher on the serving node's `grant/` beside `working`; `a_streamed_a2a_task_waiting_for_a_grant_reports_input_required_with_the_request_id`) — the design question decided as proposed, the request's id and nothing else; the rendering under the data-class policy (`AgentPolicy::{grant_arguments_class, grant_renderable_classes}`, `render_arguments_under`; `a_request_withholds_arguments_of_a_class_the_policy_does_not_render`: the value never enters the gossiped record, the digest does). **Found by #679's review and fixed:** the 1.0 state name was built by uppercasing alone (`TASK_STATE_INPUT-REQUIRED`, an enum value no 1.0 client accepts — a unit test and the 1.0 leg of the door test, seen failing first); the adapter contract's A2A grants cell still read not built — carried now; the streamed dispatch's 30 s bound would have failed a task whose supervisor answered later — the unary path's 120 s now, stated as the longest wait through this door; the watcher reported the smallest new request and never a second one — every new unexpired request once, `working` when none stands; the stream reported no expiry — `grant_expired` now; its task outlived its client — ends when the client goes; the rustdoc's default list was wrong and the fields lacked `#[serde(default)]`. **Stated:** the class is the policy's, one name per agent, not row R's per-call `DataClass`; `tasks/get` on a waiting task says `submitted`. **Fourth PR (2026-10-25):** the `grant` class of `mycelium test --faults` (`src/faults/grant.rs`; `the_grant_class_refuses_on_silence_issues_by_the_declared_rule_and_holds_across_a_restart`, five checks: default-deny on a plain transition and on silence, the declared issuer's grant admitting one call with a stranger refused by the evaluator, the request and the grant replayed from the WAL across a restart with the agent re-waiting and not asking twice, other arguments asking again, the decisions on the record; `the_grant_class_needs_a_grant_rule_tls_and_persistence_and_says_so`) over a `[[rule]]` with the new key `requires_grant = true` (`RuleDecl::requires_grant` → `Rule::requiring_grant`; `a_rule_requiring_a_grant_and_an_unknown_rule_key`) and the actor of a `grant.issue` rule as the issuer; the issue and revoke routes' decisions factored into `decide_grant` / `revoke_grant`, which the class calls (the fleet has no gateway port); both scaffolds declare `tool:purchase.approve` under the treasurer's grant; the `Scaffold` job greps `grant: passed` and **the one-hour job's step 4 runs the class on the published binary** (the job measures all six steps from the first release carrying it); `wire-check` warns `unissuable grant` (`a_grant_required_rule_admits_the_edge_and_warns_when_nobody_may_issue`); a `[[rule]]` or `[[mandate]]` with an unknown key is refused by name (§65). **Stated:** the class drives the state machine's door and the issuing door in one process — the gateway and provider doors and revocation are the door tests'; a declared `tool:<name>` is also admitted at `tool:<name>@<node>`; the stop is graceful. **Stated, not built:** a bearer client's grant at `/mcp` (only a `node:` principal can hold a grant, so the gateway's `used/gateway` claim is code with no reachable caller — exercised at the provider's tag in the shared preflight); retention of `grant/` records; a per-issuer entitlement finer than `entitle_grants_from_fleet` (the narrower `entitle_grants(node)` exists). The request and markers are on the WAL at the buffered rung; the exit's "crash" runs as a graceful restart in the test. | #673's review, 2026-10-24; #678's, #679's and #680's, 2026-10-25 |
| X4 | two fields carried from the envelope into every record and minted by none; body placement only, a header or envelope copy never read and counted; `explain --correlation` and `--closure` with captured/reconstructed/unattributable, holes named with the sinks' counts, fewer journals than nodes said first, a federation boundary; the door registry; the comparison unchanged and the corpus byte-identical; a fuzz target for the reader | **First PR:** `src/agent/causal.rs` (the rules, the hash), `ActionEnvelope`/`AeEvidence`/`DecisionRecord`/`WriteReceipt` fields, `ae_decide` reading `params._meta` and refusing `Malformed` at every door, `/mcp` forwarding the two keys inside the `tools/call` payload and `provider_enforcement::check` reading them there, `causal_misplaced` counted at `/mcp`, `/a2a`, the LLM doors and the envelope verifier; `src/closure.rs` (`Corpus::chain`, `Corpus::closure`), `mycelium explain --correlation` / `--closure [--fleet]`; tests `the_causal_fields_ride_in_the_payload_to_the_gateway_s_and_the_provider_s_records` and `a_malformed_causal_field_is_refused_before_policy_and_never_corrected` (`src/agent/http/causal_tests.rs`), `a_parent_in_the_envelope_is_counted_not_read_and_one_in_the_payload_is_signed` (`src/agent/gateway_caller.rs`), the reader's four tests (`src/closure.rs`), `the_causal_fields_are_in_the_line_and_outside_the_canonical_tuple` (`mycelium-core`), `the_causal_fields_reach_the_evidence_and_an_old_record_parses_without_them`; the fuzz target `closure_corpus` in the registry and CI's fuzz list; the scaffold's call under `_meta.correlation` and its chain read in CI. **Not built in the first PR, by the row's own text:** the door registry and the member-side doors it lists (signal/wired emits, `bulk_call`, scatter, mailbox, shard, overlay, tuple-space `take`, blackboard, the stem's activation command) — a member's direct call carries the fields as bytes it does not alter, and the provider reads them from an `mcp.invoke` payload only; a skill call's payload is text, so the provider's record of an `/a2a` call is a root; the SDKs' verbs; the reason trace's `run_id` join; the three-node closure with plants on each side and the saturated sink; the X2 scaffold's chain is read in CI, not asserted end to end across nodes, and shows no execution record at the provider and no receipt. **Found by #671's review and fixed:** the reader attributed a journal record by a field the record never carried (`AeEvidence` gained `node`, and a test now holds the helper to the product's serialisation); the fuzz hook's assertion was a tautology (`closure_members`, each member rendered once); a copied journal satisfied `--fleet` (duplicates counted, not journals); a misplaced envelope field was counted per verification and before the signature (once per request, after it); the federation and KV doors did not count a header; §58 claimed a tool handler receives `_meta` (none does); `approx_bytes` omitted the fields; a 687 483-byte first frame sniffed as a trace (the frame is parsed first). **Second PR:** `Preflight::Proceed { journal_sha256 }` and `/mcp` forwarding its decision's hash as the provider's `parent`; `scripts/causal-doors.txt` + `scripts/check-causal-doors.py` (in `make check` and CI; `--self-test` plants a door) — 18 doors, 8 with a test, 10 stated untested by name; `a_members_direct_call_carries_the_fields_in_its_own_bytes_and_the_gateway_s_raw_door_refuses_the_kind`, `the_a2a_door_carries_the_fields_on_its_own_record_and_forwards_none`, `the_llm_door_carries_the_fields_on_its_own_record` (`src/agent/http/causal_door_tests.rs`); `tests/causal_closure.rs` (three nodes, the plants named in its assertions, the saturated sink's hole); `mycelium-py` 0.2.13 / `mycelium-ts` 0.2.7 (`_task_params`/`taskParams` and `set` tests) and `mycelium-reason` 0.11.0 (the gateway test's `run-reason` case). **Found by #672's review and fixed:** the gate's scan stopped at the first `#[cfg(test)]` line, so a door appended after an inner cfg(test) item passed (the scan now ends only at a test `mod`, and the self-test plants exactly that door); the SDKs' LLM and federation verbs could not carry the fields (they do now); the forwarded parent's fallback to the caller's was documented only in a comment; the saturated-sink test described the test's own records as the node's and cited a hash no sink could have held (now stated for what it is). **Still not built:** an execution record at the provider (so the receipt's rung is not on a chain), the stem activation's parent, the tuple-space `take` and the blackboard (no record, outside the scan), the registry rows stated untested. | #671's and #672's reviews, 2026-10-23 |
| X5 | an `ExternalEffect` with `operation_id`, dedup scope, prepared record (rung-1 checkpoint before) and declared class, recorded on the prepared record; the three classes' rules on `DeliveryUnknown` and on a wrong outcome; a reconciliation probe before any retry, a retry without a probe / non-idempotent / second refused by name and counted; a compensation authorised like any other with the compensated record's hash as parent, both shown by the closure reader; a settled outcome as an execution record with the receipt's rung; the probe budget (count and age), unresolved and escalated past it; a probe is egress; apply and probe behind an `Input`-class seam, replay reaches the destination zero times; the SQLite destination extended and a mock HTTP destination with lost responses; the doors (`EffectDestination`, `Counting<D>`, `CompositionLeg`, `Execution`); the nine exits each seen failing first | `mycelium-effects/src/external.rs` (since X9 `src/effects/external.rs`, the companion re-exporting it) (`ExternalEffect`, `EffectClass`, `ProbeBudget`, `Reconciler`, the ledger, `prepared_record_hash`, `ExternalRefusal` with its reasons); `counting.rs` (`ExternalRefusalCounts`, `RefusalSnapshot::external`); `http.rs` (`HttpDestination`); `sqlite.rs` (`impl ExternalDestination`); `src/agent/action_evaluator.rs` (`Execution::Unresolved`); `mycelium-core/src/sim_seam.rs` (`input`, both builds). Exits: `a_lost_response_is_unknown_and_a_retry_without_a_probe_is_refused`, `a_probe_answering_done_settles_the_record_without_a_second_effect`, `a_probe_answering_not_done_permits_one_retry_and_the_destination_sees_one_effect`, `a_non_idempotent_destination_is_never_retried_whatever_the_probe_says`, `a_compensation_needs_the_evaluators_permit_and_names_the_record_it_compensates` (the closure reader shows both), `an_irreversible_unobservable_effect_stays_unknown`, `a_probe_past_its_budget_is_unresolved_and_egress_gates_a_probe`, `the_composed_legs_are_enforced_on_the_external_path` (and `tests/composed.rs` unchanged and green), `a_recorded_run_reaches_the_destination_once_and_its_replay_zero_times` — four plants seen failing (the probe requirement, the budget, the permit, the replay suppression; the PR records the runs). **Decided:** no new `CompositionLeg` — reconciliation is a state of the record; the reconciler is a parallel path, not pluggable through `EffectDestination`/`Counting<D>`/the tuple-space consumer (`composed.rs` unchanged, its own composition test on the external path); the `Input` seam's record is the inventory's §2.4 row (the seam gate does not scan the companion). **Found by #674's review and fixed:** a found `Prepared` record re-sent (now unknown, probed, never re-sent — a dying-sender test); the ledger answering `Replayed` to other content (a found record binds content, class and scope; `DeclarationMismatch`); `retry` sending whatever it was handed and checking no composition (bound and checked); a `CompensationPermit` not bound to its compensation (operation, destination, parent, validity, principal); a lost compensation stranding its record (the link is kept, the settling probe compensates); escalation one call late (immediate, the probe counted before it is made); fabricated `permit` decisions on execution records (indeterminate / the destination's own / the evaluator's). **The receipt's rung:** written where true — `effect_durability: on_disk` for a local destination's commit; a remote commit is the destination's claim, carried as `destination` and the dedup outcome (the local rung cannot name it). **Not built, stated:** a real provider; credential binding on the reference HTTP destination; a policy-authorised probe; a re-authorisation API or route after escalation; the exposure policy over the unresolved age (X9, §6); an operator route over the ledger. | #674's review, 2026-10-24 |
| X6 | a command over a declaration directory and a configuration building a fleet of in-process nodes and running `revoke`, `restart`, `partition`, `replay` and `policy-diff` against it; the report naming the guarantee ids exercised and the ones it could not, in the report's vocabulary; no activation run; `policy-diff` exact only where the record carries every input, else unattributable; `replay` under `sim` only, `not_in_build` elsewhere, non-zero only when asked by name; the exits | `src/faults.rs` (`run`, `Fleet`, the five class modules, `Report`), `src/agent/fault_probes.rs` (row Q's verifier moves), `test_cli` in `src/main.rs`. **Exits:** against X2's scaffold in CI (`Scaffold` job: `revoke`, `restart`, `partition` pass, `replay` `not_in_build`, the exercised ids checked); the planted policy regression (`requires_values` removed in `--against`) reported by `policy-diff` (CI, and `a_planted_policy_regression_is_reported_by_policy_diff`); a planted revocation that reaches one verifier less fails `revoke` (the KV arm's revocation check removed in a throwaway worktree — the PR records the run); a declaration `wire-check` refuses is refused by `test` with the same text (`a_declaration_wire_check_refuses_is_refused_with_the_same_findings`, and CI diffs the two outputs); an `[[activation]]` whose command would write a file runs nothing (`an_activation_runs_no_command`); `replay` without `sim` reads `not_in_build` and the other classes run (CI builds the binary both ways: `Scaffold` without, `embedded` with). **Decided:** the fleet is one node per unit on a runtime of its own, a cut is the runtime dropped (D6: in-process, one-sided); a declared `[[group]]` is governed to a band of one (floor one, ceiling one), and the re-fill check requires a survivor's own join decision after the cut — the first cut counted a second member that already held the seat, and the governor plant passed through it; `replay` records each node alone — a fleet recorded together on one kernel diverged at its first cross-node channel choice (frame arrival is outside the seams, inventory §3.1.1), so the class holds each node to the shape Z5 proved and says so; `policy-diff`'s decisions are the command's own enforcement point's over the declaration's call set, not a provider's; a revoked mandate reads *not established*, as `assess` says. **Found by the row, and fixed:** a governed group's member that went dark **held the floor** — its `grp/` entry is written once and withdrawn only by a graceful shutdown, so the survivors' governors counted it and never rolled to fill the floor (the co-op fixture's `depots` group: 0 live members, 0 join decisions after 105 s); the governor now counts a member only if it is this node or in its peer table (`membership.governed` revision 2; the class's re-fill check, seen failing first against the fixture). **Found and stated:** a dark node's advertisement leaves `resolve` only after `3 × ttl_secs` and nothing tombstones it at eviction — the scaffold's 30 s is a 90 s blind spot, now measured by the class; the stale `grp/` entry itself stays in the KV until the node returns (the fleet snapshot's group size still counts it). **Found by `replay`, stated and not fixed:** a node with `[persistence]` diverges on replay between its first WAL write and the governor's jitter draw (two seams whose order the schedule decides); the Z5 proof is over a node without persistence (`what-is-proven.md`). **Found by #675's review and fixed:** the `faults/` row was not classified for the raw KV routes (the door disagreed with the front door); the port allocator shared `alloc_port`'s seed and cursor-less sequence, so a faults test bound what a neighbouring test was handed; the re-fill baseline was sampled after the evaporation wait, so the co-op fixture's genuine re-fill read as none (sampled at the cut now; the `faults-group` fixture runs the class in CI); a secure-profile configuration was refused by the command's own rewrite (the profile is not applied and reported); `unknown` left the report's vocabulary (a node built and never started resolves every id now, and *exercised* means the class drove it); the cut dropped the retraction handles before the runtime; a node with no peer counted a fleet of one. **Not built, stated:** a two-sided partition (`--faults two-sided` says so); a load class; consensus, caller-envelope and knowledge-record verifiers (the suite's); a provider-side run of the call set; the regression floor's fixtures under `restart`; withdrawing a dark node's soft state (`grp/`, `cap/`) at eviction, as #627 does at a graceful shutdown — the other readers of `grp/` still count the dark member, and a survivor's eviction becoming a fleet-visible tombstone is a decision of its own. | #675's review, 2026-10-24 |
| X7 | a table, one row per adapter and one column per property, cells carried / not carried / not applicable, a carried cell linking its test; the companion-registered guarantees the generated part; the F2 gate checks every adapter has a row and every carried cell names a test CI ran (`--executed`); one limits section under one name in every companion README, checked by `check-materials.py` | `src/adapter_contract.rs` (`ADAPTERS`, `render_markdown`, `for_runtime`) → `docs/reference/adapter-contract.md` (generated; `the_checked_in_adapter_contract_is_current`); `every_carried_cell_names_a_test_that_exists` (each cell's test a `fn`/`def`/jest title in its file; runtimes unique; guarantee ids the catalogue's); `scripts/check-feature-pages.py` (j) — a row per adapter, a test per carried cell, `--executed` over Rust, pytest and jest keys — with the mutation cases `adapter-row-missing`, `adapter-carried-without-test`, `adapter-test-not-in-file`, `adapter-cell-vocabulary`, `adapter-test-not-executed` (the planted cell naming a test no job ran, in all three languages); `check-materials.py`'s limits-section check and its case `missing-limits-section`; the seven companion READMEs' *What this does not establish* (`mycelium-py`, `mycelium-ts`, `mycelium-agentfacts`, `mycelium-wasm-host`, `strands-session-mycelium` new — the Strands *Claims* section stays; `langgraph-checkpoint-mycelium`, `mycelium-commitment` renamed and extended — the plan's "ten" counted READMEs that do not exist: the other companions are documented on `companions.md`). **Found by #676's review and fixed:** the executed mode never loaded the tracked-file list, so pytest's rootdir-mangled nodeids (`langgraph-checkpoint-mycelium/test_kv_receipt.py::…` for the SDK's tests) matched nothing and the coverage job was red on the first head — fixed, and the mutation suite's Python log line is now the real form, so the planted-cell case fails without the fix; the schema's four new unit fields were `required` under the pinned `/1` — optional now, with the additive policy stated; five carried cells were not shown by the tests they cited (the A2A door's and `mycelium-reason`'s cancellation — no adapter carries cancellation, the legend says so now; reason's identity cell cites `guide_20_routing_policy`; the A2A door's enforcement cell cites the bypass matrix; two over-claiming notes cut); the wasm-host limits section claimed a consensus placement the crate does not have; a runtime unit with a stem section is now a finding. **The cells closed in the adapters, on Richard's word (2026-10-24):** the checkpointer reads its receipts; the Strands bearer is tested; both SDKs gain the grant verbs and the A2A task verbs, and the grants cell was mis-cut (a grant rides as a presented mandate). **Decided, A2A durability:** not built — a task is the door's handle on one call at one node, the work it tracks is not durable either, and durable work is the tuple space's; the cell says *by design*. **Decided:** the native Rust embedding is the eighth row, so a unit without `runtime` has a row to be read against; the "generated part" is the guarantee ids each door resolves (an adapter outside the node registers none), checked against the catalogue. **Cells worth reading:** the checkpointer reads nothing of the receipt its writes return; the Strands store's bearer has no test asserting the principal it names; neither SDK can cancel or carry a grant; an A2A task is lost with its node. **Not built, stated:** the table makes no adapter carry anything; no cell is measured at runtime. | #676's review, 2026-10-24 |
| X8 | an optional `runtime` key (`python` / `typescript` / `strands` / `langgraph`) a unit an SDK agent loads declares; `wire-check` reports per unit which X7 properties the runtime carries; no new engine, no fleet-level file; an unknown runtime a finding by name | `NodeCapabilityConfig::runtime` (`src/capability_config.rs`; empty refused by the loader), `UnitSummary::{runtime, adapter, carries, not_carried}` and the finding `unknown runtime` (`src/wire_check.rs`), the text line per unit; the schema and the golden regenerated; `docs/reference/unit-file.md` § `runtime`; the Python scaffold's unit declares `runtime = "python"`. **Exit:** `a_units_runtime_names_what_it_carries_and_an_unknown_one_is_a_finding` — a native unit and a Strands unit, each with its carried and not-carried properties printed, `cobol` a finding by name. The gateway's `units/declare` accepts and ignores the key. | #676's review, 2026-10-24 |
| X9 | one runnable demonstration in CI that the rows compose: three nodes under the replay seams, one external effect's acknowledgement lost, dependent work on the other two nodes carrying the parent, independent work beside it, one node crashed and restarted from its WAL; the fleet probes before it retries and the destination sees one effect; reads the closure from the three journals naming every dependent and no independent record; resumes, compensates or leaves each member by its class, each as a new record; explains every decision with `explain --closure`, and `test --faults` reports the ids exercised; the same under a partition and under two concurrent retries; two forms — the CI form seamed, recorded into the corpus and replayed with the destination reached zero times in the recovery phases, the live form on the published binary run by the one-hour job; the plants (independent work untouched, a second effect never, a journal withheld said so); a page, a proof-page line, the philosophy's parenthesis | `src/faults/recovery.rs` (the `recovery` class, ten checks; `Lossy`, the mock destination); `the_recovery_class_recovers_the_closure_by_class_and_leaves_independent_work` (`src/faults.rs`) — the live form, every check held; `tests/faults_recovery_replay.rs` (`the_checked_in_recovery_bundle_replays_here_and_matches_a_fresh_recording`: the corpus bundle `tests/replay-corpus/x9-selective-recovery/` replayed against a fresh fleet, `destination_reached_in_recovery == 0`, the checks made on the replay, a fresh recording compared choice for choice); `src/effects/external.rs` `apply_caused` (the parent carried), `leave` (the third verb), `claim_retry` / `claim_probe` / `claim_compensation` / `claim_leave` (two moves racing on one record, one refused — the retry's claim shown under a plant that removed the pre-check); the closure's *INCOMPLETE* and the hole check (`check 3`, with node 2's eviction at node 0); the WAL's replay asserted in check 1; the destination's attempts asserted beside its applications (checks 1, 2, 9); the `Scaffold` job's `recovery: passed`; `one-hour.yml` step 5; `docs/features/selective-recovery.md`, `what-is-proven.md`'s line, `philosophy.md`'s parenthesis. **Decided (2026-10-25, Richard):** the side-effect contract moved into the substrate (`mycelium::effects`, the companion re-exporting it unchanged) because the published binary cannot depend on a companion that depends on it; the ledger re-based on the node-local journal so `rusqlite` stays quarantined. **Found by the run:** a hole hides what hangs below it — with node 2's journal withheld, node 1's dependent two steps down through node 2's record is outside the closure too, and the reader names its parent as a hole; the row's "names every dependent record" holds once the journal is back, and the class says so. **Found by #677's review and fixed:** the ledger's fold skipped a frame it could not decode (an effect re-sent) — refused by name now; a `Failed` record re-prepared lost its history — kept now, as the field's doc says; the check-then-act race the retry's claim closed was open in `probe`, `compensate` and `leave` — each claims under the lock now; a failed compensation stranded its record and a bare `apply` of its id completed it without a permit — the link comes off on failure; a composed compensation was sent unchecked — refused now; the root's one-effect check could not fail (the destination deduplicates) — the attempts are asserted; the default build named the wrong missing feature; `ae.recorded_before_dispatch` resolves `not_applicable` in every fleet the class builds — replaced by `persist.sync_mode`, the permit's ordering noted as the reconciler's own. **Not built, stated:** a real destination; more than one; a closure read at runtime; a policy over the closure's size (§6); a composed compensation; the partition is one node dark, in process (D6); the seamed form records the recovery's decisions, not the fleet's schedule (the nodes' runtimes are outside the kernel, inventory §3.1.1) | #677's review, 2026-10-25 |
| X10 | a watch over `units/` and `skills/` that re-runs `wire-check` and re-declares on a clean check; a finding leaves the last good declaration; skill text written through the route that owns `prompts/`; no restart; shown in CI with the scaffold | `src/declare.rs` (`Watch::tick`, `heartbeat`, `retract`, `run`) and its four tests (`a_unit_edit_is_redeclared_and_the_old_handle_retracted`, `a_unit_wire_check_refuses_keeps_the_previous_declaration`, `a_refused_token_is_an_error_naming_the_route`, `a_skill_file_becomes_a_prompt_template`), written against a `todo!()` tick and seen failing first, plus three from #667's review (a gateway-rejected unit put back and not retried, a lost handle declared again, a removed unit retracted; the skill test gained the removed-file case); the CI step in `Scaffold` (declare, watch, an edit reaching the fleet, a refused unit leaving it, Ctrl-C retracting). **Not built, by the row's own text:** the *Input*-class seam for the filesystem event source — the watch polls on a timer and is outside every recording, stated in the inventory and in the Exit. **Found by the review and fixed:** the exit test asserted before the node's async writes landed; a node restart left the watch blind; the once mode sent a lease it never heartbeated; a poll longer than a third of the lease lost the declaration; the `cli`-only binary did not build; a gateway refusal stormed and lost the previous declaration. | #667's review, 2026-10-23 |
| X11 | the front doors; the concepts chapter; the examples; the first-hour tutorial with its test; the guide and the runbooks; the feature pages; the three audits; the wiki ingest | **(2026-10-25):** the README's *Build a fleet* line routes to tutorial 7 and the feature pages, a compass row *Adopt it without a toolchain* (the hour is not claimed until the job measures it on a release), the docs index's Developer route starts at the published node and `init`, the capability map's rows name their feature pages with a new *Start from the published node* row and a maintainer note naming the registry (`scripts/check-front-doors.py`, `check-materials.py`, `check-feature-pages.py` green); glossary anchors for `correlation`, `parent`, `closure`, `effect-class`, `reconciliation-probe`, `effective-configuration` (the registry's ids make each owning page link them), the `—` Learn columns filled; **tutorial 7** (`docs/guide/tutorials/07-the-first-hour.md`) is the `one-hour` job step for step, `tests/first_hour_tutorial.rs` diffs each tagged fence against the workflow's step with the job's bookkeeping removed (`the_tutorials_blocks_are_the_jobs_steps`); guide 20 § the grant, guide 18 § effect classes, probe, selective recovery (the ledger named as a journal — it said SQLite since X9), guide 19 § reading a run across journals; `production-readiness.md` names per section the guarantee ids `doctor` answers; the SDK READMEs start from the published node and the scaffold with the checkout as the fallback; every row X0–X10 has a feature page in the registry. **Stated, not built:** the five steps are unchanged — they are the toolchain path and the no-toolchain path is tutorial 7 beside them; the `declare --watch` scaffold run is the CI step "declare — a unit edit under the watch" (`ci.yml`), not an example with a matrix row; the examples `authority_grant` and `selective_recovery` were never example binaries — their rows delivered the `grant` and `recovery` classes against the scaffolds, and the examples page says so rather than adding matrix rows (F2's P2: a feature recorded `example: none` gains no row); the three audits' findings and the independent-reader exit are recorded in this PR's thread. | #681's review, 2026-10-25 |
| Y7 | a federation guarantee family `fed.*` resolved by the same report; `secure-multi-domain` requiring `secure-single-domain`'s twenty and the federation ones; on this node `fed.pinned`, `fed.body_bound`, `fed.partner_slots`, `fed.caller_named`, `fed.catalogue_signed`, `fed.partner_egress`; the partner's posture, non-merger and clock agreement `not_verifiable_here`, never counted; rev 1; refusal by name for each of the six; the two-mesh Docker suite under the profile; `doctor` prints the family; the catalogue golden regenerated; `deprecations.md` gains the entry | **Closed (2026-10-27, decided by the maintainer).** Its last promise, *the two-mesh Docker suite under the profile*, is delivered by two PRs: #691 runs every mesh node of both domains under `secure-multi-domain` in `make test-federation` (CI job `federation`; leg 0 reads each node's own `GET /gateway/guarantees` for the profile met), and #695 runs the side #691's consumer could not — each gateway of the two-domain scaffold holds a client, and its own running `GET /gateway/guarantees` reads `fed.pinned`, `fed.caller_named` and `fed.partner_egress` `enforced` with the other four `fed.*` (CI job `Scaffold`, its two-domain step; row Y8). **(2026-10-26):** the family is `core_guarantees()` in `src/agent/guarantee.rs` (subsystem `federation`), the role facts `fed_edge_role` / `fed_client_role` / `fed_role` reading the attached `FederationEdge` and `FederationClient`s; the six named, and a seventh this row adds — `fed.domain_enforced` (`domain_profile = enforced`: TLS, SWIM off; a node fact every member holds, so it applies with no edge or client) — agreed with the maintainer 2026-10-26 (D25, rev 0.13), because SWIM's unauthenticated datagrams at a federation boundary are the one federation posture a configuration alone decides; the three externals `fed.partner_posture`, `fed.non_merger`, `fed.clock_agreement` (`Ext`, never counted; `GuaranteeReport::unresolved` now leaves out one that does not apply); `SECURE_MULTI_DOMAIN` rev 1 = rev 5's twenty + the seven, pinned by `the_multi_domain_profile_is_the_single_domain_set_and_the_family`; the refusal shown for each of the seven by `multi_domain_refuses_naming_fed_*` (one setting dropped from a node holding every other; `catalogue_signed` and `partner_egress` each for both of their settings), with the contrast `multi_domain_with_every_federation_setting_names_no_fed_id`; seen failing first under four plants — every `fed.*` resolver answering `enforced` (7 of 7 fail), the family out of the required set (7 of 7 and the pin), the client's partner-key check removed and the endpoint check removed (each failing its own test); `doctor` prints the family: `doctor_prints_the_federation_family_under_the_multi_domain_profile`; the catalogue regenerated (`docs/reference/guarantee-catalogue.{md,json}`); `deprecations.md` §66; the feature page `docs/features/multi-domain-profile.md`. Also found and fixed: `with_federation_clients` after `start()` was not counted as a late attachment (`federation_clients_attached_after_start_are_counted_as_late`). **The two-mesh Docker suite under the profile (the promise the row stayed open on; delivered 2026-10-27 for both meshes' nodes by the Y8-track precursor PR, #691 — whether that closes the row is the maintainer's call, given the carry below):** *"the two-mesh Docker suite under the profile"* — `make test-federation`, CI job `federation` (`cluster-suites.yml`), every mesh node of both domains started under `secure-multi-domain` rev 1 (`examples/federation_node.rs`, `FED_PROFILE`): certificates issued off-node by a one-shot `pki` service per domain (`mycelium::issue_node_cert`; the CA key in a volume no node mounts, one certificate volume per node), HTTPS gateways closed by a token and pinned by the consumer on their SPKI, identity proofs, an egress allow-list naming the partner's endpoint hosts (beta: alpha's two gateway addresses; alpha, which calls no partner: `egress.invalid`, admitting nothing), persistence (`flush`), `consensus_require_electorate`, an action evaluator at the gateway and the provider with its evidence journal, an execution authority with durable epochs, edges that bind bodies and meter partners; the image built with `compliance` (the audit chain). Leg 0 reads each node's own `GET /gateway/guarantees` (`profile_met`: rev 1, selected, all twenty-seven enforced or not applicable) and each gateway's edge facts enforced, searches each of the six nodes' filesystems and finds only its own key (no CA key, no other node's), and shows the partner side cannot reach the admin surface (404 on the gateway listener; the admin port bound to the mesh address); leg 1 shows two doors refusing what the catalogue granted — the evaluator (`demo/ledger`, granted, not permitted) and the edge's body binding (a credential with no digest, `fed.body_bound`); leg 2 an exclusive outcome refused outside an electorate on each mesh; leg 10 a node with its CA on-node refused at start naming `id.ca_key_off_node` and no other requirement (exit 3); the earlier legs unchanged and passing. Seen failing first: without `compliance` every node refused naming `audit.chain`; with the evaluator naming the gateway instead of the provider, the four calls that cross failed while every other check passed; `profile_met` rejects a planted report with one requirement `not_configured`, `id.ca_key_off_node` `not_applicable`, revision 2, a required entry missing, or `started: false`; on #691's first head's image a node could read three nodes' keys and `/fed-admin/tables` answered 200 on the gateway port from another container (the two checks the review asked for, seen failing there). The evaluator denial leg fails with `FED_PERMIT_EXPORTS` removed (64 passed, 1 failed, that check); the body-binding leg's plant cannot reach it in this fleet — an edge with `require_body_binding = false` is refused at start under the profile naming `fed.body_bound` — so the leg shows the door refusing from outside, and `call.rs`'s binding tests cover the refusal in-process. **Carried to Y8, stated:** the consumer is a bare `FederationClient`, not a node under the profile, so the client side (`fed.pinned`, `fed.caller_named`, `fed.partner_egress` as a node's facts) reads `not_applicable` on all six nodes and is shown by the in-process tests only — Y8 (the two-domain scaffold) is where a node holding clients runs under the profile, the maintainer's decision; **stated, not built:** `fed.partner_slots` is configured and reported but not exercised (the skill answers at once, so filling four slots needs a slow provider); no electorate is declared (the leg shows the refusal outside one, not a decision inside one); the runner reads the gateways with `curl -k` (it is the harness, holding no CA); **stated, not built:** `doctor` reporting the edge's and the clients' facts (they are code attachments a node built from a file never has: `doctor` reads them `not_applicable`, stated on the page). **Independent review (#685), its findings:** (1, the blocker) an edge or clients attached after `start()` bypassed the profile — now **inert** under `secure-multi-domain`, and counted (`federation_attachment_is_inert`; narrowed to that profile in the confirmation round, `…_takes_effect_under_the_single_domain_profile`; `a_federation_attachment_after_start_is_inert_under_the_multi_domain_profile`, the reviewer's sequence, seen failing first; `…_takes_effect_under_dev`); (2) a node facing no partner is said so — `GuaranteeReport::faces_no_partner`, one line in the start log and in `doctor` (no side is required: a member of a federating domain need not face a partner); (5) `fed.caller_named` states that it restates `gw.not_open` for the clients' routes; (6) `a_node_providing_every_requirement_starts_under_the_multi_domain_profile`; (7) the edge and `with_cohort_budget` counted under their own names (`a_cohort_budget_attached_after_start_is_counted_as_late`); the confirmation round on `70efd946` found no blocker; (8) `fed.partner_egress` states that a `.suffix` entry admits every host under it; (4) the seventh guarantee kept — agreed with the maintainer 2026-10-26, D25 updated; (3) decided by the maintainer 2026-10-26: the exit stays as written, and the row **stays open on that one promise only**, to be closed by the Y8-track precursor PR (the two-mesh suite under the profile) — the suite's nodes brought to the single-domain twenty (off-node CA, audit chain, gateway TLS, evaluator, electorate), which Y8 reuses — delivered for both meshes' nodes 2026-10-27 (#691, above), the client side carried to Y8; closing the row is the maintainer's call | — |
| Y4 | an exporter under the existing `otel` feature, from the node as SkillRunner already exports; a span carries correlation, parent, rule id and revision, outcome kind, reason, and the record's journal hash as an opaque reference, never its inputs, subjects, mandate contents or the chain's links; a holder asks the node for the record by hash, under the audit scope, and verifies it there; the guarantee report labels the exporter *not an evidence path*; exit: a golden pinning the span's field set, a test that an input snapshot never reaches the exporter, the report's line | **Delivered (#684, 2026-10-26; the by-hash read route #692, 2026-10-27).** Decided by Richard, 2026-10-26: the route is split out of this row's PR; it serves decision records from the sink or a written trace with a stated *not found* meaning (never existed · evicted · not written) and evidence records from the journal, under `audit:read`, with verification defined. **The follow-up, promise by promise:** *"a holder asks the node for the record by hash"* — `GET /gateway/records/decision/{sha256}` and `GET /gateway/records/evidence/{sha256}` (`src/agent/http/compliance.rs`, `gw_record_decision` / `gw_record_evidence`), asked for by the hash `RecordSpan` carries: `a_decision_record_is_served_by_the_hash_its_span_carries_under_the_audit_scope`, `an_evidence_record_is_served_from_the_journal_and_verified_against_the_chain`; *"under the audit scope"* — both mapped to `audit:read` in `required_scope`, a token without it 403 in both tests; *"from the sink or a written trace"* — the sinks registered with the route (the agent's own as `agent`, any other by `GossipAgent::with_decision_record_source`), read through each sink's own record-index tap — a `TapRing` (`DecisionSink::tap_ring`, lock-order row 74) that keeps the newest copies, the oldest let go, by count and byte estimate — into a bounded index (`src/agent/record_index.rs`, 4096 records / 4 MiB, row 73), so the route never takes a sink's ring lock and the newest records are there however long since the last read — `by_hash_reads_under_load_never_make_a_decision_point_drop_a_record` (0 contended over 20,000 records), `the_index_keeps_the_newest_records_when_no_one_reads_between_them` (5000 decisions, no read, the 5000th served), `each_tap_slot_has_its_own_reader_and_the_ring_keeps_the_newest` (`mycelium-core`), `a_second_sink_is_searched_only_once_registered_and_what_it_kept_before_is_missed` (labels unique, a slot for the agent's life); then the trace the embedder declares (`GossipAgent::with_decision_trace_file`, read by `closure::find_in_trace` under `Corpus::add_trace`'s rule — a `\r` stripped only before a `\n`, as `str::lines` does — each line bounded at 1 MiB): `a_trace_only_node_serves_from_the_trace_and_never_says_never_existed`, `find_in_trace_agrees_with_add_trace_and_bounds_each_line`; *"a stated not found meaning (never existed · evicted · not written)"* — `not_found` is the one meaning left or `undetermined` with `cannot_rule_out` and `unsearched`: a decision is **always** `undetermined` (it may sit in a sink the route was never given, `not_searched`), with `not_written` (no sink registered, a registered sink's drops, a declared trace file not written yet) and `evicted` (no sink registered, the index's evictions, a registered sink's records the index never received) where the counters cannot rule them out — `not_found_names_what_the_counters_cannot_rule_out`; evidence is `never_existed` only for the journal as this node opened it, `undetermined` with `evicted` when it was truncated at this open, `not_written` with no journal — `a_journal_repaired_at_open_cannot_rule_out_an_evicted_record`, the scope stated in the answer and `audit.md` §3a; *"evidence records from the journal"* — paged off the async workers by `read_evidence_journal_from`; *"and verifies it there", "with verification defined"* — the answer carries the exact bytes (`record_b64`), SHA-256 over which is the span's hash (recomputed by each test from the bytes, never read from the answer), and for evidence the reference record in this node's signed, hash-linked audit stream citing it as `journal_sha256` — one in the verified segment preferred over one that is not (the evidence test plants an unverified citing record sealed first) — with its `seq`, `linked`, `citing_records` and its content hash as `GET /gateway/audit` lists it; a decision record is `not_chained`; `audit.md` §3a states both checks; a hash that is not 64 lowercase hex is 400 at both routes (`a_record_is_asked_for_by_its_full_lowercase_hash_only`). **Fail-first, by plant:** the first PR's five (the not-searched count forced to 0, drops ignored, the chain lookup never referenced, the journal's repair ignored, the records hashed as pretty JSON); #692's review's — `never_existed` alone from a registered sink, a missing trace file read as empty, `missed` forced to 0, the index reading the ring (`snapshot`) under load, the first citing record taken, a non-object line counted, the line bound removed, the two tap slots sharing one — each failed its test; its confirmation round's — the oldest copies kept (the 5000-decision test also failed on the round's head), the ring's byte bound removed, a final bare `\r` stripped, a duplicate label admitted — each failed its test. **Not built:** an SDK verb (no audit route has one); `never_existed` for any decision (not knowable: a sink the route was never given); a repair at an earlier open of the evidence journal (not persisted; the answer is scoped to this open); `mycelium-stem` registering its provisioner's sink (a stem built with `compliance` can call `with_decision_record_source`; the binary does not). `src/span_export.rs` — `RecordSpan` (`from_decision`, `from_evidence`; the type has no field for an input, target, effect, subject, resource, arguments, checked constraints, prose reason, mandate digest or chain link), `SpanCollector` (the decision sink through its tap, never drained, its lock never taken; the evidence journal by cursor), `span_export::otlp` (`emit`, `export`, `provider`) under `otel`, through an OpenTelemetry `TracerProvider` — a batch processor on its own thread, where `src/bin/skillrunner/audit.rs` uses the simple exporter (a follow-up: the skill runner's export blocks its caller the same way); trace id from the correlation, span id from the record's hash, the parent span the record's parent within one run, the record's own time — no clock, no draw. **Golden:** `the_attribute_set_is_pinned` (`ATTRIBUTE_KEYS`, `SPAN_SCHEMA = mycelium.span/1`). **The input never reaches the exporter:** `a_span_carries_the_identity_and_outcome_and_nothing_of_the_inputs` (twenty-seven sentinels, one per kept-off part, on a fixture asserted to carry every field) and `what_reaches_the_exporter_is_the_pinned_set_and_no_input` (a real `TracerProvider`, a capturing exporter) — fail-first: a planted input value in a decision span's reason and, separately, the evidence record's prose reason carried, each failed both tests and the golden by name. **The report's line:** `trace.spans_not_evidence` (core, rev 1; `enforced` in an `otel` build, by the type; `not_in_build` without), the catalogue regenerated; `otel` in the report's features. CI: `cargo test --lib --features otel,tls span_export` and the `otel` clippy now with `--tests,tls` (`ci.yml`, `make check-full`). **#684's review, fixed:** the collector reads the sink through a tap (`DecisionSink::tap`, a bounded channel fed after the ring's lock is released) and never takes the ring's lock — `a_collector_never_makes_a_decision_point_drop_a_record`, 0 contended drops over 20,000 records where `snapshot` gave 1,250; `otlp::provider` runs a batch processor on its own thread with a bounded export timeout — `exporting_to_an_unreachable_collector_never_stalls_the_caller` on a current-thread runtime against a refused port; a full sink's drops reported per `collect` (`a_full_sink_is_reported_not_silent`) and the 4096 limit stated; the guarantee scoped to spans `span_export` builds, the skill runner's named outside it, the correlation stated as the caller's; the sentinel fixture covers `route`, `history`, `origin_domain`, `supersedes`, `effect_durability`. **Its confirmation round, fixed:** the tap's send is a plain `try_send`, not a seam choice (one baseline site, inventory §2.3), so a recording replays identically with or without a collector — `a_tap_records_nothing_so_a_recording_replays_with_or_without_one` (`mycelium-core`, under `sim`); every drop reason reported per `collect` (`Collected::dropped`, `DropCounts`) and the bound stated by bytes and records — `every_drop_reason_is_reported_under_the_default_bounds`; a dropped collector frees the tap and a closed tap costs no copy (`RwLock`, lock-order row 72) — `a_dropped_collector_frees_the_tap_and_costs_nothing`; `AlreadyTapped` is an `Error` and guide 19's snippet compiles as a test kept identical to the guide (`the_guide_19_snippet_is_this_code`); the tap's capacity clamped to `MAX_TAP_CAPACITY`; `shutdown`'s full-queue limit and the uncounted processor drops stated; the cursor's repair limit stated. `docs/features/span-export.md`, the glossary's `span-export`, guide 19 § sending the records to a collector. **Read by D17 narrowly:** an evidence record's `reason` is the evaluator's prose and may name a subject, so its span carries no reason; *rule* is the enforcement point and *revision* the policy revision. **Not built, stated:** an export loop or endpoint setting in the `mycelium` binary (the embedder drives the collector); a durable collector cursor; the OTLP client is not the node's egress-gated one; no span store is read by anything in the repository (the row's own *not built*) | #684's review and its confirmation round, 2026-10-26 (1–5 and N1–N6 fixed, 6–10 answered) |
| Y1 | a selector over the knowledge layer and the wiki — **the wiki half not built** (below) — returning records for a run, ranked by capability and task, each with its verdict, standing and issuer intact; a summary a selection, not a synthesis; compression only within one verdict class, never across `InsufficientEvidence` and `Rejected`, never folding a `Retracted` or `BasisWithdrawn` record into live ones; a condensed text written back as a new record with its own issuer, linked by the existing link kinds to every record it drew on; the selection scoped by the caller's mandate (an agent cannot read into context what it could not read directly); exit: a condensed record refused without an issuer, refused citing across a verdict boundary, a mandate-scoped selection excluding an uncovered record shown by a plant; no vector store, ranking the adopter's | **The wiki half is not built:** its pages carry no issuer, verdict or standing, so a page is selected by filing it as a record; only the knowledge-layer half is delivered. `src/knowledge/selection.rs`: `Selector::select` returns `Selected` (the record unchanged, `attribution`, `standing`, `verdict`, `class`), the verdict computed over the admitted records whose standing is current (`a_verdict_is_computed_over_what_the_scope_can_read`, `a_withdrawn_record_never_counts_toward_a_verdict`, `an_unchecked_record_never_counts_toward_a_verdict_under_verified_only`), a record derived from an unadmitted one withheld (`verified_only_withholds_a_record_derived_from_an_unchecked_one`), expiry carried down `DerivedFrom` (`a_derived_record_expires_with_its_oldest_source`), verified records only by default, ranked by the adopter's closure over `SelectionRequest { capability, task, limit }` — `ranking_is_the_adopters_and_the_order_is_deterministic`, `each_selected_record_keeps_its_issuer_attribution_standing_and_verdict`; one class per record, standing before verdict (`VerdictClass::of`), `Selection::by_class`; `write_condensed` refuses an unattributed condensation (`a_condensed_record_is_refused_without_its_issuers_signature`), one across classes (`a_condensed_record_is_refused_across_a_verdict_boundary` — Rejected with InsufficientEvidence, a Retracted and a BasisWithdrawn record with a live one; `the_class_is_re_derived_at_write_time`), one filed under another subject than its sources' — one subject per condensation, so its class is its sources' at every read (`a_condensation_is_refused_unless_its_own_class_is_its_sources_class`, `a_condensation_is_filed_under_its_sources_subject`), one dated ahead of the drift bound (`a_future_dated_condensation_is_refused`), one over expired sources (`a_condensation_over_expired_records_is_refused`), one under a revoked key (`a_condensation_signed_under_a_revoked_key_is_refused`), a held one re-presented unsigned (`a_held_condensation_presented_unsigned_is_refused`), and one not a claim, out of scope, not `DerivedFrom` or citing an unreadable source (`a_condensation_is_a_claim_in_scope_derived_only_from_readable_sources`); the condensation links by `DerivedFrom` to every source, so a retraction reaches it (`a_retracted_source_reaches_the_condensation_as_basis_withdrawn`); `ReadScope::from_mandate` (enumerates `knowledge.read`, a plain-path scope, its window kept and re-checked at every use — `a_selection_past_the_mandates_window_reads_nothing`, `a_dot_segment_is_never_covered`) and `ScopedView`, the direct read the selector reads through, withholding a record derived from an unreadable or absent one (`a_scoped_selection_excludes_what_the_mandate_does_not_cover_and_what_derives_from_it`). **Fail-first, by plant:** an unsigned write-back (verification removed, `put` for `put_signed`), the class check removed, the scope's coverage filter removed and the derivation rule removed — each failed its test; the review's seven regression tests each failed on the first commit, and the confirmation round's six on the second. **Not built:** the wiki half; a condensation across subjects (a summary over several subjects is a selection); a read mandate's epoch and revocation standing; re-verification against the current key view; scoping `KnowledgeStore`'s own methods (no route or SDK verb reads records). No vector store, by design | the independent review, three rounds on #683 (blocker, blocker, no blocker; each answered in the PR) |
| Y3 | the existing Ops Console extended, not a fourth page; a page in the browser against the nodes the operator points it at, reading only their existing routes — beside `/stats`, `/gateway/fleet`, `/gateway/diagnose`, `/gateway/audit` and `/metrics`, the guarantees report, the tuple overview and the governor snapshot; fan-out the browser's, the page merging the answers; the closure the offline reader, not a panel; an opt-in CORS setting, off by default, an origin allow-list, read-only routes only; the `:8099` server and `/api?host=…&path=…` proxy removed, the page a static file; exit: the console runs in CI against a three-node fleet and renders each panel from a route named in its README; a test asserts the example opens no listener of its own; CORS unset answers a cross-origin read with no `Access-Control-Allow-Origin`, set answers only the origins listed, on the read-only routes | **In review (2026-10-27, #693).** *Extended, not a fourth page:* `examples/ops_console/index.html` is the console's page, renamed from `ops_console.html`, its tabs kept and three added. *Reads only existing routes, the three named among them:* the page's `PANELS` — Overview `/stats`, Fleet `/gateway/fleet`, Diagnose `/gateway/diagnose`, **Guarantees `/gateway/guarantees`**, **Governors `/gateway/govern`** (the governor snapshot), **Tuples `/gateway/tuple/overview`**, Integrity, Audit `/gateway/audit`, KV `/gateway/kv/keys`, Metrics `/metrics` — held equal to the README's table and each path to the gateway's cross-origin read list by `the_console_reads_only_the_routes_its_readme_names_and_each_is_a_cross_origin_read`. *Fan-out the browser's, merged on the page:* `refresh` asks every listed node and renders a section per node plus a merged view (Overview's per-node table, Guarantees' guarantee × node matrix, KV's prefix × node counts). *The closure not a panel:* none. *The CORS setting:* `GossipConfig::gateway_cors_origins` (`GOSSIP_GATEWAY_CORS_ORIGINS`), off by default — with it empty the layer is not installed (`src/agent/http/mod.rs`); exact origins only, `config::check_cors_origin` refusing `*`, `null`, a path, query, credentials, uppercase, a default port, and `validate()` a list with no `http_port` (`a_cors_origin_is_an_exact_origin_or_it_is_refused_by_name`, `cors_origins_are_off_by_default_and_parse_from_the_environment`); `src/agent/http/cors.rs` answers only `GET`/`HEAD` on `CORS_READ_PATHS`, a listed origin echoed with `Vary: Origin`, its `GET` preflight answered before `gateway_auth`. *Server and proxy removed:* `examples/ops_console/main.rs` and its `[[example]]` deleted (`deprecations.md` §67). *Exit — CI against three nodes, each panel from its README's route:* the `ops-console` job (`.github/workflows/ci.yml`) builds `examples/observed_fleet.rs` (three nodes, the tuple space mounted, `compliance` + `metrics`) and runs `scripts/ops-console-render.sh`: Chrome renders every tab and `scripts/check-ops-console-render.py` requires each node's section `ok` from exactly the routes the README's row names (cases `tab-<Tab>`), plus `unlisted-origin` (every node `blocked`) and `no-bearer` (every node `unauthorized`). *Exit — no listener of its own:* `the_console_opens_no_listener_of_its_own` (the directory is `README.md` + `index.html`, no `[[example]]` built from it, one `fetch` to a listed node's absolute base, no relative fetch, socket, stream or beacon). *Exit — the CORS answers:* `with_cors_unset_no_answer_carries_an_allow_origin` (every read path, a listed origin and its preflight: no `Access-Control-Allow-Origin`), `with_cors_set_only_listed_origins_read_only_the_read_paths` (each read path echoes a listed origin and answers its `GET` preflight; an unlisted, lookalike or `null` origin, a `POST`/`DELETE`/`PUT`/`PATCH` preflight, a write on a listed path, and nine routes off the list are not answered; no bearer still 401). **Fail-first, by plant:** the path filter removed, a preflight answered for any method, the origin echoed on a write, and the layer installed with an empty list admitting every origin — each failed `with_cors_set_…` or `with_cors_unset_…`; the console tests fail on `main` (the `main.rs` binary, no `PANELS`); the render cases fail with the fleet's origin unset (`blocked`). Every browser showcase and the coop bootstrap list `http://127.0.0.1:8099`. **Not built, stated:** an example, not a product; no authentication of its own beyond the gateway's; the community skill cluster (`skillrunner`) has no setting for the origin, so the page reads it `blocked`; Private Network Access preflights. **#693's review, fixed:** (1) an advertised `ui/viz` is made a link only if `http:`/`https:` (render cases `viz-link-refused`, seen failing first with the check removed, and `viz-link-shown`); (2) the KV value route is a cross-origin read for one `ui/` key only, and what each listed route discloses is in `rbac.md` § Cross-origin reads (`a_listed_origin_reads_only_ui_keys_through_the_kv_value_route`, seen failing with the restriction removed); (3) a gateway with no credential model lists only loopback origins — `start()` refuses any other by name (`an_open_gateway_refuses_an_origin_that_is_not_on_this_host`, seen failing with the refusal removed), the authority sentence corrected in `security.md`, `rbac.md`, `observability.md`, a guarantee line proposed, not added; (4) the page sends its bearer only to typed or confirmed nodes and refuses a node entry with a path or query, `#token=` in history stated; (5) `check_cors_origin` refuses a signed or zero-led port, a non-ASCII host, a non-canonical IP, a comma (seen failing against the earlier parser); (6) `three_node_demo` lists the origin, guides 09 and 02 and the lint command corrected; (7) `HEAD`, a merged companion route and the HTTPS listener in the lib tests, the tuple wait fails when the space never advertises; (8) Private Network Access stated not handled | — |
| Y8 | `init --template two-domains` writes two fleets of two nodes each, from the published binaries (X0), each under `secure-multi-domain` (Y7), with certificates issued off-node by `mycelium tls issue`, named tokens, egress lists naming the partner, a pinned SPKI each way, and one exported capability called across the edge with a credential naming the local caller; the README's last lines are `doctor` on each node and a call refused when the pin is wrong; one domain is the scaffold with the second fleet deleted, and the README says so; exit: the scaffold runs in CI from the published binaries; the cross-edge call succeeds once and is refused under a wrong pin, a replayed body and an exceeded slot cap, each shown; every node's `doctor` is green under the profile; carried from Y7 (#691): a node holding a client shows `fed.pinned`, `fed.caller_named` and `fed.partner_egress` `enforced` in its own `GET /gateway/guarantees` while it runs | **In review (2026-10-27, #695).** `templates/two-domains` (`src/init.rs` registers it; `every_template_file_on_disk_is_embedded`): a food bank (`foodbank-gw`, `foodbank-1`) exporting `pantry/stock`, `pantry/reserve` and a kitchen (`kitchen-gw`, `kitchen-1`) exporting `menu/today`, each node `profile = "secure-multi-domain"`; the operator's steps in `fleet.sh`, the node program in `src/{main,fleet,authority}.rs`, the edge in `federation.toml`. CI: the `Scaffold` job's step *Two-domain template* runs every line below. **Each promise:** *two fleets of two nodes* — four `nodes/*.toml`, `start-all` waits for each node's `ready` (its domain's other node connected, its caller-context marker seen, its domain's exports resolving). *Under `secure-multi-domain`* — every node starts under it (`up as … under secure-multi-domain rev 1`), and each gateway's own `GET /gateway/guarantees` shows the seven `fed.*` enforced and every other requirement enforced or not applicable (the step's Python check). *Certificates issued off-node* — `mycelium tls ca` (new: `mycelium::mint_ca`, `mint_ca_makes_a_ca_that_issues_and_refuses_a_second`, seen failing with the existence check removed) mints each domain's CA under `ca/`, `mycelium tls issue` each node's certificate into `certs/<node>/`, and the step checks no node directory holds the CA key or a second key; `id.ca_key_off_node` enforced on every node. *Named tokens* — `cargo run -- tokens <gateway>` writes each gateway's `GOSSIP_GATEWAY_NAMED_TOKENS` table from `federation.toml`'s `permit`; `gw.token_tables` enforced. *Egress lists naming the partner* — the gateways list the partner gateway's host, the members `egress.invalid`; `fed.partner_egress` enforced on both gateways. *A pinned SPKI each way* — each gateway holds a client for the other pinned to `exchange/<gateway>.pin`, written by `mycelium tls pin` (new; `ca_issue_and_pin_are_the_issuers_three_steps`: the pin is the node key's `ed25519_spki_sha256`, seen failing with the pin taken over the whole certificate). *One export called across the edge with a credential naming the local caller* — each way: `call kitchen-gw pantry/stock` answered `for federation:kitchen.example/token:kitchen/planner`, `call foodbank-gw menu/today` for `…foodbank.example/token:foodbank/steward`; both domains' evaluators decide it (`federation.call` at the caller's gateway, `skill.invoke` at the provider). *The README's last lines* — `doctor` on each node (stopped: the doctor opens the node's evidence journal, which a running node holds) and the wrong pin: `connect` answered `presented an unpinned key`, `sent: false`, and a `call` then `link` down, `sent: false` — the pin refuses the connection a call needs, so it is the connection, not the call, that names the pin; restoring the right pin (`mycelium tls pin` of the gateway's certificate) and the call going through again is shown too. *One domain* — the README's paragraph, followed literally on a fresh project, runs both food-bank nodes under `secure-single-domain` (checked by hand, not in CI). *Every node's `doctor` green* — `cargo run -- doctor <node>` is the new `mycelium::doctor::run_attached` over the node as the program builds it (`the_doctor_over_an_attached_node_reads_its_attachments_and_starts_refusal`, seen failing with the attachment ignored): `start(): would start` on all four; `mycelium doctor nodes/<node>.toml`, the file alone, refuses (the step shows `ae.authorised_at_seam: not_configured`). *Refused under an exceeded slot cap* — three concurrent `call kitchen-gw pantry/reserve` against a cap of two: exactly one `already carrying 2 calls for kitchen.example` (seen passing none with the cap at 10). *Refused under a replayed body* — **delivered as a rewritten body**: `cargo run -- tamper kitchen-gw pantry/stock` presents a credential signed for one body with another, refused `binds a different request body`. **Found, not built here:** the same credential with the same body, presented again inside its window, is served again — the edge keeps no record of credentials it has served (shown by hand: two answers to one presented call); stated in the federation runbook and the README. The replayed body stays *not built* in this row: the maintainer decided 2026-10-27 that the guard is its own PR, **the edge replay-guard PR**. *Carried from Y7* — the kitchen's and the food bank's own `GET /gateway/guarantees` read `fed.pinned`, `fed.caller_named` and `fed.partner_egress` `enforced` while they run. **Not built:** *from the published binaries* — the nodes are the scaffold's own program built from the crate at the pinned tag, because the published node attaches no evaluator, edge or client from configuration; the `mycelium` binary is used for `tls ca`/`issue`/`pin`; and no published tag has `secure-multi-domain` before the next release, so CI builds the scaffold against the checkout (its dependency rewritten to a path, as for the Rust template) and an adopter can build it only from that release on. *Separate hosts* — four processes on one host: `ca/` and `certs/<node>/` show the separation by layout; the host does not enforce it. **#695's review, fixed:** a member loaded its domain's signing seed (now the gateway alone does; an operator key, whose seed no node reads, is the mandate root — CI moves both seeds away and the members still build, the gateway refuses naming the file); every private key was written 0644 — in the library (`generate_and_publish_ca`, `issue_node_cert`, `save_key_raw`; `every_private_key_written_is_owner_only`, a separate commit for the v2.38.2 PATCH) and the scaffold's seeds and tokens (CI checks 600); `mint_ca` was check-then-rename (`concurrent_mints_publish_one_ca_and_refuse_the_rest`, seen failing with the old publish); `tls issue` replaced a key silently (`--force` now, `issuing_over_an_existing_node_key_needs_force`, deprecations § 69); `tls pin` pinned a CA certificate or a key bundle (refused, in `ca_issue_and_pin_are_the_issuers_three_steps`); `hex32` dropped non-hex pairs; `setup` made `secrets/` before building; the local caller read the issuer's `ca/` (now `certs/<gateway>/`); the doctor docs said it attaches nothing. | — |
