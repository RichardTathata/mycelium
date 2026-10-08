## Mycelium — convenience targets

COMPOSE               = docker compose -f tests/integration/docker-compose.test.yml
COMPOSE_SCALE         = docker compose -f tests/integration/docker-compose.scale.yml
COMPOSE_RESILIENCE    = docker compose -f tests/integration/docker-compose.scale-resilience.yml
COMPOSE_SCALE_ENTRIES = docker compose -f tests/integration/docker-compose.scale-entries.yml
COMPOSE_LLM           = docker compose -f docker/docker-compose.yml
COMPOSE_LLM_DEMO      = docker compose -f docker/docker-compose.llm-agent.yml
COMPOSE_THREE_NODE    = docker compose -f docker/docker-compose.three-node-test.yml
COMPOSE_OVERLAY       = docker compose -f tests/overlay/docker-compose.test.yml
COMPOSE_FEDERATION    = docker compose -f docker/docker-compose.federation.yml

# The two-mesh federation suite's fixed test material (item 2 PR 10b). Seeds are in the clear on
# purpose: they are a fixture, not a secret. The public halves are derived — `make federation-keys`.
FED_ALPHA_SEED       ?= 1111111111111111111111111111111111111111111111111111111111111111
FED_BETA_SEED        ?= 2222222222222222222222222222222222222222222222222222222222222222
FED_ALPHA_PUBLIC_KEY ?= d04ab232742bb4ab3a1368bd4615e4e6d0224ab71a016baf8520a332c9778737
FED_BETA_PUBLIC_KEY  ?= a09aa5f47a6759802ff955f8dc2d2a14a5c99d23be97f864127ff9383455a4f0
FED_ENV               = ALPHA_PUBLIC_KEY=$(FED_ALPHA_PUBLIC_KEY) BETA_PUBLIC_KEY=$(FED_BETA_PUBLIC_KEY)

.PHONY: build check check-full test test-clean test-scale test-scale-clean test-scale-resilience test-scale-resilience-clean test-scale-entries test-scale-entries-clean test-llm-demo test-llm-agent test-three-node test-overlay test-confined-fleet llm-agent-interactive help

## test — build the cluster and run all integration scenarios
test:
	$(COMPOSE) down -v --remove-orphans 2>/dev/null || true
	$(COMPOSE) up -d --build
	@$(COMPOSE) logs -f runner & \
	EXIT=$$(docker wait mycelium-test-runner); \
	if [ "$$EXIT" != "0" ]; then \
	    echo "── runner failed: node logs (last 200 lines each, for CI diagnosis) ──"; \
	    $(COMPOSE) logs --tail 200 node-a node-b node-c mgmt 2>/dev/null || true; \
	fi; \
	$(COMPOSE) down -v --remove-orphans 2>/dev/null || true; \
	exit $$EXIT

## test-clean — tear down the test cluster and remove volumes
test-clean:
	$(COMPOSE) down -v --remove-orphans

## test-scale — 100-node cluster scale test (1 seed + 99 workers + mgmt + runner)
## Requires a warm Docker build cache (run `make test` first to prime it).
## Takes ~3 min: ~60 s cluster formation + 60 s gossip propagation window.
## Override SCALE_WORKERS to test at a different size: make test-scale SCALE_WORKERS=49
SCALE_WORKERS ?= 99
test-scale:
	$(COMPOSE_SCALE) down -v --remove-orphans 2>/dev/null || true
	$(COMPOSE_SCALE) up -d --build --scale worker=$(SCALE_WORKERS)
	@$(COMPOSE_SCALE) logs -f runner & \
	EXIT=$$(docker wait mycelium-scale-runner); \
	$(COMPOSE_SCALE) down -v --remove-orphans 2>/dev/null || true; \
	exit $$EXIT

## test-scale-clean — tear down the scale test cluster and remove volumes
test-scale-clean:
	$(COMPOSE_SCALE) down -v --remove-orphans

## test-scale-baseline — WS-B Phase 0: record the connection-ceiling "before" curve.
## Brings the scale cluster up at each worker count and captures seed ESTABLISHED,
## host conntrack, FORWARD-chain rule count, and seed /stats into
## tests/integration/baseline/scale-baseline.csv. Host-side (needs Docker socket +
## privilege for the --net=host VM probe). Long-running: one cluster up/down per point.
## Override the curve: make test-scale-baseline BASELINE_WORKERS="30 50 70 100"
BASELINE_WORKERS ?= 30 50 70 100
test-scale-baseline:
	BASELINE_WORKERS="$(BASELINE_WORKERS)" tests/integration/measure_scale_baseline.sh

## test-scale-resilience — crash/rejoin/anti-entropy/churn test (~22 nodes: 1 seed + 20 workers + mgmt)
## Tests: cluster formation, crash+recovery, late-joiner anti-entropy, and churn stability.
## Requires a warm Docker build cache and Docker socket access.  ~8 min on warm cache.
## Default is 20 workers — stays within the Docker bridge iptables connection limit so the
## Phase 3 late-joiner probe can establish a new TCP connection to seed (see CLAUDE.md §iptables).
## For higher scale (50+) switch the Docker network driver to macvlan or enable nftables first.
## Override: make test-scale-resilience RESILIENCE_WORKERS=50
RESILIENCE_WORKERS ?= 20
test-scale-resilience:
	$(COMPOSE_RESILIENCE) down -v --remove-orphans 2>/dev/null || true
	$(COMPOSE_RESILIENCE) up -d --build --scale worker=$(RESILIENCE_WORKERS)
	@$(COMPOSE_RESILIENCE) logs -f runner & \
	EXIT=$$(docker wait mycelium-resilience-runner); \
	$(COMPOSE_RESILIENCE) down -v --remove-orphans 2>/dev/null || true; \
	exit $$EXIT

## test-scale-resilience-clean — tear down the resilience test cluster and remove volumes
test-scale-resilience-clean:
	$(COMPOSE_RESILIENCE) down -v --remove-orphans

## test-scale-entries — entry-volume axis test (~30 nodes: 1 seed + 29 workers + mgmt)
## The 100-node test (test-scale) validates the node-count axis. This test
## validates the orthogonal entry-volume axis: load ENTRY_COUNT synthetic
## entries onto a 30-node cluster and measure convergence + anti-entropy
## sweep tail. 30 nodes deliberately stays well below the iptables ceiling
## so the runner can make new TCP connections throughout the test.
## Override examples:
##   make test-scale-entries ENTRY_COUNT=10000 ENTRY_BYTES=1024    # bytes-axis probe
##   make test-scale-entries ENTRY_COUNT=20000 WRITE_DELAY_MS=30   # sustained-rate sanity check (~10 min)
##   make test-scale-entries SCALE_ENTRIES_WORKERS=49              # wider cluster
SCALE_ENTRIES_WORKERS ?= 29
ENTRY_COUNT           ?= 5000
ENTRY_BYTES           ?= 512
WRITE_DELAY_MS        ?= 0
test-scale-entries:
	$(COMPOSE_SCALE_ENTRIES) down -v --remove-orphans 2>/dev/null || true
	ENTRY_COUNT=$(ENTRY_COUNT) ENTRY_BYTES=$(ENTRY_BYTES) WRITE_DELAY_MS=$(WRITE_DELAY_MS) \
	    $(COMPOSE_SCALE_ENTRIES) up -d --build --scale worker=$(SCALE_ENTRIES_WORKERS)
	@$(COMPOSE_SCALE_ENTRIES) logs -f runner & \
	EXIT=$$(docker wait mycelium-scale-entries-runner); \
	$(COMPOSE_SCALE_ENTRIES) down -v --remove-orphans 2>/dev/null || true; \
	exit $$EXIT

## test-scale-entries-clean — tear down the entry-volume test cluster and remove volumes
test-scale-entries-clean:
	$(COMPOSE_SCALE_ENTRIES) down -v --remove-orphans

## test-llm-demo — manual scenario: start the three_node_demo LLM cluster
## Requires Ollama installed locally. Open http://localhost:8080 to chat.
test-llm-demo:
	$(COMPOSE_LLM) up --build

## test-llm-agent — automated Docker test of the llm_agent example (MOCK_LLM=1)
## Builds the container, runs 6 scenarios, tears down. No Ollama needed.
test-llm-agent:
	$(COMPOSE_LLM_DEMO) down -v --remove-orphans 2>/dev/null || true
	$(COMPOSE_LLM_DEMO) up -d --build
	@$(COMPOSE_LLM_DEMO) logs -f runner & \
	EXIT=$$(docker wait mycelium-llm-agent-runner); \
	$(COMPOSE_LLM_DEMO) down -v --remove-orphans 2>/dev/null || true; \
	exit $$EXIT

## test-three-node — automated Docker test of three_node_demo with real Ollama
## Runs 4 scenarios: tool discovery, tool health, HTML UI, chat round-trip.
## Downloads llama3.2 (~2 GB) on first run; cached in the ollama-models volume.
test-three-node:
	$(COMPOSE_THREE_NODE) down --remove-orphans 2>/dev/null || true
	$(COMPOSE_THREE_NODE) up -d --build
	@$(COMPOSE_THREE_NODE) logs -f runner & \
	EXIT=$$(docker wait mycelium-three-node-runner); \
	$(COMPOSE_THREE_NODE) down --remove-orphans 2>/dev/null || true; \
	exit $$EXIT

## test-overlay — 3-node overlay cluster: task auction, leader election, shared log
## Builds Docker images, starts cluster, runs S11/S12/S13. ~3 min on warm cache.
test-confined-fleet: ## Boundary H7 deployment test: kind + Calico, deploy/confined-fleet (needs docker, kubectl, network)
	bash scripts/test-confined-fleet.sh

test-overlay:
	$(COMPOSE_OVERLAY) down -v --remove-orphans 2>/dev/null || true
	$(COMPOSE_OVERLAY) up -d --build
	@$(COMPOSE_OVERLAY) logs -f runner & \
	EXIT=$$(docker wait mycelium-overlay-runner); \
	$(COMPOSE_OVERLAY) down -v --remove-orphans 2>/dev/null || true; \
	exit $$EXIT

## llm-agent-interactive — start the llm_agent demo with real Ollama
## Open http://localhost:8100 for the mesh control UI.
llm-agent-interactive:
	MOCK_LLM=0 $(COMPOSE_LLM_DEMO) up --build llm-agent

## build — compile the library and the demo binary
build:
	cargo build --lib
	cargo build --example three_node_demo

## check — the pre-push gate. Runs clippy across the feature matrix that CI enforces, in ONE
## command, so the feature-gated dead-code trap (an item live only under gateway/metrics is *dead*
## under --no-default-features) is caught locally instead of turning a push CI-red. The
## --no-default-features clippy is the trap-catcher — it lints the same gateway/metrics-off mycelium
## lib that CI's "Gateway-free build" and "WASM host" jobs compile — so this is fast (~3 min, no
## wasmtime). Run it before every push.
check:
	cargo clippy --lib --tests -- -D warnings
	cargo clippy --lib --tests --features tls,metrics,a2a,llm -- -D warnings
	cargo clippy --lib --tests --features compliance -- -D warnings
	cargo clippy --lib --no-default-features -- -D warnings
	cargo clippy -p mycelium-core --lib --tests -- -D warnings
	cargo clippy -p mycelium-sim --all-targets -- -D warnings   # the replay harness (item 6)
	cargo clippy -p mycelium-core --lib --tests --features sim -- -D warnings  # the seams' OTHER arm
	cargo build --examples --features tls,metrics,a2a,llm       # CI builds these; `--lib --tests` does not
	./scripts/check-sim-seams.sh                                # no new nondeterminism outside the seams
	./scripts/with-pyyaml.sh scripts/check-test-inventory.py    # every test requirement runs in some CI step, with its features (verification policy rule 3)
	./scripts/with-pyyaml.sh scripts/test-check-test-inventory.py  # …and the check catches every bypass a review found
	./scripts/with-pyyaml.sh scripts/check-example-matrix.py     # examples/README.md's CI column matches what CI executes, both ways
	./scripts/with-pyyaml.sh scripts/test-check-example-matrix.py  # …and a ✓ row with no run, or a run with a · row, fails it
	python3 scripts/test-ci-test-coverage.py                    # the observed-coverage job's log parser (the record of rule 3)
	./scripts/test-ci-retest.sh                                 # the flake tier fails a crash, and a retry that ran nothing
	./scripts/check-kv-namespaces.sh                            # no foreign state in the gossip medium (D7)
	./scripts/check-wiki-mutation-fence.sh                      # every wiki mutation path stays inside the mandate boundary
	./scripts/check-positioning.sh                              # shared proposition, audience routes, resources and capability coverage
	python3 scripts/test-check-materials.py                      # missing links/coverage and claim regressions must fail

## gate-knowledge — the knowledge layer's SEMANTIC gate (item 3, a Phase D exit condition).
## Three negative cases — misleading evidence cannot erase a conflicting observation, cannot refresh
## expired evidence, cannot confer authority — plus the positive controls that stop a
## refuse-everything resolver from passing all three. This is NOT the behavioural claim (§13,
## research track): it establishes that three things are impossible, not that selection improves.
.PHONY: gate-knowledge
gate-knowledge:
	cargo test --lib --features tls,metrics,a2a,llm knowledge::gate -- --nocapture

## check-full — check + the main test suites + the (slow, wasmtime-heavy) wasm-host clippy; run before a
## release or when you have touched wasm-host / a feature-conditional path. A local subset of CI, not a
## mirror: the companions' suites, loom, the fuzz job and the TypeScript SDK run in CI only, and CI's
## coverage is what scripts/check-test-inventory.py proves. The Python line needs pytest and the two
## Python packages installed (pip install -e mycelium-py -e langgraph-checkpoint-mycelium pytest pytest-asyncio).
check-full: check
	cargo test  --lib --features tls,metrics,a2a,llm
	cargo test  --lib --features compliance,a2a   # the audit chain + both gateway enforcement points
	cargo test  --features tls,a2a --test '*'           # every root integration test, by discovery (rule 3)
	cargo test  --features tls,a2a --bins                # the skillrunner binary's unit tests
	cargo test  --features tls,a2a --doc                 # the crate's doctests (cargo will not mix --doc with other targets)
	cargo test  -p mycelium-wasm-host --features stem,gateway,llm   # rule_catalogue, signed-entry gateway, provisioner gateway, [[serve]]
	cargo test  -p mycelium-effects --features tuple-space,envelope --test '*'
	python3 -m pytest -q langgraph-checkpoint-mycelium/tests mycelium-py/tests   # the live suites skip without a node

	cargo test  --lib --no-default-features --features gateway
	cargo test  -p mycelium-gateway-free-tests   # the one test build of `mycelium` without `gateway` (R8)
	cargo test  -p mycelium-tls-free-tests       # …and the one with `gateway` and without `tls`
	cargo test  -p mycelium-sim           # the kernel + the same-length/different-content gate
	cargo test  -p mycelium-core --features sim   # the seams actually route through the kernel
	cargo test  -p mycelium-core --features tls   # erasure (crypto-shred), key extraction, framing's TLS cases
	cargo test  -p mycelium-core          # the substrate suite (codec/framing/hlc/store/swim) + the wire back-compat gate
	cargo clippy -p mycelium-wasm-host --all-targets -- -D warnings

## test-federation — item 2's release gate with **process isolation and a real network severance**
## (v3 item 2 PR 10b): two meshes under two CAs, one container per node, the consumer disconnected
## from and reconnected to the edge network by the runner (`docker network disconnect/connect`).
## The in-process choreography (`lib_tests.rs`) proves the same sequence but shares an address
## space and severs by stopping a gateway — that caveat is what this suite removes.
## X2 — the stem-examples suite: a co-op demo run as stem nodes from one image fed its declaration
## directory (docker/docker-compose.stem-examples.yml). `make test-stem-examples` runs every recut demo;
## `make examples-both-ways DEMO=provisioning` runs the code binary and the stem fleet and greps the
## same markers from both. The publisher key in the units is the public half of seed 42…42: `make stem-keys`.
COMPOSE_STEM   = docker compose -f docker/docker-compose.stem-examples.yml
STEM_DEMOS    ?= provisioning catalog catalog_store mcp_toolgrowth model_deploy reheal_deploy llm_agent
STEM_PUB_SEED ?= 4242424242424242424242424242424242424242424242424242424242424242
.PHONY: test-stem-examples list-stem-examples examples-both-ways stem-keys

# Verification policy rule 3: each demo is a case, printed as `@@case@@ <suite>::<demo>` when it starts;
# `make -s list-stem-examples` names them without Docker (cluster-suites.yml's stem job checks every one ran).
list-stem-examples:
	@for demo in $(STEM_DEMOS); do echo "@@case-list@@ Makefile:test-stem-examples::$$demo"; done

test-stem-examples:
	@set -e; for demo in $(STEM_DEMOS); do \
	    echo "@@case@@ Makefile:test-stem-examples::$$demo"; \
	    echo "== stem-examples: $$demo =="; \
	    docker rm -f mycelium-stem-late mycelium-stem-provider-c mycelium-stem-survivor >/dev/null 2>&1 || true; \
	    $(COMPOSE_STEM) --profile $$demo down -v --remove-orphans 2>/dev/null || true; \
	    $(COMPOSE_STEM) --profile $$demo up -d --build; \
	    $(COMPOSE_STEM) --profile $$demo logs -f driver-$$demo & \
	    EXIT=$$(docker wait mycelium-stem-driver-$$demo); \
	    if [ "$$EXIT" != "0" ]; then \
	        echo "-- driver failed: node logs (last 80 lines each) --"; \
	        for c in $$($(COMPOSE_STEM) --profile $$demo ps -a --format '{{.Name}}'); do echo "-- $$c"; docker logs --tail 80 $$c 2>&1 || true; done; \
	    fi; \
	    docker rm -f mycelium-stem-late mycelium-stem-provider-c mycelium-stem-survivor >/dev/null 2>&1 || true; \
	    $(COMPOSE_STEM) --profile $$demo down -v --remove-orphans 2>/dev/null || true; \
	    [ "$$EXIT" = "0" ] || exit $$EXIT; \
	done

examples-both-ways:
	@test -n "$(DEMO)" || { echo "usage: make examples-both-ways DEMO=<one of: $(STEM_DEMOS)>"; exit 2; }
	@if [ "$(DEMO)" = "llm_agent" ]; then \
	    echo "== code run: llm_agent is the browser demo (cargo run --example llm_agent), which asserts nothing; running the stem half only =="; \
	else \
	echo "== code run: $(DEMO) =="; \
	out=$$(cargo run -q -p mycelium-coop-examples --features wasm --bin $(DEMO) 2>&1); \
	echo "$$out" | grep -q "All assertions passed" || { echo "$$out" | tail -20; echo "code run failed"; exit 1; }; \
	echo "code run: All assertions passed"; \
	fi
	@$(MAKE) test-stem-examples STEM_DEMOS=$(DEMO)

stem-keys:
	@$(MAKE) -s federation-keys FED_ALPHA_SEED=$(STEM_PUB_SEED) FED_BETA_SEED=$(STEM_PUB_SEED) | head -1 | sed 's/^alpha/stem /'

## The public keys below are DERIVED from the suite's fixed test seeds: `make federation-keys`.
.PHONY: test-federation test-federation-clean federation-keys
test-federation:
	$(FED_ENV) $(COMPOSE_FEDERATION) down -v --remove-orphans 2>/dev/null || true
	$(FED_ENV) $(COMPOSE_FEDERATION) up -d --build
	@$(FED_ENV) $(COMPOSE_FEDERATION) logs -f runner & \
	EXIT=$$(docker wait mycelium-fed-runner); \
	if [ "$$EXIT" != "0" ]; then \
	    echo "-- runner failed: node logs (last 100 lines each, for CI diagnosis) --"; \
	    for c in alpha-a1 alpha-a2 alpha-gw1 alpha-gw2 beta-b1 beta-b2 beta-probe; do \
	        echo "-- $$c"; docker logs --tail 100 mycelium-fed-$$c 2>&1 || true; \
	    done; \
	fi; \
	$(FED_ENV) $(COMPOSE_FEDERATION) --profile plant down -v --remove-orphans 2>/dev/null || true; \
	exit $$EXIT

test-federation-clean:
	$(FED_ENV) $(COMPOSE_FEDERATION) --profile plant down -v --remove-orphans

## federation-keys — print the public halves of the suite's fixed seeds. They are pinned in this
## file (FED_ALPHA_PUBLIC_KEY / FED_BETA_PUBLIC_KEY); a mismatch would surface as `BadSignature`,
## which reads like a defect in the thing under test rather than a wrong fixture — so derive them.
federation-keys:
	@printf 'alpha %s\n' "$$(FED_ROLE=keys FED_SIGNING_SEED=$(FED_ALPHA_SEED) cargo run -q --example federation_node --features tls,a2a,cli)"
	@printf 'beta  %s\n' "$$(FED_ROLE=keys FED_SIGNING_SEED=$(FED_BETA_SEED) cargo run -q --example federation_node --features tls,a2a,cli)"

## help
help:
	@grep -E '^##' Makefile | sed 's/^## //'
