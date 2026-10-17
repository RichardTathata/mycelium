## [2026-10-17] ingest | Activation commands confined (post-360 row M)

An `[[activation]]` may name `[activation.confine]`: paths beyond the placement directory (`read`, `write`) and TCP
destinations (`connect`, `localhost:<port>` or `*:<port>`; empty is no network). The stem runs the command and its
probe under Seatbelt on macOS or Landlock + seccomp on Linux, or refuses the activation by name. The default stays
unconfined, and the guarantee report says which applies (`stem.activation_confined`).

The design choice worth keeping: on Linux the restrictions are applied to a dedicated thread that spawns the command.
Landlock domains and seccomp filters are per-thread and inherited, so the stem stays unrestricted and nothing runs
between `fork` and `exec`. Landlock limits TCP ports, not addresses and not UDP, so `localhost:<port>` is refused
under Landlock and seccomp refuses every other socket. A declared hostname is refused at load: neither backend can
hold one. Found on the way: the hook recorded every timeout as `command_failed` (it matched `"timed out"`; the runner
says `did not finish within`).

Pages: `dev/security.md` (the wasm host's doors), `dev/companions/companions.md`. Repo: `docs/reference/unit-file.md`
§`[activation.confine]`, `operations/capability-lifecycle.md`, `operations/shared-responsibility-matrix.md`, the
plan's row M block (§3.5), CI's `wasm-host` job (`MYCELIUM_CONFINE_LIVE_REQUIRED`).

The PR's review (#636) found the edges of each backend, and each is closed or stated. Landlock's TCP hooks see
`connect` and `bind`, not TCP Fast Open's `sendto(MSG_FASTOPEN)` or `listen`'s auto-bind, so seccomp refuses those.
Seccomp's number match misses x32 on x86_64, so a raw filter refuses the x32 bit. "The system read set" had grown to
`/etc` whole, so it now names files. A profile resolved per run could be widened by a symlink the previous run
planted, so it is resolved once and held open. Lesson: a sandbox's allow-list is only as narrow as its widest entry
and its oldest resolution — test with the syscalls themselves, not the shell's convenience path.
Pages also touched: `dev/concurrency/lock-order.md` (row 61).
