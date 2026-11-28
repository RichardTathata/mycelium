## [2026-11-28] ingest | ER4: one capability name, one running build per provisioner

R12-01 (`docs/plans/engineering-review-urgent-fixes.md` §6): the guard against two builds of one `{ns}/{name}`
(`started_caps`) was a local of one `provision_round`, and the lasting reservation was keyed by artifact. An install
advertises only once it resolves, so demand stays unmet while it runs, and the next round (a stem ticks every 500 ms)
started a sibling line with another digest. `cap.invoke/{ns}/{name}` is name-only and `deliver_to_handlers` sends to
every receiver, so one call ran on both builds. The fix recorded in the audit of 2026-07-15 held for one round.

The durable rules: **a guard that must outlast a round lives in the lasting state, not in the round** — the name is held
in `Provisioner::hosted`'s map (`HostedState::Installing { serves }`, `LiveHosted.provides`), and the reservation
checks `closed`, the artifact and the name (`name_holder`) and inserts in one hold, so it is the one door for the
demand, shadow and presence starts. **Release needs no code of its own:** the name frees wherever the entry leaves the
map. No new lock (row 21's text changed). A second map keyed by name, keying `hosted` by name, and a one-receiver
guard at the signal layer were rejected (two locks in step; breaks withdraw and the token logic; teaches Layer II a
provisioner's law).

Limits stated, not closed: per provisioner only; a pull that never resolves holds its name for good (no install
deadline; the HTTP source has no timeout); a stopped stem's late install serves for a moment beside its replacement
stem's build and its retire tombstones `tools/{name}/{node}` (ER5-F1 — the reachable path is across provisioners, since
no production caller withdraws an `Installing` entry); no upgrade path (retire-then-replace, D4a, a separate PR).

Pages: `concurrency/lock-order.md` row 21. Outside the wiki: `docs/features/demand-provisioning.md`, `stem.md`,
`hosted-components.md`, `presence-bands.md`, `shadow-acceptance.md`, the rule catalogue (`prov.demand_response` rev 2,
`prov.presence_floor` rev 4), `docs/guide/deprecations.md` §135, `docs/analysis/ratings.md` (the 2026-07-15
entry corrected), `CHANGELOG.md`.
