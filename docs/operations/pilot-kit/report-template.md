# Pilot report — {{domains}}

*Produced {{date}} by `report.py` from two outputs and nothing else: `mycelium doctor` on each of the scaffold's nodes
and `mycelium test --faults` over the kit's declaration. The states are the guarantee report's own words —
`enforced` · `not_configured` · `not_in_build` · `not_applicable` · `not_verifiable_here` — and each section is one
guarantee id the pilot exercised. Write the pilot's own observations where a section asks for them; change nothing
else, so a reader can regenerate this file from the same two outputs and diff it.*

## The fleet, as `doctor` reads it

{{fleet}}

## The fault classes, as `test --faults` ran them

The classes run on an in-process fleet built from `units/` and `faults.toml`, under the `dev` profile whatever the
scaffold's nodes declare: a profile is `doctor`'s to check, the classes test what holds under fault. So a guarantee can
read one state in a class's fleet and another on the scaffold's node; each section below gives both.

{{classes}}

## Guarantees the pilot exercised

<!-- section -->
### `{{id}}`

**The promise:** {{promise}}

**On the scaffold's nodes (`doctor`):** {{nodes}}

**Under fault (`test --faults`):** {{faults}}

**Who holds it:** {{matrix}}

**Observed in the pilot:** *the counterparty's words — what they saw on their own hosts and network, and whether it
matched the two lines above.*

<!-- /section -->

## Reported by `doctor`, exercised by no class

{{unexercised}}

## What no node can show

`not_verifiable_here` is a node saying the fact is outside what it can observe; each needs the deployment's own
evidence, which this report does not hold.

{{external}}

## What this report is not

It is the kit's output on the pilot's first day, not the deployment: the *named deployment* line on
[what-is-proven.md](https://github.com/RichardTathata/mycelium/blob/main/docs/operations/what-is-proven.md) stays open
until a counterparty runs Mycelium under its own name, and this report is evidence toward that line, not the line.
