## [2026-10-18] ingest | The activation egress proxy's seventh review (#644)

Clear to merge; three lows closed first. The substantive one: an admission check that reads a whole kernel table
has a cost another user controls (idle sockets anywhere inflate it), so on Linux the owner now comes from a
`sock_diag` query for the exact 4-tuple — the kernel answers from its connection hash. It is built as plain bytes
over `rustix`'s netlink socket, without `unsafe`; `/proc/net/tcp{,6}` stays only as the fallback where netlink is
forbidden. macOS has no per-socket query, so there the residual is stated: a flood of idle sockets can push the
`sysctl` read past its bound, which fails closed.

Also: a flag that marks "work in flight" must be cleared by a drop guard inside the work, or a panic after the
waiter has given up leaves it set forever; and a value chosen under `cfg(test)` hides the production formula from
every test — put the formula in a pure function and test it there.

Pages: none changed. Repo: the plan's row M block (§3.5), `docs/reference/unit-file.md`.
