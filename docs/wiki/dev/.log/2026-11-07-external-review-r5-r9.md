# 2026-11-07 — the external review's R5–R9 closed on evidence

`docs/plans/external-review-2026-10-31.md` rev 0.8 records rows R5–R9 delivered (#719, #718, #722, #720, #721, merged
2026-11-06/07, unreleased); its §5 quotes each promise beside the code and the test that delivers it.

**Durable knowledge.**

- **A bound on a listener has to start before the protocol is known.** hyper's header-read timer starts only after
  hyper-util has sniffed HTTP/1 against HTTP/2, so a silent socket, or one stopping inside the 24-byte HTTP/2 preface,
  was never timed. The gateway's `PrefaceDeadline` covers the gap; HTTP/2's keep-alive PING is floored at 1 s so a
  gossip-tight `handshake_timeout_ms` cannot close live WAN clients (R5).
- **A durable record needs a durable name.** `fsync` on a file does not make its directory entry survive a power loss;
  every holder on the path is synced on every open, not only the directories a call created, because an earlier
  failed sync or a concurrent opener can leave an existing ancestor unsynced. A filesystem that refuses directory
  fsync on a pre-existing ancestor is passed over and counted, never on a directory the call made (R6).
- **TLS session resumption skips certificate verification.** The bulk client resumes no session and pools no
  connection, so a revoked key is refused on the next fetch — the mesh made the same choice in 2.35.0 (R7).
- **The order of two counter loads can be a durability claim.** A replica-sync answer reads in-flight (Acquire) before
  dropped: an abandoned window counts its drop before releasing in-flight, and the other order lets a dropped record
  read as persisted. The window closes at WAL enqueue, because the answer's sync rides the same channel (R8).
- **An audit ignore read for every lockfile is broader than it says.** `.cargo/audit.toml` applied to all three
  lockfiles, so the one accepted warning moved onto its own lockfile's line in the `audit` job (R9).

**Follow-ups** (the plan's §5): `mycelium-reason` and `mycelium-guardrails` need a version bump at their next release
for the Tokio floor; `mycelium-core`'s rustdoc has 35 unresolved intra-doc links and no CI step runs rustdoc.
