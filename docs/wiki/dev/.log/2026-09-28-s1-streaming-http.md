# 2026-09-28 — S1: the HTTP artifact path streams, and refuses by name

**What:** `design-time-tooling.md` §11 S1. `HttpLibrarySource::fetch_remote` read a whole body with
`bytes()`, capped only by a declared `Content-Length` (512 MiB); a chunked response with no length was
unbounded (E12 — the July audit's own comment said the fix was "tracked, not added"). Now the body is
read in pieces and counted against `DEFAULT_MAX_IN_MEMORY_BYTES` (64 MiB), refused by name the moment it
crosses; `PrefetchingSource` refuses a blob past its bound. The large path is new: `RangedBlobFetcher`
(`HEAD` for size, `GET` with `Range`, only a `206` read, a `200` dropped unread) and `DiskStagedSource`
(pieces hashed as they stream into a `.part-…` file, renamed into place after the content address
matches, served through an `FsLibrarySource` over the same directory so the blob runtime's ranged
install reads from disk).

**Durable knowledge:**
- Two sizes, two paths. Small (a component) → memory, bounded. Large (a model) → disk, staged. Neither
  holds more than one piece before the hash is checked. The bound is by name in the error string, so
  an operator learns which path to use rather than seeing an OOM.
- The seam gate does not scan `mycelium-wasm-host` (`check-sim-seams.sh` walks `src` and
  `mycelium-core/src`), so the crate's `tokio::fs` use is outside it — stated, not hidden.
- Tests seen failing first on the unfixed code: `a_chunked_body_with_no_length_is_bounded_by_name`,
  `the_prefetch_cache_refuses_a_blob_past_its_bound`. The new-capability gate
  `a_large_blob_stages_to_disk_within_a_memory_bound` measures resident memory with `ps` and asserts
  growth under 32 MiB while staging 64 MiB (`MYCELIUM_S1_BLOB_MIB` scales the run; the plan's 2 GiB is
  the same test with the variable set). It is `#[ignore]`d and CI runs it **alone**: it measures the
  process's resident memory, and the first full-module run showed a 68 MiB growth that was the
  chunked-body test holding its own 64 MiB bound in the same process — a measurement of the binary,
  not of the stage. Run alone it grows by single-digit MiB at 16, 64 and 128 MiB.
- `Response::content_length()` on a `HEAD` reports the empty body's size hint, not the declared
  length; `size()` reads the `Content-Length` header. Found by the staging test's first run.

**Not claimed:** no object-store client exists (S2–S4). A store with no `Content-Length` on `HEAD`
cannot be staged and says so.

**Pages touched:** `companions/companions.md`; runbook `docs/operations/artifacts.md`;
`what-is-proven.md`; the plan's S1 row; `CHANGELOG.md` Unreleased.
