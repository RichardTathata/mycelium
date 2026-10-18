## [2026-10-18] ingest | The activation egress proxy's third review (#644)

Round 2 and both rebases confirmed. The stated crowding limit was closed at its root for other users: the kernel
knows which user owns each loopback socket, so the proxy asks it at accept (`/proc/net/tcp{,6}` on Linux,
`sysctl -b net.inet.tcp.pcblist_n` on macOS — `xsocket_n.so_uid` at offset 64 under `#pragma pack(4)`, confirmed on
a live connection) and closes another user's connection, or one it cannot attribute, before it holds any place.
What stays is same-user crowding, inside the stem's trust boundary. Also: a symlinked `value_file` is judged by the
file it names (`metadata`, not `symlink_metadata`); a bodiless answer is relayed whatever its `Content-Encoding`.
Lesson: "the proxy cannot tell who connects" was true of the protocol, not of the host — look one layer down
before stating a limit.

Pages: the plan's row M block (§3.5). Repo: `docs/reference/unit-file.md`, `peer_uid.rs`, CI's `wasm-host` job (a
step connecting as `nobody`).
