# Pilot report — {{domains}}

*Produced {{date}} by `report.py` from two outputs and nothing else: `mycelium doctor` on each of the scaffold's nodes
and `mycelium test --faults` over the kit's declaration. The states are the guarantee report's own words —
`enforced` · `not_configured` · `not_in_build` · `not_applicable` · `not_verifiable_here`. Write the pilot's own
observations where a section asks for them and change nothing else, so a reader can regenerate this file from the same
two outputs and diff it.*

## The fleet, as `doctor` reads it

{{fleet}}

- **Domain** is each node's `cluster_name`, from the effective configuration `doctor` prints. In the scaffold it names
  the domain; a deployment that sets it otherwise reads that here.
- **Build features** are the features of the program `doctor` ran in. That is the scaffold's node program
  (`bash fleet.sh doctor` builds the node as the program does, with its attachments), compiled with the features its
  `Cargo.toml` names, not the `mycelium` binary's.
- **A node whose output was not given is not in this report.** It covers the {{node_count}} given.

## The fault classes, as `test --faults` ran them

The classes run on an in-process fleet built from `units/` and `faults.toml`, under the `dev` profile whatever the
scaffold's nodes declare. It has no gateway port and no federation edge: a profile is `doctor`'s to check, and the
classes test what holds under fault. **The cross-edge call the kit makes (its step 3) feeds nothing into this report**,
so no `fed.*` guarantee is evidenced by it. Those read below only as `doctor` reports them.

{{classes}}

## Guarantees exercised where the class's fleet enforced them

Each section is one guarantee a class drove while its own fleet resolved it `enforced`: the class's checks are
evidence for it.

{{enforced_sections}}

## Guarantees a class touched where its fleet did not enforce them

A class lists these as exercised, but its own fleet resolved each as `not_applicable` or `not_configured` (its fleet
runs under `dev`, without a gateway). The class passing is not evidence for the guarantee as configured; the state on
the scaffold's nodes is `doctor`'s.

{{other_sections}}

<!-- section -->
### `{{id}}`

**The promise:** {{promise}}

**On the scaffold's nodes (`doctor`):** {{nodes}}

**Under fault (`test --faults`):** {{faults}}

**Who holds it** (the [matrix row](https://github.com/RichardTathata/mycelium/blob/main/docs/operations/shared-responsibility-matrix.md#from-a-doctor-line-to-its-row) `doctor` names): {{matrix}}

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
