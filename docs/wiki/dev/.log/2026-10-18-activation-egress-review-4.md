## [2026-10-18] ingest | The activation egress proxy's fourth review (#644)

Round 3's peer-uid admission reopened a denial: each connection waited for a lookup slot with no cap, and each lookup
read the whole socket table, whose size the flood itself grows. The fix keeps the guard and removes the
amplification: one read of the kernel's table answers every connection accepted before it began (`PeerGate`), at
most 64 wait (oldest closed first), and a proxy holds at most 256 sockets. Lesson: an admission check is itself a
door — bound what waits for it and what it costs per arrival, or the check becomes the attack.

Also: parsers that match on fewer fields than identify a connection (ports only; any TCP state) are a silent
mismatch away from naming the wrong owner — match the full 4-tuple, the live state, and sanity-check the layout.

Pages: `dev/concurrency/lock-order.md` (row 66). Repo: the plan's row M block (§3.5), `docs/reference/unit-file.md`.
