# 2026-10-09 — one stop-signal handler; a typed lost write

- **`mycelium::shutdown::ShutdownSignal`** replaces three copies (node binary, stem, now the examples): SIGINT or
  SIGTERM, installed before the bind, awaited after; a **second** signal exits at once with `128 + signal` — the
  handlers stay installed, so without it a hung shutdown waited for SIGKILL. Do not create one in an interactive
  program (a handler nobody awaits swallows Ctrl-C) — the node binary skips it under `-i`. Pinned by
  `tests/shutdown_signal.rs`, which re-runs its own binary as a child that hangs after the first signal (libtest
  prints `test NAME ... ` with no newline before a test's output, so the child's markers end a line, not fill it).
- **The demo images stop on `docker stop`**: `federation_node`, `llm_agent`, `three_node_demo` (whose roles never
  return, so `main` races the role against the signal and then shuts the agent down).
- **`mycelium-py` 0.2.8 `SupersededError`** for a lost `consistent_set` / `cross_group_propose` (409 `superseded`),
  an `httpx.HTTPStatusError` subclass so existing handlers keep catching it.
- A public module makes the next release a MINOR: **2.31.0**, not 2.30.1; the docs written as 2.30.1 moved with it.
