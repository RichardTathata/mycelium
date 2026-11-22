## [2026-11-22] ingest | `latest` only for the highest version; the release checker's allow-list and deny-list (SO3 follow-up)

A follow-up to #827 (security-operations SO3), from its re-review and the maintainer's decision of 2026-11-22.

- **`latest` moves only for the highest `vX.Y.Z`.** `publish` asks `scripts/release-is-latest.py` over
  `git ls-remote --tags origin 'refs/tags/v*'`; `gh release create --latest="$LATEST"`, and `Move latest` leaves
  `:latest` with a notice unless the answer is `true`. Compared as integers (2.10.0 > 2.9.0); pre-release
  (`vX.Y.Z-…`, not a convention here) and companion (`<name>-vX.Y.Z`) tags ignored; a listing without the tag being
  published fails closed rather than reading as "nothing is higher". RELEASING § 8's hand-run `gh release create` uses
  the same answer.
- **The checker is two things, and now says so:** an allow-list for triggers, permissions, actions, step conditions,
  the `gh release`/`imagetools` commands, a verification step's lines and the version step's outputs; a deny-list for
  other shell. A deny-list over shell cannot be complete: the re-review's fifteen fresh plants all passed the
  previous version; thirteen are now rules (verify steps hold only `set -euo pipefail`/`cp`/verify lines; the three
  image verifications name each digest once, from its own output; `buildx build` and pushes only in `linux`, by
  digest; every `||` ends `exit 1`; no `GITHUB_*`/`GH_*` env override; the files attestation's subjects exact; the
  ancestry refs exact; version's `GITHUB_OUTPUT` writes exact). Two stay known in its docstring: HTTP from an
  interpreter, and a command reached through a variable, `eval` or `sh -c`.
- **A `sha-<commit>` staging tag left by a failed verification names an unverified image** — delete it, never pull or
  promote it (RELEASING § 8).
- RELEASING § 7 no longer says a dispatch on a tag publishes (it has not since #827's F1): a tag push that started no
  workflow is recovered by pushing the tag again on its own.

Pages: `.github/workflows/release.yml`, `scripts/check-release-workflow.py`, `scripts/release-is-latest.py` (new),
`RELEASING.md` §§ 7–8, `docs/plans/security-operations.md` SO3, `docs/operations/what-is-proven.md`
(`binary-provenance`), `docs/features/published-node.md`, `docs/wiki/dev/operations.md`, `CHANGELOG.md`, `Makefile`,
`.github/workflows/ci.yml`.

**After #830's review (same day):** the env rule was a deny-list of prefixes and missed what defeats a verification
outright — `PATH` (a fake `gh`), `BASH_ENV` (the verify script is bash, so it sources the stub), and an earlier step
writing `$GITHUB_PATH` or `$GITHUB_ENV`. It is now an allow-list per level (workflow `IMAGE`/feature lists; job
`VERSION`/`DRY_RUN`/`GH_TOKEN`/`ATTEST`; step `BUNDLE_*`/`DIGEST_*`/`LATEST`), and no step writes those two files.
Also: a `||` block may hold no other `exit`; publish's refusals are matched as exact lines (an `&&` rewrite kept the
substring); no script under `scripts/` is named except to run it; the tag regex refuses leading zeros (`v02.99.0`
read as above 2.41.0); `Move latest` asks the remote again just before it moves. Lesson: the answer reads tags, not
Releases, so a failed or refused tag left on the remote holds `latest` back — RELEASING § 7 now says delete or re-run.
