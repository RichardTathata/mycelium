# 3 · A capability earns permission to run

↑ [Tutorials](README.md) · Next: [Authority boundaries](04-authority-boundaries.md)

## Objective

Separate publisher trust from reviewer acceptance. A proposed artifact may be
available for evaluation without becoming the live provider customers discover.

**Terms.** A *proposed* entry (`proposed = true`) loads only under its
[shadow name](../00-concepts.md#shadow), `{ns}/{name}.shadow`, which can be called by name but takes no
demand. A *reviewer* is a key in the host's `[hosts].trusted_reviewers`; its co-signature
(`mycelium-artifact accept`) is what lets the entry load under the live name.

## How to run

Prerequisites: Rust and loopback sockets; no Docker or model. From the repository root:

```sh
cargo run -p mycelium-coop-examples --features wasm --bin provisioning
```

The first waves install and recover a provider. Follow the final shadow/acceptance
wave; expect `shadow-then-accept complete` and `All assertions passed`.

## What it demonstrates

Read the final wave of [provisioning.rs](../../../examples/coop/src/bin/provisioning.rs),
then the [artifact lifecycle](../../operations/artifacts.md):

| Stage | Evidence to inspect | What it does not establish |
|---|---|---|
| Proposed entry | Publisher signature and proposed status | Approval for the live name |
| Shadow install | Invocation under the shadow name | Domain correctness or production acceptance |
| Reviewer acceptance | Signed acceptance bound to the entry | Permission to run every future revision |
| Live install | Discovery and invocation under the live capability | Authority for every downstream effect |

## Try a change, then a refusal

The unmodified run already publishes an acceptance signed by a stranger and
asserts that the [incumbent](../00-concepts.md#incumbent) — the provider already live under
`route/optimize` before the proposal — remains the only live provider. This is the first
negative case to inspect; a signature alone does not establish reviewer authority.

Locate the proposed artifact and acceptance in the final wave. Before the acceptance
step, inspect the existing assertion that it is absent from the live name; the
shadow name remains the evaluation route. Temporarily omit acceptance and rerun.
The final live-provider assertion must fail. Restore acceptance afterwards.
Next change the payload used in the shadow call and its expected result together;
this makes evaluation evidence about the executable explicit.

If shadow installation itself fails, check publisher trust and runtime eligibility.
If shadow works but live discovery fails, check proposed status and reviewer
acceptance before changing capability routing.

## Dev notes

This walkthrough reuses the CI-executed provisioning scenario rather than inventing
a second acceptance protocol. Its reviewer and acceptance are demo decisions;
production owns the review criteria and signing keys. The shadow run does not
certify model quality or make arbitrary host activation commands safe.
The binary cleans up its agents on normal completion; no external model is started.
For an application, define measurable acceptance criteria before automating review.
