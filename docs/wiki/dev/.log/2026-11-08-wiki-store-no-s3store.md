## [2026-11-08] ingest | no `S3Store` ships

`dev/companions/wiki.md` described `S3Store`'s conditional `PUT`/`If-Match` as the same CAS contract as
`FsStore`'s, as if one were built. None is: `mycelium-wiki` ships `FsStore` and `GitStore`, and the `WikiStore`
trait admits an object store that nobody has written. The page now says so. Found by #737's independent review
(feature pages, row F3), which fixed the same claim in `mycelium-wiki/src/{lib,fs,store}.rs`,
`docs/operations/companions.md`, `docs/design/transparency-council-substrate.md` and `docs/design/wiki-git-store.md`.
