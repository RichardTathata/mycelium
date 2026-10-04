## [2026-10-04] ingest | realignment repairs R4 — the gate reads the host the client dials

**What:** `mycelium-core/src/config.rs` (`host_of_url` parses with the WHATWG `url` crate; core gains
`url = "2"`, already in the lockfile through reqwest), `mycelium-wiki/src/sink.rs` (`remote_host`
refuses an authority with a backslash, whitespace or a control character), three witnesses, the
changelog, the plan row, and this log.

**Durable knowledge:**

- **An allow-list is only as good as the agreement between the gate's parser and the client's.**
  The hand-rolled parser and reqwest's WHATWG parser disagreed on a backslash in the authority, so
  `http://evil.example\@allowed.example/` passed as `allowed.example` and connected to
  `evil.example`. No redirect was involved, which made this stronger than the review's F03. The
  regression test does not hard-code expected hosts; it asserts the gate's host *equals the client's
  parse* for every vector, so it keeps meaning something if either parser changes.
- **Where the client's parser is not ours, refuse what the parsers could disagree on.** The git
  mirror hands its remote to git, and so to curl or ssh, each with its own URL rules. Matching all of
  them is not possible from here; refusing an authority containing a backslash, whitespace or a
  control character is.
- **Still a name-based gate.** Nothing here resolves; an allowed name that resolves to an address the
  operator meant to deny (DNS rebinding, the metadata address) is outside the guarantee, and R5 says
  so in the threat model and the catalogue.
