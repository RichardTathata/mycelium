## [2026-11-27] ingest | ER5 PR 1: a dropped or shut-down provisioner stops what it installed

R14-01 (`docs/plans/engineering-review-urgent-fixes.md` §7): a stem's ticker ended with `drop(prov)`, `Provisioner` had
no `Drop`, and a WASM install's serve task was a `JoinHandle` aborted only in `uninstall` — dropped, it detached and
kept its receiver, so a stem restarted on the same node ran one call on two builds.

The durable rules: **a drop is a stop, by construction** — a WASM install's serve loop selects a retire `oneshot`,
`biased`, ahead of `recv`, and the dropped sender ends it, so no implementation has to remember an abort; **nothing
aborts a serve loop**, so a call in flight is answered and only calls queued behind it are dropped (a timeout, never a
negative). `Installed::retire` (stop, keep placed bytes) is what a stop does; `uninstall` (stop, remove them) is a
withdraw. `Provisioner::hosted` became `Mutex<Hosted { closed, map }>` — the same lock — and the install completion
checks token and `closed` and advertises in one hold, so a shutdown either retires a `Live` install or the install
never advertises. Install tasks are not aborted (the blocking pull would leak its temp file).

Pages: `concurrency/lock-order.md` row 21 (type, sites, the advertisement under the lock: rows 1 and 7 as leaves).
Outside the wiki: `docs/features/stem.md`, `docs/features/hosted-components.md`, `docs/guide/deprecations.md`
§TBD-ER5, `CHANGELOG.md`. Not yet (PR 2): the stem's other tasks on a drop, the re-probe `JoinSet`.
