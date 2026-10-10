## [2026-10-10] ingest | the KV namespace table becomes a gate

- `scripts/check-kv-namespaces.sh` gains a second check: every KV prefix production code uses — slash-bearing
  `const`/`static` `&str` literals, `format!` key heads, the first literal of a KV call (the call set is named in the
  script's header) — across every library crate must reduce to a namespace with a row in `src/lib.rs` § KV namespace
  ownership (top segment; under `sys/`, the second too), or start with an entry of the new
  `scripts/kv-namespaces-nonkeys.txt` (30 entries, each with its reason; a stale or reasonless entry fails).
  `--self-test` plants five rowless literals (const, format, next-line format, call, a removed row) plus a stale and
  a reasonless allow-list entry in a scratch copy and requires each to be named, and a test-module plant not to be;
  both run in `make check` and CI.
- Why: the wiki lint missed live prefixes five times (ledger 2026-07-07 … 2026-10-10). On the unfixed table the gate
  names exactly the 2026-10-10 misses, `sys/config/` and `sys/govern/`; the four rows (as PR #588 adds them) are in.
- Same day, the adversarial review of #591: literals match the table's row patterns (not the top segment), constant
  `&str` arrays and slices are enumerated, and the `#[cfg(test)]` skip ends at the item's own `;` or matching `}` (it
  hid `route.rs:372-573`, including the prompt id `llm/{model}`). The self-test grew to 14 plants; the allow-list
  to 36 (three journal streams, a ring name, an operation id, the prompt id).
- Pages: `dev/testing/testing.md` (gate list + paragraph); `.claude/commands/wiki-lint.md` (the sweep is the
  script's; the lint reviews the allow-list's reasons).
