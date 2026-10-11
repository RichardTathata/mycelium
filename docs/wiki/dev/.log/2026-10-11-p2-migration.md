# 2026-10-11 — P2's migration: close · enumerate · carry · complete

**What shipped (unreleased, after 2.32.0):** changing an electorate's members is a migration that **terminates**.
The parked branch's lazy carry (`fix/p2-migration` rounds 4–5: the successor read a strict majority of its
predecessor at its first touch of each slot, with the "carried" set in memory) left the predecessor needed for ever;
the maintainer decided (2026-10-11) on close → enumerate → carry → complete. Design record §8.3 has the procedure and
the six-sentence safety argument; `src/agent/migration.rs` the driver; `src/agent/electorate.rs` the records
(`ElectorateDecl::predecessors`, `Retirement { passes_default }`, `Completion`); `src/consensus.rs` the gate's
`Retire` kind, the group tag on the acceptor memory (`AcceptorSlot::groups`, record tag `0x04`), the manifest
(`CarryManifestRequest`/`Page`), the carry ballot (`CarryPrepare`/`CarryPropose`, `propose_carry`) and the door and
acceptor rules (`successor_ready`, `carry_admissible`, `migration_complete`, `incomplete_carry`, `default_passes`,
`completion_acceptable`).

**Decisions taken while building:**
- A carry ballot is distinguishable on the wire (two appended variants) so an **acceptor** can admit it per
  predecessor and refuse an ordinary proposal until the completion — fail closed at both ends for rows (i) and (iv);
  the first cut admitted the carry only at the door, which made a merge or the D2 escape impossible (a merge's carry
  from the first retiree had to wait for the second).
- A **merge** completes each predecessor as its retirement becomes visible; ordinary decisions wait for all. A
  group with an **incomplete carry** (a predecessor retired to it, no completion) cannot retire — its successor
  would enumerate it alone — so a chain never reads past its predecessor and `holds_default` is one level.
- The group tag lives **in the acceptor record** (tag `0x04`, appended groups; untagged slots keep the 2.32.0
  bytes) rather than a separate key: the manifest after a restart must be complete, and the record is already the
  durable, fsynced thing. A genesis without predecessors encodes in the 2.32.0 layout (the codec is not
  self-describing; `decode` tries the new shape, then the old).
- No lock added: every new cache is a papaya map or set; the per-node single-flight for a completion run is an
  insert-if-absent on a set with a drop guard.

**Rebase note:** both parked commits conflicted wholesale with #601's cut-back and the release; resolved to main
(the shipped model) and re-applied as new work. #604 (restore-or-retire, docs) merged mid-way; the migration how-to
is the "retire the group" branch of its fork, its restore-or-retire bullet kept verbatim.

**Not built, stated:** DR of a group that lost its majority; a batched carry (one ballot per slot, sequential); a
digest-only acceptance no decision record resolves stops the carry at that slot, named.

**#606's review (the same day), four fixes:** (1) the retirement's *prepare* closed a group with no preconditions —
a lagging member's bare prepare could close a successor whose carry was incomplete, for ever; now every acceptor
runs the retire preconditions at the prepare (`retirable` + a live successor naming the group) and refuses without
closing, and the escape from a predecessor that never retires is an explicit `drop_predecessor` (a successor-majority
decision, void if a retirement to the group is visible); (2) manifests report untagged slots too (a 2.32.0-shaped
record is carried; a slot the successor may not decide at all is skipped); (3) a retiree that holds the default need
only be live, not unique, so two defaults can be merged; (4) a verified completion stands as proof of the
retirement (`predecessor_epoch`, `passes_default` inside it) and the predecessor's records are fsynced at
completion on every successor member. Plus the carry's first ballot above the manifests' highest promise/acceptance.

**Round 2 (the drop race):** a drop admitted on the mere absence of a retirement raced the retirement (quorums in
different groups). Now a drop is admitted only for a predecessor whose retirement to another successor is visible, or
under the operator's `force` assertion (outside the guarantee, audited); a carry that finds another value above the
carried one is a detected conflict — counted on `commit_conflicts`, named in the completion (`conflicts`,
`conflict_slots`), the completion still decided (a conflict is a fact about the past; refusing would freeze the
successor); `completion_acceptable` checks `passes_default`/`predecessor_epoch` against the retirement; `slot_home`
tells "not this group's" (skip) from "the default cannot be named here" (stop, named).
