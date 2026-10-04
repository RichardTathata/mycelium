## [2026-10-04] ingest | realignment repairs R2 — the WAL's startup repair pinned, its failure a refusal, one owner

**What:** `mycelium-core/src/persistence.rs` (`decode_wal_records` reports a trailing 1–3-byte prefix
as `Torn`; `OwnershipLock` — the journal's lock moved to core — and `WalHandle::hold_ownership`;
the `replay` / `spawn_wal_writer` rustdocs tell a direct embedder what the lifecycle does for the node
binary), `src/agent/lifecycle.rs` (the lock before replay; the startup snapshot's result is a refusal
by name instead of `let _ =`), `src/agent/journal.rs` (uses the core lock), `runtime-invariants.md`
§Persistence invariant 4, the sim-seam baseline rows and inventory entries, the changelog, and this
log.

**Durable knowledge:**

- **The WAL's torn-tail repair is the startup snapshot, not `replay`.** `replay` applies the good
  prefix and leaves the tail; `do_snapshot`'s step 4 truncates it; `start()` triggers that snapshot
  before the handle is installed. The review's F01 probe composed `replay` + `spawn_wal_writer`
  without the lifecycle and so reproduced a scenario the node binary does not have. The invariant was
  real and unwritten; now it is pinned and on the page.
- **The one real hole was `let _ = handle.trigger_snapshot().await`.** A failed startup snapshot
  (a directory the node cannot write into, a full disk) started the node behind the torn frame, and
  the appends after it were acknowledged and then swallowed by the frame's claimed length — the
  journal's F02 shape, in the WAL, on a path that needs a filesystem failure to reach. Refusing is the
  v2.20.0 class ("a node used to start degraded"). Under `quarantine` it warns and starts without the
  repair — the files are readable, so moving them aside would discard state.
- **A read-only persistence directory also stops the lock file**, which is why the refusal witness
  pre-creates `wal.bin.lock` before taking the directory's write bit away: the test must reach the
  snapshot, not fail at ownership. Written down because the next person to write such a test will
  hit it.
- **The lock lives in the writer task, not the handle, and `shutdown()` now stops the writer.**
  The first cut held it on `WalHandle`, and the pin test's restart was refused: `shutdown()` never
  touched the WAL writer (its `shutdown` was `#[allow(dead_code)]`), the handle lives in `task_ctx`
  and is cloned into every connection context, so the lock outlived the agent's shutdown. Now
  `hold_ownership` sends the lock down the writer's channel ahead of every append, the task drops it
  on exit, and `shutdown_with_timeout` stops the writer after the task drain and waits for its
  receiver to close. A consequence worth knowing: the writer's final snapshot now runs at
  `shutdown()`, not at drop.
- **A graceful restart cannot pin a WAL durability property.** The pin test's first version
  restarted the node and read the key back, and it *passed with the startup snapshot removed*:
  `shutdown()` takes a final snapshot from the store, and the store holds the value whatever the WAL
  says. The witness now copies `snapshot.bin` and `wal.bin` while the node runs, right after the
  synced append, and replays the copy — a crash. With the startup snapshot toggled off, that replay
  returns `[]`: the acknowledged append is gone. Any future WAL witness should model the crash the
  same way.
- **The second agent on one path used to fail at the bind, not at the WAL.** Both agents share the
  node id, so the port collision masked the shared `wal.bin`; the ownership refusal now comes first
  and names the owner. The witness asserts the message, which is what makes it fail-first.
