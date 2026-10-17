## [2026-10-17] ingest | concepts pass: fleet growth and one name per concept on the newcomer path

Up: [domain](../domain.md). The newcomer path (README → `docs/guide/00-concepts.md` → `docs/guide/tutorials/`
→ the guide chapters) used fleet-growth and authority terms before defining them. Each is now defined where a
newcomer meets it, taken from the code it cites.

- **`00-concepts.md` § Fleet growth** (new): stem node and stem fleet (`mycelium-wasm-host/src/stem.rs`: a node
  that declares from its unit file; only one with `[hosts]` runs a provisioner and installs), catalogue · library ·
  librarian (`docs/operations/artifacts.md`), unit file, presence band / floor / ceiling (a convergence target, not a
  cardinality guarantee), provisioner vs governor, self-election, runtime kind, activation, `[[serve]]`,
  shadow → accept, `wire-check` (*would bind*, never *is bound*).
- **Authority words:** enforcement point (gateway or provider; covers only the routes it fronts) and evidence
  journal; history scope (row K); credential binding and placeholder (row L). Gateway caller, caller context and
  caller envelope named as one thing (`src/agent/gateway_caller.rs`), distinct from the action envelope.
- **Disambiguation rows** for *claim*, *scope*, *journal*, *catalogue* (code spells `catalog`) and *admission*, in
  the style of the existing *profile* row; the duplicate *Bundle* row merged.
- **Corrections:** `docs/positioning.md` defined a stem as "a node with a `[hosts]` table"; the code's definition is
  wider, and only a stem with `[hosts]` installs. Positioning said an evaluator without a journal "enforces and
  records nothing"; the code (`src/agent/mod.rs`, `with_evidence_journal`) says it still enforces and records
  nothing. The guide index said Python/TypeScript "use a gateway process"; they drive a node through its embedded
  gateway.

Main term chosen: **presence floor** ("provider floor", "desired floor" are aliases). The five-step path keeps
"declared provider floor" (gated text, unchanged).
