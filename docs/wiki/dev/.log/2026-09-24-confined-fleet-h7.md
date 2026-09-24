## [2026-09-24] ingest | the confined-fleet profile (Boundary H, item H7)

Up: [dev](../dev.md) · page: [security](../security.md) · records `docs/design/confined-fleet.md` (ADR, adopted),
`docs/operations/confined-fleet.md` (runbook) · code `src/agent/confinement.rs`, `deploy/confined-fleet/`,
`scripts/test-confined-fleet.sh`, `tests/confined-fleet/` · CI job *Confined-fleet deployment*.

**Finding.** Every other Boundary H mitigation covers only acts that pass through the substrate. The Hugging Face
incident's decisive acts did not. An in-pod sidecar cannot separate agent egress from gateway egress, because a pod
shares one network namespace.

**Change.**
- Separate-pod reference manifests: key only in the gateway pod, agent egress restricted to the gateway API port
  and DNS, the admin port closed to agents, no service-account token.
- `ConfinementReport`: node settings as `Set`/`Unset`/`NotInBuild`; network always `Unverified`.
- A deployment test with kind and Calico (kindnet does not enforce NetworkPolicy) and a positive control per
  blocked path. It runs on a private kubeconfig, so the caller's kubectl context is untouched.
- Local run: PASS on every check. Instance metadata is N/A, not discriminating in kind.

**Kept honest.** Stand-in gateway, so gateway recording is not tested end to end; Calico only; opt-in.
