## [2026-10-17] ingest | A confined re-probe re-resolves a replaced path (row M's follow-up)

Row M kept each activation's confinement profile and re-resolved it when a declared path no longer had the
device and inode it was resolved with (`Profile::is_current`). The hook re-resolved, but the re-probe did not. It
failed the probe, so a rotated directory or a file replaced by an atomic rename withdrew the install, and the next
round reinstalled it: a flap if the path churned.

The re-probe (`activation::reprobe_one`) now takes its profile from the hook's `ResolvedProfiles`, which each
`Watched` shares by `Arc`. A stale profile is re-resolved through `prepare`, with every check repeated, and the probe
runs under the fresh one. Only a refused re-resolution withdraws the install, and the trace names it
(`prov.probe` `confinement_failed`, rule revision 3). A profile that is no longer current *after* the run was stale
for part of it. Its verdict is not trusted: the probe is re-resolved and run once more, and a change under both runs
is `confinement_changed`.

The invariant worth keeping: **check currency on both sides of the run.** A check before the run alone leaves the
window open. On Linux the run already fails closed, because the rules come from descriptors pinned at resolution.
On macOS a same-path real directory matches the Seatbelt profile by name, and a planted symlink resolves to a target
the profile does not name. The post-run check makes the verdict honest on both platforms.

Pages: `dev/concurrency/lock-order.md` (row 63: the re-probe task now takes it). Repo: `docs/reference/unit-file.md`
§`[activation.confine]`, the rule catalogue, and the plan's row M block and §3.6.
