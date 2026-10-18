## [2026-10-18] ingest | The activation egress proxy's first review (#644)

Every confined command now starts with a cleared environment (not
only an egress-proxied one), so the egress guarantee's "no value in the environment" holds for all; a binding the
command could read some other way — a `value_file` in its read paths, an `env` entry carrying the same value —
refuses the stem's start; an upstream's echo of the value is masked (`Redactor`, length-preserving, across chunk
boundaries; an encoded answer refused; no tunnel to a bound host); a reload narrows a proxy the file no longer names;
the proxy is bounded (connections, header read, idle tunnels, buffered bodies); the https path is tested against a
loopback TLS upstream. Lesson: a guarantee about a credential must be checked against every way a process can read
— its environment, its files, and what its peers send back — not only the path the feature was built for.

Pages: `dev/security.md`. Repo: the plan's row M block (§3.5), `docs/reference/unit-file.md`, `crown-jewel.md`, `deprecations.md` §51.
