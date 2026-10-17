## [2026-10-17] ingest | Credentials attached at egress (post-360 row L)

A node declares credential bindings (`[[egress_credentials]]`, `GOSSIP_EGRESS_CREDENTIALS`); a caller holds the
placeholder `mycelium-credential:<name>`; `EgressCredentials::send` attaches the value only at the bound host, refuses
the request elsewhere, follows redirects itself and drops the credential on a hop that leaves the binding, and counts
each binding's uses. Values are read once at `start()` and appear in no `Debug`, error or log. `[[serve]]` gained
`credential`, `OpenAiBackend` gained `with_credentials`, guarantee `egress.credentials`. Found on the way: a unit's
`{:?}` printed a `[[serve]]`'s literal `api_key` (now redacted). Design note worth keeping: the gate owns redirects
because a reqwest redirect policy can only follow or stop, and reqwest's cross-origin strip covers `Authorization` and
`Cookie` but not a custom credential header.

Pages: `dev/security.md` (WS3 bullet). Repo: `docs/operations/crown-jewel.md` (the new subsection and the table of
secret entry points), `docs/reference/unit-file.md`, `docs/reference/configuration.md`, the guarantee catalogue, the
plan's row L block (§3.5).
