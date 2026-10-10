## [2026-10-10] ingest | P2 — a consensus electorate is a governed group

- Post-360 row P2 built (`docs/design/consensus-electorate.md` §8). An **electorate group** is a group with a
  governance declaration `ElectorateDecl { group, size }` at `sys/govern/electorate/{group}` (`src/agent/electorate.rs`;
  `declare_electorate` / `retire_electorate`; `POST`/`DELETE /gateway/govern/electorate`, `govern:write`, audited). It
  does not evaporate; the governor skips it (`membership_governor::converge`), the emergent watcher defers to it
  (`governor_owned_groups`), `is_governed_group` counts it; a size step is at most one member.
- The engine's door (`propose_inner`, `cross_propose_inner`, `src/consensus.rs`) holds an electorate group's roster to
  its declared size (`ElectorateUnavailable`, which can now carry `observed > declared_min`), counts votes only from that
  roster (`electorate_vote_filter` — tension (b): the group names the electorate, a trust slice only narrows it) and
  raises the quorum to a strict majority. With `consensus_require_electorate` a safety-sensitive proposal (flag, or the
  `lock/`·`leader/`·`consistent/` families) anywhere else is `ElectorateNotGoverned` (403 `electorate_not_governed`).
  `consensus_electorate` routes the cluster-scoped exclusive verbs to a group. Detection, not prevention: an embedded
  `grp/` write is refused at the next proposal; no Layer I guard.
- `cons.safety_profile` rev 2 is node-enforced; `secure-single-domain` rev 3 requires it.
- Pages touched: `dev/architecture/runtime-invariants.md` (Layer III posture paragraph), `dev/operations.md` (profile
  rev 3). Outside the wiki: the decision record (§3 table, residual, §4.1, §5, §7, new §8), threat model §7, guide 04,
  guide 20, the FAQ, error-handling, diagnostics, production-readiness, configuration reference, `what-is-proven.md`,
  philosophy, `src/lib.rs` namespace table, the guarantee catalogue golden, CHANGELOG.
