## [2026-10-17] ingest | Credentials attached at egress — the independent review (#637)

Eleven findings. Fixed with tests seen failing first: a start-time refusal echoed a value pasted into the wrong field
(the env entry, `value_env`, `value_file`, and zero-gaps Z2's `api_key_env` message) — now named by index and binding
or section; matching ignored the port — `CredentialBinding.port`, required for a loopback host; two bindings on one
header were both counted — refused; a 300 was followed — only 301/302/303/307/308 now; a `[[serve]]` endpoint outside
its binding is refused at stem start. Recorded rather than changed: the activation-command half of row L is *not
built*; a placeholder request uses the node's egress policy captured at `start()`; the gate's client honours a
configured proxy. Lesson worth keeping: a refusal that quotes a config field is a log line that can carry a secret —
name the field, not its contents.

Pages: `dev/security.md`. Repo: the plan's row L block and §3.6, `deprecations.md` §48, `crown-jewel.md`,
`unit-file.md`, `configuration.md`, guide 13.
