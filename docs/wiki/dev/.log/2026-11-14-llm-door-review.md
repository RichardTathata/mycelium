## [2026-11-14] ingest | #785's review: what surrounds the `llm.invoke` provider check

- **"Five receivers call `check`" is not "every receiver".** Three more take a kind off the mesh with no decision,
  each now a dated *not built* row on `docs/operations/what-is-proven.md`: a hosted capability's
  `cap.invoke/{ns}/{name}` (through `rpc_rx`, where the check is inert because the kind is not protected — open
  today; **decided 2026-11-14: protected at an enforcing provider, in its own PR in wave 1b**; row I5 already calls
  the family an action for `ot`); `bulk_serve` (no caller context, no
  check); a raw `signal_rx` + `RpcRequest::from` receiver (no public way to run the check). The 2026-11-13 log's
  "five receivers … each calls `check`" is true of those five and says nothing of these three.
- **`[authority]` qualifies a bare `prompt:<ns>/<name>` to its node** (`src/agent/declared_authority.rs`), as it
  did `tool:` and `skill:`: both LLM doors present `prompt:<ns>/<name>@<node>`, and without it the easy rule was `*`.
- **A stopped call is 409 at the gateway**, not 403: `data.reason: cancelled` is a stop after admission. The
  provider's caller-context refusal at the LLM door carries no `data` and stays 502.
- **The LLM door's waiting checks are unbounded**: it spawns a task per call and awaits a `Waiting` check there; the
  other four doors hold at most `MAX_DEFERRED_CHECKS`. Stated, not built: bringing it under `DeferredChecks` means
  moving the check into the dispatch loop.
- **`mycelium-reason`'s router retries any error reply on the next provider**, so once a gateway mints, a cancelled
  call would be re-sent under a new term: an entry condition for I4's PR 2.
- **CI:** the `compliance,llm llm_door_tests` step is strict (`CI_RETEST_STRICT=1`), and the pair has a clippy line.

Pages: `docs/wiki/dev/security.md`, `docs/features/provider-enforcement.md`, `docs/features/declared-authority.md`,
`docs/operations/what-is-proven.md`, rule `provider.enforcement` rev 6.
