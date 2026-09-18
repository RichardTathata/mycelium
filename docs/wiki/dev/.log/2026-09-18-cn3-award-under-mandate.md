## [2026-09-18] ingest | CN3 — the award under the acceptor's mandate

Up: [dev](../dev.md) · plan `docs/plans/v3-contracts-axis.md` §6.9 (CN3 ✓) · record `docs/design/scoped-mandates.md`
· code `mycelium-commitment/src/lib.rs` (`commit_award_under_mandate`).

### What landed

Item 5's fence applied at the one place the contract net accepts an obligation: the award. The declarer holds the
requirement's `ResourceAuthority` (its scope at its installed epoch) and the acceptor's `Mandate`; the award is
committed only if the mandate is the acceptor's own and the authority authorizes `accept` for it now. A stale
holder — a mandate minted under an epoch the resource has moved past — is refused as `Superseded`, before any
write.

### Two things worth carrying forward

1. **Check the holder before the epoch.** A mandate that passes every epoch and window test but names another
   principal would authorize the wrong acceptor; `MandateNotTheAcceptors` is checked first and refuses by name.
   The plan's sentence is "the *acceptor's* mandate epoch", and the possessive is a check of its own.
2. **"Skipped, not faked" resolved as "not called".** The plan allowed the negative case to be skipped where
   item 5 is absent. Item 5 is present, so the case runs in CI; and a deployment without mandates does not get a
   `commit_award_under_mandate` that silently passes — it uses `commit_award_linearizable`, a different method.
   A check that can be turned off is a check that will be; a check that is either made or not called cannot be
   mistaken for made.

### Not done

Revocation *during* an award round (a mandate superseded between the check and the commit) — the check is
before the round, not inside it; the round's own linearizability bounds the window but does not close it. The
CN-gate's four-arm harness is §13.3's experiment, on the research track.

### Pages touched

- [history.md](../history.md) — the CN3 section.
- The plan's §6.9 table marks CN3 landed; the crate README and the companions bullet; CHANGELOG.
