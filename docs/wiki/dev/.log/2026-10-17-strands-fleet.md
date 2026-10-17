## [2026-10-17] ingest | A Strands fleet, demonstrated (post-360 row P)

`examples/strands_fleet/`: an operator and three nodes from `examples/strands_fleet_node.rs`, and Strands agents (scripted
models) driving them — a Swarm planning over `/a2a`, the planner refused a dispatch it holds no mandate for (and
refused again presenting the courier's grant), the courier dispatching with its own grant and possession proof, the hub
stopped and the courier resumed on the depot from its stored session. CI job `strands-fleet`. Found on the way: a
mandate is an appointment — one holder per scope and epoch at each gateway's verifier (`ConflictingAppointments`
otherwise; the verifier's memory is per node, so one holder fleet-wide is the authority's to keep) — and a node's
execution authority guards one scope, so per-agent permissions are the policy's job and the mandate is the role. The
review found the first operator signed a grant for anyone who asked, which made the appointment the driver's choice;
it now appoints from configuration and signs nothing else.
The refusal of a mandate-requiring allowance without a mandate is recorded `indeterminate`, not `deny`, and runs
nothing.

Pages: `dev/examples.md`. Repo: `examples/strands_fleet/README.md`, `examples/README.md`, the plan's row P block (§3.5)
and §3.6.
