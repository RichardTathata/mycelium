## [2026-10-17] ingest | A2A at the current specification (post-360 row N)

`/a2a` serves A2A 1.0 (`SendMessage`, `SendStreamingMessage`, `GetTask`, `CancelTask`) and 0.3 (`message/send`,
`message/stream`) beside the pre-0.2 methods; the card is at `/.well-known/agent-card.json` with
`supportedInterfaces`. Every send runs one path (`dispatch_send` / `tasks_send_subscribe`), so the C7 matrix and the
federated refusals cover each name. A message names its skill in `metadata.skillId` or reaches
`with_a2a_default_skill`. The 2026-10-12 probe is a CI job (`a2a-sdk`) driving `a2a-sdk` 1.2.2 against
`examples/a2a_sdk_node.rs`. Found on the way: the A2A tests' fixed 200 ms wait before the first send lost the race
with the spawned capability advert under a loaded suite (`skill not found`); they poll the `cap/` key now.

Pages: `dev/security.md` (new paragraph), `dev/examples.md`. Repo: `docs/guide/08-a2a-interop.md`,
`deprecations.md` §46, `rbac.md`, the plan's row N block (§3.5) and §3.6.
