## [2026-10-18] ingest | The activation egress proxy's fifth review (#644)

Round 4 bounded the peer-uid waiters by closing the oldest past 64 — which, under a burst, closed the command's own
connection. The root fix moves the queue to where FIFO order is free: while 64 wait, the accept loop stops
accepting, and newer connections wait in the kernel's listen backlog. Nothing is evicted; the residue is a
sustained flood filling the backlog faster than reads drain it, which costs a SYN retry, not a refusal. Lesson: a
bounded queue with eviction is a refusal policy; when the producer is a listener, backpressure into the kernel's
own queue is usually the better bound.

Also: a per-instance cap multiplied by an unbounded instance count is no bound — the descriptor budget is now one
per process (`FdBudget`, a quarter of the soft `RLIMIT_NOFILE`), shared by every proxy and counting upstream
sockets; any external read on an admission path (a `sysctl` child) needs its own deadline; and a kernel table keyed
by address family must be searched under every spelling a client can use (IPv4-mapped in `tcp6`) and every state
a live client can be in (FIN_WAIT1/2, CLOSE_WAIT — never TIME_WAIT).

Pages: `dev/concurrency/lock-order.md` (row 66). Repo: the plan's row M block (§3.5), `docs/reference/unit-file.md`.
