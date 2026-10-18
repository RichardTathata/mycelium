## [2026-10-18] ingest | The activation egress proxy's sixth review (#644)

Round 5's shared descriptor budget was one pool, so one proxy's flood could spend it for every other activation —
a shared bound without per-tenant shares is a new shared resource to exhaust. Each proxy now draws on its share
(budget over proxies, at least 8, a proxy that would push it lower refused at start), a quarter of each share is
reserved for upstream sockets, and a proxy at its share pauses accepting rather than accepting and closing — the
same backpressure into the kernel's backlog that round 5 used for unchecked waiters, now the only response to any
admission limit at accept.

Also: a budget is only as true as what it counts — reqwest's idle pool held upstream sockets the budget had already
released, so the proxy's path keeps no pool; and an abandoned blocking read still holds its thread, so a bounded
read must also refuse to start another while one is out.

Pages: none changed (no lock added; `Notify` and atomics only). Repo: the plan's row M block (§3.5),
`docs/reference/unit-file.md`.
