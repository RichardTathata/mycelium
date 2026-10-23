## [2026-10-23] ingest | adopter-surface X4, first PR: one id and one parent through the records

- Two fields, carried never minted: `params._meta.correlation` (one key segment) and `params._meta.parent` (a
  content hash) enter the `ActionEnvelope` in `ae_decide` — the one preflight every door runs — and ride on every
  evidence record it produces, decision and execution, at the gateway and at the provider. `/mcp` forwards the two
  keys inside the `tools/call` payload (the part the caller envelope's signature digests); the provider's `check`
  reads them there. The placement rule from the plan (rev 0.11): never a header, never the envelope — both counted
  (`causal_misplaced`, `/stats`), never read. Malformed: `PreflightRefusal::Malformed`, `-32602`, before policy.
- The reader: `src/closure.rs`, `mycelium explain --correlation` / `--closure [--fleet]`; a record's identity is the
  sha256 of its bytes as written (journal frame body, trace line), the hash `journal_sha256` cites. Captured /
  reconstructed (by `operation_id`) / unattributable; holes named with the sinks' drop counts, never filled; a
  federation edge a boundary; a cycle terminates (fuzz target `closure_corpus`, in the reachability registry).
- Not built in this PR (stated on the feature page, the plan's §8 and `deprecations.md` §58): the door registry and
  the member-side doors, the SDK verbs, the reason trace join, the three-node plants.
- Pages: `docs/features/causal-records.md` (registry entry, glossary `causal-records`), guide 20's new section,
  `deprecations.md` §58, the plan §8 (and §1.1's first measurement: v2.37.0, 9 s, printed 10 minutes).
