# decision — keep the full wiki surface; FTT is one use case, not the shape (2026-08-16)

**Decision (Richard, 2026-08-16):** wiki-only/git-as-truth deployments (FTT's council-wiki) are
**one use case** of mycelium-wiki, not its shape — so the whole surface stays:

- **`GitMirror`** stays as the audit story for the *general* store-as-truth shape (FsStore/S3
  corpora for agent fleets — the crate's primary offering); `GitStore` remains the E1–E4 envelope
  exception, not the default.
- **`FsStore`** stays the reference data plane; **`write_page`** stays the bootstrap convenience and
  the default `write_pages` building block; the **KV-native variant** stays a documented non-build
  behind its own envelope.
- The **mechanical duplication** between `GitMirror` and `GitStore` git plumbing (two commit paths,
  two push+tripwire impls, two renderers) is **accepted, not to be consolidated** — merging would
  couple the deliberately-dead-simple sink to the store's machinery, which the original projection
  critique argued against. Revisit only if a third git-plumbing consumer appears.

Redundancy audit that led here: nothing shipped is dead; every kept item serves a deployment shape
outside the FTT envelope. Recorded so future sessions don't re-litigate the keep.
