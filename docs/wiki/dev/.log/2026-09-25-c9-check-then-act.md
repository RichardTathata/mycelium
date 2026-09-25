## [2026-09-25] ingest | closure plan C9: the check-then-act window, answered per site

Up: [dev](../dev.md) · page: [security](../security.md) · record `docs/design/authority-at-execution.md` §9 · code
`mycelium-wiki/src/git_store.rs` (`late_writes`), `mycelium-wiki/hooks/pre-receive-mandate-window`.

The review's point, generalised: every "check, then act" has a window as long as the longest pause, and normal
latency does not bound it. It cannot be closed where the effect has no clock, so each site now names its answer:
prevent (a remote hook with its own clock), cancel (C10), or detect (a late write counted after the fact). The hook
judges the appointment's window only; revocation at the remote remains the fence's job, since a hook has no
revocation feed.
