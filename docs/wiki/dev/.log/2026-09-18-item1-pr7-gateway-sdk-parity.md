## [2026-09-18] ingest | item 1 PR 7 — gateway/SDK receipt parity

Up: [dev](../dev.md) · record `docs/design/contracts-receipts.md` §5 (landed note) · code
`src/agent/http.rs` (`commit_json`, `commit_error_response`, `committed_bool`), `src/agent/consensus_handle.rs`
(`receipt_from` now `pub(crate)`), `mycelium-core/src/receipt.rs` (`LocalDurability::tag`, `failure_reason`),
`mycelium-py/src/mycelium/agent.py`, `mycelium-ts/src/{types,agent,index}.ts`.

### What landed

The last PR of item 1: the two consensus-backed gateway verbs answer `"local_durability"` beside `"persisted"`,
in the receipt's own four names, and both SDKs read it. The handlers go through the same `receipt_from` as
`cluster_propose_receipt`, so the HTTP answer and the Rust receipt cannot drift; `"persisted"` is read off the
result *before* it becomes a receipt, so the old field is byte-for-byte what it was.

### Three things worth carrying forward

1. **Parity means one mapping, not two that agree today.** The tempting version re-derived `local_durability`
   in the handler from `persisted` and the config — a second copy of `receipt_from`'s one judgement, which would
   drift the day either changed. Making `receipt_from` crate-visible and calling it is the whole design; the
   helpers around it only render.
2. **The vocabulary is a pin, because it is a wire.** `LocalDurability::tag` is four strings that two SDKs
   parse; a rename is a wire change. It is pinned in `mycelium-core`, and the SDK stub tests pin the same four
   from the other side. The live gateway test shows the collapse undone — a node *without* persistence answers
   `persisted: true` **and** `not_configured` — which is the sentence the whole field exists to say.
3. **A test that never runs is not a gate.** `mycelium-py/tests/test_commit_result.py` has been node-free
   since 2026-09-05 and was never in CI's explicit pytest list. Extending it without adding it would have
   shipped parity "tested" by nothing; it is in the list now. Worth a periodic sweep: every SDK test file
   against the CI list.

### Not shown

The `failed` shape live: a stopped WAL writer is not inducible from outside the node. Its rendering
(`"local_durability": "failed"` + `"local_durability_error"`, `"ok": true` because the *value* committed) is
unit-pinned in `commit_json`'s test, not observed end-to-end.

### Pages touched

- [history.md](../history.md) — the item 1 PR 7 section.
- `docs/guide/04-consensus.md`, `docs/operations/deployment.md`, `docs/operations/production-readiness.md` —
  the JSON field beside `persisted`.
- The ADR's §5 carries a dated landed note; `persisted`'s `#[deprecated]` waits for the 2.8.0 cut.
