## [2026-10-18] ingest | The activation egress proxy's second review (#644)

Round 1's fixes confirmed; seven availability and precision findings fixed. The one worth keeping: **count what has
authenticated, not what has connected.** A cap on raw connections to a loopback proxy is a lock any local process
can turn — idle token-less connections filled it. The proxy now counts a connection against its cap only once it
presents the token, closes unauthenticated ones fast (2 s, on the 407, oldest first past 16), and keeps a tunnel's
place until it closes. What remains is stated: a process opening token-less connections faster than they are
closed can still crowd the command out, since nothing distinguishes connectors before the token. Also: mask the
longest value first; read every encoding header; a hard link is a second name for a secret.

Pages: `dev/concurrency/lock-order.md` (row 65). Repo: the plan's row M block (§3.5), `docs/reference/unit-file.md`,
`crown-jewel.md`, `deprecations.md` §51.
