# langgraph-checkpoint-mycelium 0.1.1 — async row selection (2026-09-05)

Finding 5 (P2) of the external review (`2026-09-05-persistence-durability-p1.md`). Verified:
`alist` iterated `self._list_rows(...)` — `kv/keys` + per-row `kv` GET on `httpx.Client` — and
awaited only `_aread_tuple`. The docstring acknowledged it ("rows are tiny"); tiny bodies do not
remove network latency, and one slow gateway round-trip stalls every task on the loop.

## What changed
- `saver.py`: row selection split into a pure core — `_row_window(config, before)` returns the KV
  prefix and a key selector; `_row_if_matches(raw, filter)` decodes + filters — with two drivers
  that are the same code modulo `await`: `_list_rows` (sync) and the new `_alist_rows` (async,
  `_akv_keys` / `_akv_get`). `alist` iterates `_alist_rows` with `async for`.
- `tests/test_alist_async.py` (**no node required** — a dict-backed `FakeSaver` overrides the
  gateway primitives and records sync vs async use): 14-case parity matrix (namespace, id,
  `before`, metadata filter, limit, percent-encoded thread id, malformed key/row) asserting
  `_alist_rows == _list_rows`; the regression gate asserting `alist` makes **zero** sync calls;
  a shape test pinning newest-first / exclusive `before` / limit 0. CI's Python job already runs
  `pytest langgraph-checkpoint-mycelium/tests`, so these run on every PR (the integration file
  skips without a node; this one does not).
- README paragraph, `pyproject.toml` 0.1.0 → 0.1.1, CHANGELOG `[Unreleased]` Fixed.

## Reusable lesson
An `a*` method that calls any sync helper is a blocking call in disguise; the fix pattern is
"pure core + two thin drivers", and the gate is a fake that *records which client was used*,
not an integration test that only checks results.
