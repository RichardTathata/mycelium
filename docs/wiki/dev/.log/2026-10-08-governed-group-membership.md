# 2026-10-08 — a governed group's membership moves through a governance route

`POST`/`DELETE /gateway/mesh/group` refuse a group under a live membership intent (403 `governed_group`);
`POST`/`DELETE /gateway/govern/group` (`govern:write`) moves the node, for any group; every membership change through
either is audited or counted. Boundary choice: an election runs over any group, so "is an electorate" is not recorded;
the membership intent is the operator's statement that a group's population is governed, and the governor's own
liveness reading (`MEMBERSHIP_INTENT_TTL_MS`) decides it. Pages: `security.md`; `rbac.md`, `tuning.md`,
`diagnostics.md`, guide 04, `deprecations.md` §20.
