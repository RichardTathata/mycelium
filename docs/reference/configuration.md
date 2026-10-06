# Configuration — who owns each field

↑ [Docs map](../README.md) · the guarantees each setting resolves to: [guarantee catalogue](guarantee-catalogue.md)

**Traced at `main` `f67b3f37` (2026-10-05)** — realignment repairs A2, `docs/plans/realignment-repairs.md` §3.4,
decision D3. One row per `GossipConfig` field (`mycelium-core/src/config.rs`): which part of the system
consumes it, the Cargo feature it needs to have any effect, **where an unsupported or invalid value is
refused by name — or that it is not**, whether changing it needs a restart, and its default and
environment variable. Every row cites `file:line` where it matters; a line number drifts, the function
it names does not.

The table answers operational questions a type definition cannot: *will this setting do anything in my
build, will a bad value stop the node or be quietly ignored, can I change it live.* It is not a
`CoreConfig` split — that is decision D3's deferred question; this is the information a split would need.

## Conventions

Conventions:
- "validate()" = refused by name in `GossipConfig::validate` (called by `start()` at lifecycle.rs:34 AND by `load_from_file`).
- **Zero-sentinel caveat (Z):** `GossipAgent::new` runs `derive_unset` *before* `start()`'s `validate()` (mod.rs:880), so a `0` in `default_ttl`, `writer_channel_depth`, `max_seen_entries`, `ping_peer_sample_size`, `propagation_window_secs` is **derived, not refused** on the agent path; validate's zero refusal for these fires only via `load_from_file` (which validates before any derive — so a TOML/env `0` meant as "auto" is refused there). `gossip_shards` is rounded with `next_power_of_two()` in new() (mod.rs:883), so `0` becomes `1` and is never refused on the agent path.
- "profile" = refused at `start()` only under `profile = "secure-single-domain"` via `guarantee::check` (lifecycle.rs:138), naming the unmet guarantee id. That is the **guarantee profile**; the control profile is set at runtime (`set_control_profile`, `POST /gateway/govern/profile`) and is not a field here, and `domain_profile` is a field of its own — the table of things called *profile* is in [`operations/control-profiles.md`](../operations/control-profiles.md).
- restart: `GossipConfig` lives in an immutable `Arc` (mod.rs:914); re-reading `ctx.config` does not make a field changeable. The only runtime-changeable fields are the 5 in `HotConfig` (setters `src/agent/introspect.rs:44-78`, also moved by `ClusterTuner` cluster_tuner.rs:78-82 and the `TimingIntent` reconciler timing_governor.rs:73-90).
- "parse" = serde/TOML (`GossipError::Toml`) or `apply_env_overrides` (`GossipError::Parse` / `InvalidField`).

## The fields

| field | component | feature | refused where | restart | default (env) | notes |
|---|---|---|---|---|---|---|
| bind_address | core transport/state | — | validate() (empty / not an IP, config.rs:1434-1440); start() re-parse + `NodeIdMismatch` if ≠ node_id (lifecycle.rs:142-152) | yes | `"127.0.0.1"` (GOSSIP_BIND_ADDRESS) | listener bind lifecycle.rs:526 |
| bind_port | core transport/state | — | validate() (0, :1443); validate() `FieldConflict` (:1572) with http_port; start() NodeIdMismatch | yes | `8080` (GOSSIP_BIND_PORT) | also SWIM UDP port when swim_udp_port=None (lifecycle.rs:605) |
| cluster_name | gateway | — (surfaced only via gateway /stats, metrics label) | not refused (any string; env blank → None, config.rs:1683) | yes | `None` (GOSSIP_CLUSTER_NAME) | label only; http.rs:195 (metrics), :946 (/stats), introspect.rs:32 getter |
| bootstrap_peers | core transport/state | — | parse (NodeId Deserialize; env `InvalidField GOSSIP_BOOTSTRAP_PEERS`, config.rs:1731-1740) | yes | `[]` (GOSSIP_BOOTSTRAP_PEERS, comma `ip:port`) | self filtered out in new() (mod.rs:897); also N estimate for derive_unset; validate() only *warns* if >20 with uncapped forwarding |
| propagation_window_secs | core transport/state | — | validate() (0, :1489) — see Z: derived on agent path | yes | `60`; `auto()`=0→`max(60, health×eviction×2)` (GOSSIP_PROPAGATION_WINDOW_SECS) | GC task lifecycle.rs:681; audit_invariants warns if < eviction window |
| health_check_interval_secs | core transport/state | — | validate() (0, >3600, :1474-1480) | **no** — `set_health_check_interval_secs` (introspect.rs:63) + `TimingIntent` (timing_governor.rs:73); health monitor re-reads hot value per cycle | `10` (GOSSIP_HEALTH_CHECK_INTERVAL_SECS) | the runtime setter refuses a value above 3600 (`InvalidField`, nothing applied or pinned; 0 = revert to static), the bound `TimingIntent` also applies (R9). Static value still read by consensus_handle.rs:213, membership cooldown default |
| membership_cooldown_secs | control | — | not refused — **clamped** to ≥1 s (`membership_cooldown()` config.rs:1667) | yes (doc says so, read at governor start membership_governor.rs:350) | `None` = 3 × health_check_interval_secs (GOSSIP_MEMBERSHIP_COOLDOWN_SECS) | |
| default_ttl | core transport/state | — | validate() (0, :1485) — see Z | yes | `5`; `auto()`=0→`max(5,⌈log2(N+1)⌉)` (GOSSIP_DEFAULT_TTL) | copied into CoreCtx.default_ttl at new() (mod.rs:911) |
| max_connections | core transport/state | — | validate() (0, >65535, :1463-1467) | yes | `1024` (GOSSIP_MAX_CONNECTIONS) | Semaphore at listener start lifecycle.rs:544 |
| writer_channel_depth | core transport/state | — | validate() (0, :1495; <64 warn only) — see Z | **no** (new writers only) — `set_writer_channel_depth` (introspect.rs:50, clamps ≥1) + ClusterTuner | `1024`; `auto()`=0→`max(1024, N×4)` (GOSSIP_WRITER_CHANNEL_DEPTH) | hot read tasks.rs:254/613; existing writers keep their channel |
| max_forwarding_peers | core transport/state | — | not refused (validate() warn only, :1557) | yes | `i64::MAX as usize` (GOSSIP_MAX_FORWARDING_PEERS) | shard ctx lifecycle.rs:571 |
| reconnect_backoff_secs | core transport/state | — | validate() (0, >300, :1528-1534) | **no** (new connections only) — `set_reconnect_backoff_secs` (introspect.rs:74) + TimingIntent (timing_governor.rs:78) | `5` (GOSSIP_RECONNECT_BACKOFF_SECS) | the setter refuses a value above 300 (R9); existing writer keeps backoff captured at spawn (doc introspect.rs:70) |
| gossip_channel_capacity | core transport/state | — | validate() (0, :1507) | yes | `1024` (GOSSIP_GOSSIP_CHANNEL_CAPACITY) | mpsc capacity in new() mod.rs:882 |
| max_seen_entries | core transport/state | — | validate() (0, :1513) — see Z | yes | `100_000`; `auto()`=0→`max(100k, N×1000)` (GOSSIP_MAX_SEEN_ENTRIES) | GC lifecycle.rs:682 |
| peer_eviction_intervals | core transport/state | — | validate() (0, :1519) | yes | `3` (GOSSIP_PEER_EVICTION_INTERVALS) | health monitor lifecycle.rs:657 |
| gossip_shards | core transport/state | — | validate() (0, :1524) but **silently rounded up to next power of two in new()** (mod.rs:883), so 0→1 on the agent path | yes | `min(available_parallelism or 4, 16)` (GOSSIP_GOSSIP_SHARDS) | explicit values are not capped at 16 |
| intern_keys | core transport/state | — | parse (env non-bool → `InvalidField GOSSIP_INTERN_KEYS`) | yes | `true` (GOSSIP_INTERN_KEYS) | listener + WAL replay lifecycle.rs:175/536 |
| intern_max_keys | core transport/state | — | not refused (0 = unlimited) | yes | `0` (GOSSIP_INTERN_MAX_KEYS) | |
| ping_peer_sample_size | core transport/state | — | validate() (0, :1542) — see Z | yes | `20`; `auto()`=0→`min(N, max(20,√N))` (GOSSIP_PING_PEER_SAMPLE_SIZE) | health monitor lifecycle.rs:658, connection.rs:333 |
| tcp_accept_backlog | core transport/state | — | validate() (0, :1548) | yes | `1024` (GOSSIP_TCP_ACCEPT_BACKLOG) | lifecycle.rs:526 |
| max_peers | core transport/state | — | validate() (0, :1554) | yes | `i64::MAX as usize` (GOSSIP_MAX_PEERS) | lifecycle.rs:538 |
| max_active_connections | core transport/state | — | not refused (0 = unlimited) | yes | `0` (GOSSIP_MAX_ACTIVE_CONNECTIONS) | hard ceiling on resolved_fanout; lifecycle.rs:618/659 |
| gossip_fanout | core transport/state | — | not refused (0 = auto; explicit capped at known peers by `resolved_fanout`) | yes | `0` (GOSSIP_FANOUT) | |
| swim_failure_detector | core transport/state | — | validate() refuses `true` under `domain_profile = Enforced` (:1417); parse (env non-bool refused) | yes | `true` (GOSSIP_SWIM_FAILURE_DETECTOR) | lifecycle.rs:365 starts UDP listener; mixed on/off fleet hazard documented only |
| swim_udp_port | core transport/state | — | not refused (`Some(0)` or a port clashing with http_port is not checked); bind failure is a start() error (generic) | yes | `None` = bind_port (GOSSIP_SWIM_UDP_PORT) | ignored when SWIM off; lifecycle.rs:605 |
| swim_probe_interval_ms | core transport/state | — | validate() (0 when SWIM on, block :1445-1460) | yes | `500` (GOSSIP_SWIM_PROBE_INTERVAL_MS) | prober also max(1ms) |
| swim_probe_timeout_ms | core transport/state | — | validate() (0 when SWIM on) | yes | `300` (GOSSIP_SWIM_PROBE_TIMEOUT_MS) | |
| swim_indirect_probes | core transport/state | — | not refused (0 accepted) | yes | `3` (GOSSIP_SWIM_INDIRECT_PROBES) | lifecycle.rs:635 |
| swim_gossip_updates | core transport/state | — | not refused (0 accepted) | yes | `12` (GOSSIP_SWIM_GOSSIP_UPDATES) | lifecycle.rs:615 |
| swim_suspicion_timeout_ms | core transport/state | — | validate() (0 when SWIM on) | yes | `4000` (GOSSIP_SWIM_SUSPICION_TIMEOUT_MS) | |
| writer_idle_timeout_secs | core transport/state | — | not refused (0 = never) | yes | `30` (GOSSIP_WRITER_IDLE_TIMEOUT_SECS) | lifecycle.rs:539/570/656 |
| group_aware_forwarding | core transport/state | — | parse (env non-bool refused) | yes | `true` (GOSSIP_GROUP_AWARE_FORWARDING) | lifecycle.rs:572 |
| epidemic_extra_peers | core transport/state | — | not refused | yes | `3` (GOSSIP_EPIDEMIC_EXTRA_PEERS) | lifecycle.rs:573 |
| health_check_max_jitter_ms | core transport/state | — | not refused (0 = half interval) | yes | `0` (GOSSIP_HEALTH_CHECK_MAX_JITTER_MS) | lifecycle.rs:662 |
| signal_window_secs | core transport/state | — | not refused (0 accepted — every pheromone entry immediately stale) | yes | `600` (GOSSIP_SIGNAL_WINDOW_SECS) | SignalHandlers::new at new() mod.rs:906; lifecycle.rs:699 |
| signal_ordered_delivery | core transport/state | — | not refused | yes | `false` (no env) | reorder buffer built only at new() (mod.rs:942) |
| signal_reorder_max_hold_ms | core transport/state | — | not refused (0 accepted) | yes | `500` (no env) | only when ordered delivery on |
| signal_reorder_max_depth | core transport/state | — | not refused (0 accepted; behaviour of depth 0 in SignalReorderBuffer not checked → ?) | yes | `64` (no env) | |
| max_store_entries | core transport/state | — | not refused (0 = unlimited) | yes | `0` (GOSSIP_MAX_STORE_ENTRIES) | KvState::new at new() mod.rs:909; live writes over cap silently refused (store.rs:~565, `LocalApplication::Refused`) |
| max_clock_drift_ms | core transport/state | — | not refused (0 = bound disabled, documented) | yes | `300_000` = `DEFAULT_MAX_CLOCK_DRIFT_MS` (GOSSIP_MAX_CLOCK_DRIFT_MS) | `Hlc::with_max_drift` at new() mod.rs:926 |
| locality_path | core transport/state | — | validate() (empty segment, :1590) | yes | `[]` (GOSSIP_LOCALITY_PATH) | advertised once at start lifecycle.rs:729 |
| topology_policies | control (consensus) | consensus | validate() (Hard needs spread_depth and spread_min_distinct ≥2, :1599-1605) | yes | `{}` (no env) | only reader consensus_handle.rs:259 (module `#[cfg(feature="consensus")]` mod.rs:32); in a no-consensus build accepted, validated, ignored |
| http_port | gateway | gateway | validate() (0; == bind_port; :1568-1572); start() refuses a bind failure by name `gateway` (lifecycle.rs:384) | yes (bulk advertise port alone: `set_bulk_serving_port` service_handle.rs:135) | `None` (GOSSIP_HTTP_PORT) | in a build without `gateway`, `start()` refuses it by name (2.25.0, R8; `lifecycle.rs`) — it used to be ignored while still copied into BulkTransport (mod.rs) and advertised in bulk tickets |
| http_addr | gateway | gateway | validate() (empty / not IP, only when http_port set, :1578-1582); start() re-parse (lifecycle.rs:372) | yes | `"127.0.0.1"` (GOSSIP_HTTP_ADDR) | |
| gateway_tls | gateway | tls (+gateway) | start(): refused by name without `tls`, and without `gateway` (2.25.0) (lifecycle.rs); bad cert/key refused at start via `prepare_gateway` → field `gateway` (http.rs:175, lifecycle.rs:384); profile (`gw.tls`) | yes | `None` (no env) | **with `tls` but no `http_port` it is silently ignored** (not refused) |
| persistence | core transport/state | — | validate() (snapshot_wal_threshold/interval 0, :1614-1620; unwritable base_path **warn only**); start(): ownership lock, unreadable replay (`on_unreadable=refuse`), failed startup snapshot all refused as field `persistence` (lifecycle.rs:206-279); profile (persist.*) | yes | `None` (no env) | **since R7 (#529) an uncreatable data directory refuses `start()` by name**; before it, start() warned and ran in memory while `persist.configured` still read `enforced` |
| bulk_fetch_timeout_secs | gateway | gateway | not refused (0 accepted → zero reqwest timeout; effect not tested → ?) | yes | `30` (no env) | used only for the reqwest client `#[cfg(feature="gateway")]` (bulk.rs:85, mod.rs:984) |
| max_concurrent_bulk_handlers | gateway | gateway (bulk_serve) | not refused (0 = unlimited) | **no** — `set_max_concurrent_bulk_handlers` (introspect.rs:55) + ClusterTuner | `64` (GOSSIP_MAX_CONCURRENT_BULK_HANDLERS) | sampled per admission bulk.rs:274 |
| max_inbound_frames_per_sec | core transport/state | — | not refused (0 = unlimited) | **no** — `set_max_inbound_frames_per_sec` (introspect.rs:44) + ClusterTuner | `0` (GOSSIP_MAX_INBOUND_FRAMES_PER_SEC) | sampled per inbound frame; note rate.rs:39 derives the M7 default threshold from the *static* `ctx.config` value, not the hot one |
| rate_observation_enabled | control | — | not refused — env: any value other than `1/true/TRUE/yes` silently = false (config.rs:1818) | yes | `false` (GOSSIP_RATE_OBSERVATION) | decider spawned at start lifecycle.rs:424; connection.rs:139 |
| rate_aggregate_threshold_fps | control | — | not refused (0 = 8×max_inbound or 8000) | yes | `0` (GOSSIP_RATE_AGGREGATE_THRESHOLD_FPS) | rate.rs:35 |
| emergent_detectors_enabled | control | — | not refused — env lenient bool as above | yes | `false` (GOSSIP_EMERGENT_DETECTORS) | loops spawned at start lifecycle.rs:431; results surfaced on gateway /stats |
| gateway_auth_token | gateway | gateway | not refused (empty string accepted: `GOSSIP_GATEWAY_AUTH_TOKEN=""` → `Some("")`, config.rs:1882; whether an empty bearer is then admitted depends on header whitespace handling → ?) | yes | `None` (GOSSIP_GATEWAY_AUTH_TOKEN) | gateway_auth http.rs:484, resolve_token :674; profile (`gw.not_open`) — that guarantee resolves on `is_some()`, so `Some("")` counts as closed |
| gateway_scoped_tokens | gateway | compliance | validate() (family wildcard scope, :1397); start() refused by name without `compliance` **only in a gateway build** (`cfg(all(gateway, not(compliance)))`, lifecycle.rs:40-55) | yes | `[]` (no env) | in a no-gateway build accepted and ignored (harmless — no gateway) |
| gateway_named_tokens | gateway | compliance | parse (env `InvalidField`, `parse_named_tokens_env` config.rs:1146); validate() (family wildcard); start() without `compliance` (gateway builds) | yes | `[]` (GOSSIP_GATEWAY_NAMED_TOKENS, `name\|token\|scope,…;…`) | |
| gateway_identity_issuer | gateway | gateway | not refused (any string, incl. empty) | yes | `None` = node id (GOSSIP_GATEWAY_IDENTITY_ISSUER) | http.rs:592 |
| require_identity_proofs | deployment | tls | not refused — **inert without `[tls]`** (readers inside `#[cfg(feature="tls")]` prewarm/identity watcher, lifecycle.rs:767/797, run only when `config.tls` set); env lenient bool; profile (`id.proofs_required`, guarantee.rs:669-672) | yes | `false` (GOSSIP_REQUIRE_IDENTITY_PROOFS) | |
| gateway_caller_profile | gateway | gateway | parse (serde enum / env `InvalidField`, config.rs:1836); profile (`gw.caller_profile` needs `secure` + tls, guarantee.rs:643-647); `legacy` warns at start (lifecycle.rs:484) | yes | `Secure` (GOSSIP_GATEWAY_CALLER_PROFILE = secure\|legacy) | gateway_caller.rs:306/415 |
| protected_rpc_kinds | gateway | gateway | not refused (any strings) | yes | `[]` (GOSSIP_PROTECTED_RPC_KINDS, comma) | `is_protected_kind` http.rs:2172; provider_enforcement.rs:380 (gateway+tls) |
| control_max_staleness_ms | control | — | validate() (0, :1368) | yes | `30_000` (GOSSIP_CONTROL_MAX_STALENESS_MS) | `ConfidenceBound::from_config` read by governors from immutable config (membership_governor.rs:360, opacity.rs:391, cluster_tuner.rs:133) |
| control_min_peers_heard | control | — | validate() (0, :1374) | yes | `1` (GOSSIP_CONTROL_MIN_PEERS_HEARD) | same |
| domain_profile | deployment | — (Enforced effectively needs tls) | parse (env `InvalidField`, config.rs:1862); validate() refuses Enforced with SWIM on or no `tls` (:1417-1426) | yes | `Open` (GOSSIP_DOMAIN_PROFILE) | **no runtime reader** outside validate() — its only effect is these two refusals |
| profile | deployment | — (secure-single-domain is only satisfiable with gateway+tls+compliance) | validate() (unknown name, :1410); start() `guarantee::check` refuses unmet required ids as field `profile` (lifecycle.rs:136-141) | yes | `None` = dev (GOSSIP_PROFILE) | required set guarantee.rs:262-292 (rev 2, 17 ids) |
| egress | deployment | gateway (all consumers are outbound HTTP clients: mcp/llm/probes/federation/oidc) | start(): only the OIDC-issuer/JWKS contradiction is refused, field `oidc` (lifecycle.rs:76-92, compliance); allow_hosts entries themselves not validated | yes (`egress_policy()` returns `&` to frozen config, mod.rs:1508) | `allow_hosts: []` = allow all (no env) | guarantee `egress.allow_list` (guarantee.rs:807) |
| oidc | gateway | compliance | start(): refused by name without `compliance` in **every** build (`cfg(not(compliance))`, lifecycle.rs:60-70; non-compliance type `OidcNotInBuild` keeps it parseable); validate() (group_scopes family wildcard, compliance); start() egress conflict | yes | `None` (no env) | verifier built at gateway start http.rs:580 |
| tls | deployment (transport identity) | tls | start(): refused by name without `tls` (lifecycle.rs:104-115); `load_or_generate` errors returned from start (lifecycle.rs:291-345); validate() requires it under `domain_profile = Enforced`; profile (`mesh.tls`, `id.ca_key_off_node`, ...) | yes | `None` (no env) | audit sink without tls refused (lifecycle.rs:94-103) |

## What tracing it found

Writing this table down found three defects, recorded in the plan and all fixed:

- **R7 (fixed, #529):** a persistence directory that could not be created let the node start **in
  memory**, while `persist.configured` — resolved from the configuration alone — still said
  `enforced`, and `secure-single-domain` admitted the node.
- **R8 (fixed):** `http_port` and `gateway_tls` were silently ignored in a build without `gateway`, and
  the `gw.*` guarantees resolved from config there. `start()` now refuses each by name, the `gw.*`
  guarantees read `not_in_build`, and `mycelium-gateway-free-tests` is the first test build of
  `mycelium` without the gateway.
- **R9 (fixed):** the runtime timing setters bypassed `validate()`'s bounds; they now refuse a value
  above them, changing nothing — the bounds the cluster timing governor already applied.

Also worth knowing, not defects in themselves: a `0` in five fields is *derived* on the agent path but
*refused* through `load_from_file` (the zero-sentinel caveat above); boolean environment variables
disagree on an unrecognised value (three read it as `false`, three refuse it); `domain_profile` is read
only by `validate()`.
