## [2026-09-25] ingest | closure plan C6: the filesystem store asks too

Up: [dev](../dev.md) · page: [security](../security.md) · record `docs/plans/boundary-h-closure.md` C6 · code
`mycelium-wiki/src/fs.rs` (`FsStore::with_authority`), `mycelium-wiki/src/store.rs` (`WriteAuthority`).

The one store where the seam was missing. `FsStore` asks the authority after taking its own mutation lock and
before writing anything, in all four mutators. It has no appointment fence, so for it the authority is the whole
check; the plan ranked this lowest because no deployment runs `FsStore` with authority on the line, and it was
cheap enough to close anyway. The trait moved to the always-compiled store module; the old path still resolves.
