## [2026-11-27] ingest | A governed group's `gcap/` projection follows its membership (ER9 PR 1, R10-01)

The emergent-group watcher (`src/agent/emergent_groups.rs`) took governed groups out of its join and its leave (#56,
`30719ff0`) so that a membership intent's `max` and `drain` could hold; the projection task (`gcap/` and the
`requires` opacity) was fused to those two branches and was lost with them — by accident, not by a stated design. A
drained node kept projecting (dispatch chose an empty group and reported `Emitted`), a governor-joined node never
projected (`Unwired`), electorate members never projected, and after an intent evaporated a drained node projected
without being in `grp/`.

Durable knowledge:
- **"Member" and "project" are separate questions.** For an emergent group the watcher writes both on a filter
  match. For a governed group it writes only the projection, exactly while `grp/{g}/{self}` is live in the store and
  the filter matches (D9a), whoever wrote the membership: the governor, `/gateway/govern/group`, `join_group`, a peer.
  The governor stays unaware of projections (no shared `joined` map, no lock).
- **One definition of "governed"** (D9c): `membership_governor::governed_groups` / `is_governed`. The intent's group
  is its body's (the governor's reading); `GovernedFor::Node` honours `target` (the watcher), `GovernedFor::Fleet`
  does not (the gateway's refusals, as the electorate floor and the conflict detector read it). The gateway's old
  reading took the group from the key — the only behaviour change there.
- **Evaporation needs a task-local `was_governed` set:** the re-join on governed → emergent is restricted to the
  transition, so a hand-left emergent group is not rejoined.
- **Wake sources, no new timer:** a predicate subscription on `grp/` keys ending `/{self}` and the intent prefix,
  into the existing debounce; an evaporation is noticed on the next own-advertisement refresh.
- **Replay corpora:** neither `tests/replay-corpus/` recording (`x9-selective-recovery`, `y2-schedule-run`) holds a
  group, an intent or a `grp/` write, so the extra wakes add no pass there; both still replay (see the PR).

Not built here: ER9's second PR, the readers' membership check (`wiring_snapshot`, `demand_snapshot`).

Pages: `docs/wiki/dev/owner-signed-records.md` (rows `grp/` and `cap-group/`·`gcap/`: the new forged-write
consequences, statuses unchanged), `docs/wiki/dev/architecture/runtime-invariants.md`, `docs/features/governed-membership.md`,
`docs/guide/02-capabilities.md`, `docs/guide/deprecations.md` §TBD-ER9, `docs/design/consensus-electorate.md`,
`src/lib.rs` (prefix table), `CHANGELOG.md`.
