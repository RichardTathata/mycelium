# The adapter contract

## What it is

The [adapter contract](../guide/00-concepts.md#adapter-contract) is one table, `docs/reference/adapter-contract.md`:
a row per runtime adapter — the native Rust embedding, `mycelium-py`, `mycelium-ts`, `langgraph-checkpoint-mycelium`,
`strands-session-mycelium`, `mycelium-reason`, the A2A door, the MCP door — and a column per property: session
persistence, cancellation, tracing ([the causal records](../guide/00-concepts.md#causal-records)), identity
propagation, [grants](../guide/00-concepts.md#grant), provider-side enforcement, receipts. A cell is *carried* and
names the test CI runs that shows it, *not carried* with the reason and the adapter's *What this does not establish*
section, or *not applicable*. A [unit file](../guide/00-concepts.md#unit-file)'s `runtime` key names a row, and
`wire-check` prints, per unit, what that runtime carries, so a fleet of mixed runtimes says what it enforces where.

## When to use it, and when not

- **Use it when** choosing how an agent reaches the fleet, or reading a declaration that mixes runtimes: the cell
  says whether the property survives that adapter, and the test says how that is known.
- **Not when** you need the adapter to carry something it does not: the table records; it makes no adapter carry
  anything, and a *not carried* cell is a fact about today's code, with the companion's own limits section saying
  what stands behind it.

## Turn it on

This feature is turned on in code.

- [`ADAPTERS`](../../src/adapter_contract.rs) — the rows: each adapter's cells, the guarantee ids its door resolves,
  and its limits section; [`render_markdown`](../../src/adapter_contract.rs) writes the reference page from them.
- [`for_runtime`](../../src/adapter_contract.rs) — the row a unit's [`runtime`](../reference/unit-file.md#runtime)
  names, which `wire-check` reads to fill each unit's `carries` and `not_carried` and to raise the finding
  `unknown runtime`.
- `the_checked_in_adapter_contract_is_current`, in [`src/adapter_contract.rs`](../../src/adapter_contract.rs): the
  page is the code's rendering byte for byte (regenerate with `UPDATE_ADAPTER_CONTRACT=1 cargo test --lib
  adapter_contract`).

## See it work

No runnable example exists: the [example index's capability matrix](../../examples/README.md#the-capability-matrix)
has no row for this feature. The Python scaffold's unit declares `runtime = "python"`, and CI's `Scaffold` job runs
`wire-check` over it in the Python template step, asserting the unit's carried line. The tests are in [`src/adapter_contract.rs`](../../src/adapter_contract.rs):
`the_checked_in_adapter_contract_is_current` and `every_carried_cell_names_a_test_that_exists` (every carried cell's
test is a `fn`, a `def` or a jest title in the file it links; every runtime named once; every guarantee id the
catalogue's); and in [`src/wire_check.rs`](../../src/wire_check.rs):
`a_units_runtime_names_what_it_carries_and_an_unknown_one_is_a_finding` (a native unit and a Strands unit, each
with its carried and not-carried properties printed; `cobol` a finding by name). The gate
`scripts/check-feature-pages.py` holds the page to the code — a row per adapter, a test per carried cell, and in
CI's `--executed` mode every such test executed in the run, in its own target — and `scripts/check-materials.py`
holds every companion README to its limits section.

## What it guarantees, and what it does not

- It adds no guarantee. The doors' rows name the ids their doors resolve — the MCP door and the native embedding
  [`prov.enforcement`](../reference/guarantee-catalogue.md), the A2A door [`a2a.admission`](../reference/guarantee-catalogue.md),
  all three and `mycelium-reason` [`ae.authorised_at_seam`](../reference/guarantee-catalogue.md) and
  [`ae.recorded_before_dispatch`](../reference/guarantee-catalogue.md); an adapter outside the node registers none,
  and what it carries is what the doors it calls carry.
- A *carried* cell is a test CI ran, in the target its file belongs to; a cell with no test is refused by the gate,
  and a planted cell naming a test that did not run fails CI's `test-coverage` job.
- A *not carried* cell is a limit of today's code, stated in the adapter's *What this does not establish* section
  — one section under one name in every companion README (`check-materials.py`). The ones the plan's rows own:
  the checkpointer's and the Strands store's tracing fields; cancellation that reaches the node from an SDK (the SDKs'
  grant verbs, the pending-grants stream and the A2A `input-required` state are carried since X3's second and third PRs).
- **What the doors do not establish** (the limits section the door rows cite): a task accepted at `/a2a` lives in
  that node and is lost with it, **by design** — the work it tracks is not durable either, and durable work is the
  tuple space's and the commitment net's; `tasks/cancel` ends a task's record and stops no work, and `tools/call`
  has no cancel — no adapter carries cancellation today, and running work stops only when its authority lapses; a
  tool result and a task result are not receipts; provider-side enforcement needs an evaluator attached at the
  provider, and fails closed without one.
- **Upgrade note:** a unit carrying `runtime` is refused by every reader older than this release — a 2.37.0
  gateway answers `400 invalid unit file`, a 2.37.0 stem or `wire-check` refuses the directory (unknown keys are
  refused by name since 2.35.0) — so declare it only once every gateway and stem reading the directory runs this
  release ([deprecations § 61](../guide/deprecations.md#61-a-units-runtime-key-is-refused-by-readers-before-this-release-unreleased-adopter-surface-x8)).
- The `runtime` key is a declaration of what will run the unit, read by `wire-check` only; the gateway accepts and
  ignores it, and no engine is selected by it (D4: a directory of unit files is the fleet file).
- **Not built, stated:** the table does not make an adapter carry anything; no cell is measured at runtime — the
  plan rows are [`adopter-surface.md`](../plans/adopter-surface.md) X7 and X8.

## Go deeper

- Reference: [the adapter contract](../reference/adapter-contract.md), [the unit file § `runtime`](../reference/unit-file.md#runtime).
- Guide: [language bridges](../guide/10-language-bridges.md) (the SDKs), [authorising actions](../guide/20-authorising-actions.md) (the doors).
- Operations: [companions](../operations/companions.md).
- No design record; the plan rows are the record.
- Code: [`src/adapter_contract.rs`](../../src/adapter_contract.rs), [`src/wire_check.rs`](../../src/wire_check.rs),
  [`src/capability_config.rs`](../../src/capability_config.rs).
