## [2026-09-30] ingest | X2, the first slice: two demos both ways

**What:** `docker/Dockerfile.stem` (one image: the stem node, the artifact tool, the co-op
`stem_driver`), `docker/docker-compose.stem-examples.yml` (profiles `provisioning`, `catalog`),
`make test-stem-examples` / `make examples-both-ways DEMO=…` / `make stem-keys`, the `stem-examples`
job in `cluster-suites.yml`, and the units' real publisher key.

**Durable knowledge:**
- **The checker accepted what the stem refuses.** X1's units carried `trusted_publishers =
  ["ed25519:coop-ci"]`; `wire-check` never parses the key, `Stem::start` requires 64 hex. The stem run
  is the second reader of the same file, and it found the placeholder. The key is now derived
  (`make stem-keys`, seed 42…42) — a fixture, not a secret.
- **The driver is the seed.** A co-op depot (`spawn_depot`) binds 127.0.0.1 under a shared CA and
  cannot join a container fleet; the driver is a plain node on the suite's private network and the
  stems bootstrap from it, so there is no TLS and no readiness endpoint to wait on — the driver waits
  on peers and on resolution instead.
- **Which roles are code, written down.** §16 said seeder/buffer/worker stay code; the recut makes
  that a list per demo with a reason per exclusion (wave 3's acceptance, catalog's peer cache, the MCP
  bridge, the blob runtime's activation hook, llm_agent's simulated pulls). Each is a stem capability
  gap, not a suite gap.
- **Not proved here.** Docker is not running on this machine; the suite's first evidence is its CI
  run, and the buyer deck cites it only after that.

**Pages touched:** plan row X2 (◐), `examples/units/README.md`, CHANGELOG, `Makefile`, `cluster-suites.yml`.
