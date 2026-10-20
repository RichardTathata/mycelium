# <Feature name>

<!-- A feature page links to its sources and copies nothing from them. See README.md in this directory. -->

## What it is

<!-- Two or three plain sentences. Link the concept's entries (one or more) in ../guide/00-concepts.md. -->

## When to use it, and when not

- **Use it when** <!-- one line -->
- **Not when** <!-- one line -->

## Turn it on

<!-- The settings, each linked to ../reference/configuration.md or ../reference/unit-file.md, and a link to a
configuration or unit file that a test loads. No snippet of the page's own.
For a feature enabled in code, open with "This feature is turned on in code", then one item per line: its name in
backticks linked to the file that defines it, the last path segment a fn, struct, enum, trait or const there. A
configuration field is its own line, `name` in backticks linked to ../reference/configuration.md#the-fields. End
with the test that runs that code: its name in backticks, a `fn <name>` in the linked file, listed by CI's test
universe and matched by no entry of scripts/test-coverage-exceptions.txt; say so if it runs a hand-kept copy
rather than reading the page. -->

## See it work

<!-- A link to the feature's row in ../../examples/README.md; the index keeps the run command and the CI status.
If there is no example, open with "No runnable example exists" and link the test that runs the feature end to
end: its name in backticks, a `fn <name>` in the linked file, listed by CI's test universe and matched by no entry
of scripts/test-coverage-exceptions.txt. The registry records `example: none`. -->

## What it guarantees, and what it does not

<!-- Guarantee and rule ids, each linked to where it is catalogued (a companion-registered id, such as stem.*, to
where its states are described); the plan rows' *not built* items; the ../operations/what-is-proven.md ledger.
Without a per-entry anchor, link the table and name the row beside the link. One short clause per headline limit,
linked to where it is stated in full. Evidence status stays in the ledger. -->

## Go deeper

<!-- Guide chapter, operations runbook, design record, code. An empty slot says so: "no design record; the plan
row is the record". -->
