# dev/companions — the onboarding checklist

↑ [companions/](companions.md)

What a new companion crate owes before it counts as landed. The v3 contracts axis added several at
once, and each arrived with a *different* subset of these done — which is how the list stopped being
folklore and became a page.

> **A companion missing a row below is a lint finding**, not a style preference. The cost of a
> missing row is always paid later by someone who could not find the thing.

## The rows

| # | Row | Done when | Skippable? |
|---|---|---|---|
| 1 | **A Cargo feature line** | the crate is a workspace member and its optional deps are behind a feature, so a minimal embed does not pay for it | no |
| 2 | **A CI job** | its tests run on every change, scoped with `-p` (a workspace-wide build pulls `wasmtime` via wasm-host) | no |
| 3 | **Docker-suite membership** | *only if it makes a cross-node claim.* An in-process test cannot falsify a claim about two processes | **yes**, with the reason recorded |
| 4 | **A row in [`operations/companions.md`](../../../operations/companions.md)** | an operator can run it: durability posture, failover shape, teardown, what it does *not* emit | **yes**, if it has no production surface — say so explicitly |
| 5 | **A page here** | design, rationale, and the gate that would fail if the claim stopped holding | no |
| 6 | **A gallery entry** | one runnable demonstration that ends by naming what it does **not** establish | no |
| 7 | **An SDK verb** | *only if it has a gateway route.* A route without a verb is a surface only Rust can reach | **yes**, if it has no route |

Rows 3, 4 and 7 are the conditional ones, and the condition must be **written down**. "It has no
operational surface" is a fine answer; leaving the row blank so a reader cannot tell the difference
between *deliberate* and *forgotten* is not.

## Why row 6 is not optional

The axis's own experience: of six decisive demonstrations written for the v2.9.0 gallery, **two found
real defects** that the test suites had not. A revoked wiki curator was being told to re-read and
retry, which refuses forever; and a federated call against a blackholed gateway hung instead of
returning an unknown within a bound. Neither in-process test could have caught its defect — one
because the tests checked the transaction *text*, the other because stopping a gateway produces a
refusal and a refusal fails fast.

A demonstration is a gate, not decoration.

## Why row 4's "no production surface" answer is a real answer

[`mycelium-sim`](../../../../mycelium-sim/) is the case that proves it. The harness routes through a
seam that is **off in every shipped build**; without the `sim` feature the seam's body is the call it
replaced. So it has no operator story at all, and saying that plainly is more useful than a section
explaining how to run something nobody runs in production.

## Applying it — the 2026-09-19 sweep

| Companion | 1 feature | 2 CI | 3 Docker | 4 ops row | 5 page here | 6 demo | 7 SDK verb |
|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|
| `mycelium-tuple-space` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `mycelium-blackboard` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `mycelium-wiki` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `mycelium-reason` | ✅ | ✅ | n/a | ✅ | ✅ | ✅ | ✅ |
| `mycelium-agentfacts` | ✅ | ✅ | ✅ `test-federation` | ⚠️ | ✅ | ✅ coop `federation_facts` | n/a |
| `mycelium-guardrails` | ✅ | ✅ | n/a | ⚠️ | ✅ | ✅ `guardrail_wedge` | n/a |
| `mycelium-wasm-host` | ✅ | ✅ | n/a | ⚠️ | ✅ | ✅ coop `catalog` | n/a |
| `mycelium-commitment` | ✅ | ✅ | n/a | ⚠️ | ⚠️ | ✅ `redistribution_cn` | n/a |
| `mycelium-sim` | ✅ | ✅ | n/a | **n/a, by design** | ⚠️ | ✅ `replay_a_bundle` | n/a |
| `mycelium-effects` | ✅ | ✅ | n/a | ⚠️ | ⚠️ | **⚠️ none anywhere** | n/a |

**⚠️ = open finding. n/a = the condition does not apply, stated rather than left blank.**

Six companions have no row in the operations runbook, three have no maintainer page here, and
**`mycelium-effects` has no runnable demonstration at all** — not in its own crate, not in the root
examples, not in the co-op suite. That last one is the sharpest finding, because row 6 is the row
that has historically caught real defects.

`mycelium-sim`'s row 4 is **closed** the moment its absence is recorded as deliberate rather than
left blank, which is what this table now does. That is the whole mechanism: the checklist does not
demand every row, it demands that every row has an answer.

## The rule this page is really enforcing

A companion is not landed when its tests pass. It is landed when somebody who did not write it can
**find it, run it, operate it, and see what it does not claim**. Rows 4 through 7 are that sentence,
split into checkable parts.
