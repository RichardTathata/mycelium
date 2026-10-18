# Rule catalogue

**Generated** from the `RuleDescriptor`s each crate registers (`mycelium_core::rule`); do not edit. Regenerate with `UPDATE_RULE_CATALOGUE=1 cargo test -p mycelium-wasm-host --features stem --test rule_catalogue`. A rule is a decision point the substrate already has, described: what triggers it, what it reads, how it can end and why, what it writes, which guards it passes, how it may relate to other rules, where its code and tests are, and whether the decision trace records it. A descriptor executes nothing. A relationship is a hypothesis until a trace or a test shows it. The plan is `docs/plans/guarantees-and-rule-catalogue.md`.

Schema `mycelium.rules/1` · 29 rules.

| Rule | Responsibilities | Trace | Summary |
|---|---|---|---|
| [`a2a.admission`](#a2aadmission) | Admission, Authority | catalogue only | An absent credential is anonymous and a present bearer's scopes are dropped: a bearer does not gate this route, an evaluator does. |
| [`ae.preflight`](#aepreflight) | Authority | catalogue only | Permit, deny or indeterminate, recorded before the work runs; a refused mandate denies before policy runs, so a policy engine cannot launder a revocation. |
| [`cap.match`](#capmatch) | Response | catalogue only | The one matching rule, shared by the resolver and the offline check: namespace, name, schema id, typed attribute constraints; opacity skips, it does not unmatch. |
| [`cons.acceptor_memory`](#consacceptor_memory) | Response | catalogue only | An electorate member must keep its promises across a restart: no `[persistence]` is refused by name at every door, an empty store is warned once its genesis is known, and a peer's copy of this node's own record above its memory withholds the slot until the next start — detection, never prevention, and only of a copy that arrives before the next higher ballot. |
| [`gateway.auth`](#gatewayauth) | Authority | catalogue only | Deny by default under `compliance`: a route needs its scope; with no credential model the gateway is open — on loopback only, since a non-loopback one refuses to start unless `gateway_allow_unauthenticated` waives it — and a table the build cannot enforce refuses to start. |
| [`host.emit_admission`](#hostemit_admission) | Admission, Authority | catalogue only | A component emits only under `comp/{namespace}/…` or a kind the host listed for it, and never a protected RPC kind — an emit from inside the process would reach that kind's handlers unframed, past the door that checks authority. |
| [`kv.expiry`](#kvexpiry) | Propagation | catalogue only | An advertisement its writer stopped refreshing leaves the view after its TTL; a stale view is possible in between. |
| [`kv.propagation`](#kvpropagation) | Propagation | catalogue only | Every update converges by last-writer-wins on the HLC; propagation is unconditional and never taught a higher-layer law (detection, not prevention). |
| [`membership.governed`](#membershipgoverned) | Response | instrumented | This node rolls to join or leave a governed group against the intent's band — after spacing and settling, and (under an enforcing profile) only on a confident view. |
| [`prov.activation`](#provactivation) | Response | instrumented | After placement the declared command hands the blob to its local runtime — confined when the unit names a profile, which a node that cannot apply it refuses by name rather than run unconfined, as it refuses one whose egress proxy is not running; a failure is an install failure at stage `activation`, retried next round. |
| [`prov.activation_egress`](#provactivation_egress) | Authority | catalogue only | An activation's command reaches another host only through its proxy: a listed host by name, a credential attached by the node's gate where its binding authorises; anything else is refused with the proxy's marker, logged and counted per activation by reason (`mycelium_activation_egress_total{activation, event}`) — not recorded in the decision trace. |
| [`prov.advertise`](#provadvertise) | Propagation | catalogue only | A completed install advertises its capability to the mesh; the advertisement's lifetime is the install's. |
| [`prov.demand_response`](#provdemand_response) | Response | instrumented | A requirement with demanding nodes and no provider, as this node sees it, makes this node a candidate installer; an observed unmet requirement is not an instruction to install. |
| [`prov.eligible`](#proveligible) | Response | instrumented | An entry providing an `llm-meta` ad is never hosted (a model's metadata is its serving node's own, post-360 row R); otherwise this node hosts the entry's kind, has install budget left, and has memory and disk headroom for it. The check has side effects (counters) and is never re-evaluated for a record. |
| [`prov.health_pass`](#provhealth_pass) | Response | instrumented | A live install whose probe fails this round is withdrawn; restart ≡ provisioning brings it back. |
| [`prov.install`](#provinstall) | Response | instrumented | Fetch, verify, place and host the artifact under an install token; a superseded token tears its install down rather than advertising it. |
| [`prov.loadable`](#provloadable) | Authority | catalogue only | A proposed entry loads live only once a listed reviewer has accepted it; until then it may load into the shadow lane only (D20). |
| [`prov.presence_floor`](#provpresence_floor) | Response | instrumented | Fewer live providers than the declared floor, as this node sees it, makes this node a candidate installer of the cheapest loadable entry. |
| [`prov.probe`](#provprobe) | Response | instrumented | The probe gates the capability: a failing initial probe is an activation error (nothing advertised); a later failure flips the health flag the next health pass withdraws on. A confined re-probe re-resolves a profile whose declared path was replaced, every check repeated, and runs under the fresh one; only a refused re-resolution (`confinement_*`), or a path that changed under every attempt (`confinement_changed`), withdraws. Recorded under the install's token; a re-probe records only a change of verdict. |
| [`prov.promotion`](#provpromotion) | Response | instrumented | A shadow install whose entry a listed reviewer has since accepted is withdrawn from the shadow lane (D20). |
| [`prov.provenance`](#provprovenance) | Authority | catalogue only | An entry is a candidate only if a trusted publisher signed it; an empty trust list trusts everything, which the report says. |
| [`prov.rights_admission`](#provrights_admission) | Authority | instrumented | An install takes a right from this node's allocation first; under the enforcing profile a node with none left refuses, and records that it did. |
| [`prov.self_election`](#provself_election) | Response | instrumented | Herd damping: a candidate acts this round with probability p, drawn after the guards so a decline costs nothing. Replay-covered since the draw went through the seam. |
| [`prov.shed`](#provshed) | Response | instrumented | More live providers than the declared ceiling withdraws the hosts ranked beyond it by the band's rendezvous order, among the providers that advertise they will act on it (`prov-shed`; rev 2; rev 1 drew per host, and every host could withdraw at once). |
| [`prov.withdraw`](#provwithdraw) | Propagation, Response | catalogue only | Withdrawal removes the install and tombstones its advertisement; restart ≡ provisioning is how it comes back. |
| [`provider.enforcement`](#providerenforcement) | Authority | catalogue only | Authority where the work happens: a protected call is checked at the provider whichever door it came through; with enforcement on and no evaluator it is refused, with it off nothing is checked. |
| [`signal.admission`](#signaladmission) | Admission | partial — refusals and sheds only; an admitted signal is the hot path and records nothing | Admission is scoped (Cluster · Group · Individual) and shed under load — except Individual and the boundary transitions; shedding happens before the sender is recorded. |
| [`signal.forwarding`](#signalforwarding) | Propagation | catalogue only | Forwarding is unconditional (flood fallback); only admission is scoped, and a frame addressed to this node terminates here. |
| [`signal.suppression`](#signalsuppression) | Admission | catalogue only | A kind can be held until a time; held signals are released on flush. Suppression never changes forwarding. |

## `a2a.admission`

rev 1 · `mycelium::agent::a2a` · Admission, Authority · trace: catalogue only

An absent credential is anonymous and a present bearer's scopes are dropped: a bearer does not gate this route, an evaluator does.

- **Trigger:** a request on `/a2a`
- **Reads:**
  - `an optional bearer or federation credential` — scope: this request; freshness: local
- **Outcomes:**
  - Action: `identified`, `anonymous`
  - Refusal: `credential_invalid`
- **Effects:** a principal whose scopes are dropped; the AE preflight decides the rest
- **May trigger:** [`ae.preflight`](#aepreflight)
- **Code:** `http::a2a_optional_auth` · **Docs:** docs/operations/production-readiness.md · **Tests:** `a2a_is_anonymous_dispatch_whatever_bearer_is_configured`

## `ae.preflight`

rev 3 · `mycelium::agent::http` · Authority · trace: catalogue only

Permit, deny or indeterminate, recorded before the work runs; a refused mandate denies before policy runs, so a policy engine cannot launder a revocation.

- **Trigger:** a gateway dispatch that an evaluator covers
- **Reads:**
  - `the action envelope, the presented mandate, the deployed policy revision` — scope: this request; freshness: the decision clock
  - `the actor's recent decisions, when the evaluator asks for them (post-360 row K); carried on the evidence record` — scope: this enforcement point (its evidence journal); freshness: the journal, read in the background once an evaluator that asks for history is attached (unavailable until then), and kept current by every record appended
  - `from a routing door (post-360 row R), the destination decided on with its declared locality and the request's data class; carried on the evidence record with the destinations already refused` — scope: this request; the locality is the provider's own `llm-meta` declaration; freshness: the provider's ad as gossiped; the class as classified at the door
- **Outcomes:**
  - Action: `permit`
  - Refusal: `deny`, `indeterminate`, `mandate_revoked`, `mandate_expired`, `stale_policy`
  - NoAction: `inert_no_evaluator`
- **Effects:** the evidence journal record before dispatch; the audit chain reference under `compliance`
- **Guards:** Authority
- **Depends on:** [`gateway.auth`](#gatewayauth)
- **Code:** `http::ae_preflight / ae_record` · **Docs:** docs/guide/20-authorising-actions.md · **Tests:** `an_expired_envelope_is_denied_before_the_evaluator_runs`, `test_c7_the_bypass_matrix_no_door_runs_revoked_work`, `at_most_three_posts_in_ten_minutes_counted_at_this_gateway`, `a_confidential_request_to_an_external_destination_is_denied_naming_class_and_locality`

## `cap.match`

rev 1 · `mycelium::capability` · Response · trace: catalogue only

The one matching rule, shared by the resolver and the offline check: namespace, name, schema id, typed attribute constraints; opacity skips, it does not unmatch.

- **Trigger:** a resolve, a demand, the offline check, a group filter
- **Reads:**
  - `cap/` advertisements as gossiped — scope: the fleet as this node sees it; freshness: as gossiped; TTL-bounded
- **Outcomes:**
  - Action: `matched`
  - NoAction: `ns_or_name_differs`, `schema_id_differs`, `constraint_unmet`, `opaque_skipped`
- **May trigger:** [`prov.demand_response`](#provdemand_response)
- **Depends on:** [`kv.propagation`](#kvpropagation)
- **Code:** `capability::CapFilter::matches` · **Docs:** docs/guide/02-capabilities.md · **Tests:** `filter_matches_capability`, `constraint_matches_eq_ne`, `constraint_matches_ordering`

## `cons.acceptor_memory`

rev 1 · `mycelium::agent::acceptor_guard` · Response · trace: catalogue only

An electorate member must keep its promises across a restart: no `[persistence]` is refused by name at every door, an empty store is warned once its genesis is known, and a peer's copy of this node's own record above its memory withholds the slot until the next start — detection, never prevention, and only of a copy that arrives before the next higher ballot.

- **Trigger:** an electorate adoption, declaration or genesis answer; a committed genesis found at start or arriving by gossip; an inbound copy of this node's own acceptor record; every prepare and proposal for a slot
- **Reads:**
  - `[persistence]` in the node's configuration — scope: this node; freshness: fixed at start
  - the acceptor memory (`TaskCtx::consensus_accepted`), restored once at start — scope: this node; freshness: local, atomic
  - a peer's copy of `sys/consensus-accepted/{me}/{slot}` and `consensus/committed/electorate/…`, as they arrive — scope: one inbound update; freshness: as gossiped; may be stale or forged
- **Outcomes:**
  - Refusal: `persistence_required`, `slot_withheld`
  - Action: `memory_empty_warned`, `promise_forgotten_withheld`
  - NoAction: `not_a_member`, `copy_at_or_below_memory`, `genesis_not_decided`
- **Effects:** `TaskCtx::acceptor_withheld` (cleared only by a restart); the counters `electorate_persistence_refused`, `acceptor_memory_empty_for_electorate`, `acceptor_promises_forgotten`; never a Layer I write
- **Guards:** Other
- **Depends on:** [`kv.propagation`](#kvpropagation)
- **Code:** `acceptor_guard::{persistence_required, check_memory_for, install, withheld}` · **Docs:** docs/design/consensus-electorate.md · **Tests:** `a_member_without_persistence_is_refused_an_electorate_by_name`, `a_member_restarted_over_an_empty_store_is_warned_once_its_genesis_is_learned`, `a_forgotten_promise_withholds_the_slot_so_a_second_value_is_not_decided`

## `gateway.auth`

rev 1 · `mycelium::agent::http` · Authority · trace: catalogue only

Deny by default under `compliance`: a route needs its scope; with no credential model the gateway is open — on loopback only, since a non-loopback one refuses to start unless `gateway_allow_unauthenticated` waives it — and a table the build cannot enforce refuses to start.

- **Trigger:** every gateway request
- **Reads:**
  - the bearer or JWT presented, the token tables, `[oidc]` — scope: this node; freshness: local
- **Outcomes:**
  - Action: `admitted`, `anonymous_open_gateway`
  - Refusal: `authentication_required`, `insufficient_scope`
- **Effects:** the resolved principal and granted scopes travel to the handler
- **Guards:** Authority
- **May trigger:** [`ae.preflight`](#aepreflight)
- **Code:** `http::gateway_auth / required_scope` · **Docs:** docs/operations/rbac.md · **Tests:** `named_tokens_alone_still_close_the_gateway`, `a_token_table_this_build_cannot_enforce_refuses_to_start`, `an_exposed_gateway_with_no_credential_refuses_to_start`

## `host.emit_admission`

rev 1 · `mycelium-wasm-host::host` · Admission, Authority · trace: catalogue only

A component emits only under `comp/{namespace}/…` or a kind the host listed for it, and never a protected RPC kind — an emit from inside the process would reach that kind's handlers unframed, past the door that checks authority.

- **Trigger:** a hosted component calls its `mesh.emit(kind, payload)` import
- **Reads:**
  - the component's namespace, the kinds the host listed for it, and the node's `protected_rpc_kinds` — all set at instantiation — scope: this component instance; freshness: fixed at instantiation
- **Outcomes:**
  - Action: `admitted`
  - Refusal: `protected_kind`, `foreign_kind`, `malformed`
- **Effects:** the signal is emitted at cluster scope, or dropped — never sent — and counted: `mycelium_wasm_host_emits_refused_total{reason}`
- **Guards:** Authority
- **Code:** `host::HostState::emit` · **Docs:** docs/reference/unit-file.md · **Tests:** `a_component_cannot_emit_a_protected_kind_but_can_emit_in_its_own_namespace`, `a_protected_kind_is_refused_even_when_listed`

## `kv.expiry`

rev 1 · `mycelium-core::store` · Propagation · trace: catalogue only

An advertisement its writer stopped refreshing leaves the view after its TTL; a stale view is possible in between.

- **Trigger:** the GC tick, and every read
- **Reads:**
  - `the LWW store and its HLC` — scope: this node; freshness: local, atomic
  - `the entry's TTL and the wall clock through the seam` — scope: this node; freshness: the decision clock
- **Outcomes:**
  - Action: `expired`
  - NoAction: `live`, `no_ttl`
- **Effects:** the entry removed; a tombstone where the writer deleted
- **May trigger:** [`prov.presence_floor`](#provpresence_floor)
- **Code:** `store::Store::flush_expired` · **Docs:** docs/guide/01-gossip-kv.md · **Tests:** `flush_expired`, `is_expired`, `default_ttl_secs`

## `kv.propagation`

rev 1 · `mycelium-core::store / connection` · Propagation · trace: catalogue only

Every update converges by last-writer-wins on the HLC; propagation is unconditional and never taught a higher-layer law (detection, not prevention).

- **Trigger:** a local write, an incoming update, an anti-entropy round
- **Reads:**
  - `the LWW store and its HLC` — scope: this node; freshness: local, atomic
  - `an incoming wire frame` — scope: one peer connection; freshness: as received
- **Outcomes:**
  - Action: `applied_newer`, `forwarded`
  - NoAction: `older_or_equal`, `seen`
  - Refusal: `frame_too_large`, `signature_invalid`, `sender_removed`
- **Effects:** the store (LWW by HLC), the WAL after it, the gossip shards
- **Guards:** Provenance
- **May trigger:** [`signal.admission`](#signaladmission)
- **Code:** `store::apply_and_notify / connection::handle_connection` · **Docs:** docs/guide/01-gossip-kv.md · **Tests:** `gossip_spreads_membership_for_discovery`, `codec_matches_golden_wire_bytes`

## `membership.governed`

rev 1 · `mycelium::membership_governor` · Response · trace: instrumented

This node rolls to join or leave a governed group against the intent's band — after spacing and settling, and (under an enforcing profile) only on a confident view.

- **Trigger:** the governor tick
- **Reads:**
  - the governed group's roster and floor (`grp/`, `sys/`) — scope: the group as this node sees it; freshness: as gossiped; the confidence bound applies
  - `ViewConfidence (peers heard, staleness)` — scope: this node; freshness: computed at the tick
- **Outcomes:**
  - Action: `join`, `leave`
  - Deferral: `view_not_confident`, `settling`, `spacing`
  - NoAction: `hold`
- **Effects:** this node joins or leaves the group (`grp/`); the control ledger
- **Guards:** Settling, Cooldown
- **May trigger:** [`prov.presence_floor`](#provpresence_floor)
- **Depends on:** [`kv.propagation`](#kvpropagation)
- **Code:** `agent::membership_governor` · **Docs:** docs/guide/22-stability-and-control.md · **Tests:** `a_membership_decision_classifies_by_cost_not_by_verb`, `a_stale_membership_intent_stops_declaring_a_floor`, `healthy_membership_in_bounds_does_not_trip`

## `prov.activation`

rev 3 · `mycelium-wasm-host::activation` · Response · trace: instrumented

After placement the declared command hands the blob to its local runtime — confined when the unit names a profile, which a node that cannot apply it refuses by name rather than run unconfined, as it refuses one whose egress proxy is not running; a failure is an install failure at stage `activation`, retried next round.

- **Trigger:** a placed blob whose capability a `[[activation]]` names
- **Reads:**
  - the unit file (`[hosts]`, `[[presence]]`, `[[activation]]`) — scope: this node; freshness: read at start
  - the placed path, and `artifact:<hex>` references rendered to placed paths — scope: this node; freshness: local
  - the activation's egress proxy, started with the stem (`[activation.confine.egress]`) — scope: this node; freshness: local; started once, at the stem's start
- **Outcomes:**
  - Action: `activated`
  - Refusal: `command_failed`, `command_timed_out`, `reference_not_placed_yet`, `render_failed`, `confinement_unavailable`, `confinement_failed`, `egress_proxy_unavailable`
  - NoAction: `no_declaration`
- **Effects:** the operator's command — under its `[activation.confine]` profile (Seatbelt, or Landlock with seccomp) when the unit names one, else with the stem's privileges; with `[activation.confine.egress]`, confined to its egress proxy's port in a cleared environment
- **Guards:** Other
- **May trigger:** [`prov.probe`](#provprobe)
- **Depends on:** [`prov.install`](#provinstall)
- **Code:** `activation::hook` · **Docs:** docs/reference/unit-file.md · **Tests:** `a_declared_activation_runs_after_placement_and_its_probe_gates_the_capability`, `an_activation_resolves_artifact_references_to_placed_paths`, `a_verbose_command_that_succeeds_is_not_reported_as_a_timeout`, `a_hung_activation_is_recorded_as_a_timeout`, `a_profile_the_platform_cannot_apply_refuses_the_activation_by_name`, `a_confined_activation_reads_and_writes_only_what_its_profile_names`, `a_confined_activation_reaches_only_its_declared_tcp_ports`, `a_confined_activation_runs_in_a_stem_and_the_stem_says_which_applies`, `a_proxy_that_cannot_start_refuses_the_activation`

## `prov.activation_egress`

rev 1 · `mycelium-wasm-host::activation_egress` · Authority · trace: catalogue only

An activation's command reaches another host only through its proxy: a listed host by name, a credential attached by the node's gate where its binding authorises; anything else is refused with the proxy's marker, logged and counted per activation by reason (`mycelium_activation_egress_total{activation, event}`) — not recorded in the decision trace.

- **Trigger:** a request an egress-proxied activation's command sends through its proxy (absolute-form HTTP or `CONNECT`)
- **Reads:**
  - the activation's egress allow-list (`[activation.confine.egress] hosts`, replaced live by `Stem::set_activation_egress_hosts` / SIGHUP; lock-order row 64) — scope: this activation; freshness: read per request
  - the node's egress allow-list and credential bindings (`EgressCredentials`) — scope: this node; freshness: read at the agent's start
- **Outcomes:**
  - Action: `forwarded`, `tunnelled`
  - Refusal: `no_proxy_token`, `malformed_request`, `host_not_listed`, `credential_not_granted`, `credential_not_authorised`, `node_egress_denied`, `body_too_large`, `body_cap`, `connection_cap`, `preauth_evicted`, `peer_uid_other`, `peer_uid_unknown`, `open_cap`, `fd_budget`, `credential_host_tunnel`, `encoded_response`
- **Effects:** the request sent on over the entry's scheme with placeholders resolved by the node's gate, its answer relayed with the granted bindings' values masked; or a byte tunnel, closed when idle; `mycelium_activation_egress_total{activation, event}`
- **Guards:** Authority
- **Depends on:** [`prov.activation`](#provactivation)
- **Code:** `activation_egress::Inner::handle` · **Docs:** docs/reference/unit-file.md · **Tests:** `a_confined_command_reaches_an_allowed_host_through_its_proxy_and_is_refused_another`, `an_allow_list_change_takes_effect_without_a_restart`, `a_placeholder_is_replaced_only_at_the_bound_host_and_the_value_is_never_in_the_commands_environment`, `the_proxy_requires_its_token_and_limits_by_host`, `a_placeholder_the_activation_may_not_name_is_refused_and_never_sent`, `an_https_entry_gets_the_credential_over_tls_and_an_echo_of_it_is_masked`, `an_encoded_response_is_refused_where_a_value_could_hide_in_it`, `the_proxy_bounds_connections_header_reads_tunnels_and_bodies`, `token_less_connections_cannot_lock_the_command_out`, `a_tunnel_counts_against_the_connection_cap_until_it_closes`, `bodiless_requests_in_flight_are_not_counted_as_buffered_bodies`, `every_encoding_header_is_read_before_a_body_is_relayed`, `a_connection_another_user_owns_or_no_one_can_name_is_closed_at_accept`, `another_users_connection_is_closed_at_accept`, `a_bodiless_answer_or_repeated_identity_is_relayed`, `a_flood_of_other_users_connections_is_bounded_and_batched`, `a_proxy_holds_at_most_max_open_sockets`

## `prov.advertise`

rev 1 · `mycelium-wasm-host::provisioner` · Propagation · trace: catalogue only

A completed install advertises its capability to the mesh; the advertisement's lifetime is the install's.

- **Trigger:** an install that completed, before its state swaps to `Live`
- **Reads:**
  - the signed catalogue (`installable/` as gossiped, or the library manifest) — scope: the fleet as this node sees it; freshness: as gossiped; verified on read
- **Outcomes:**
  - Action: `advertised`
- **Effects:** `cap/` advertisement of the entry's capability, held by the install; tombstoned on withdrawal
- **May trigger:** [`prov.demand_response`](#provdemand_response)
- **Depends on:** [`prov.install`](#provinstall), [`prov.probe`](#provprobe)
- **Code:** `provisioner::Provisioner::start_install_as` · **Docs:** docs/guide/02-capabilities.md · **Tests:** `the_stem_fleet_fills_a_presence_floor_and_reheals`

## `prov.demand_response`

rev 1 · `mycelium-wasm-host::provisioner` · Response · trace: instrumented

A requirement with demanding nodes and no provider, as this node sees it, makes this node a candidate installer; an observed unmet requirement is not an instruction to install.

- **Trigger:** every provisioning round, per catalogue entry this node could host
- **Reads:**
  - capabilities().demand(filter) — `demand/` and `cap/` as gossiped — scope: the fleet as this node sees it; freshness: as gossiped; may be stale or partial
  - `Provisioner::hosted (lock-order row 21)` — scope: this node; freshness: local, atomic
  - the signed catalogue (`installable/` as gossiped, or the library manifest) — scope: the fleet as this node sees it; freshness: as gossiped; verified on read
  - the unit file (`[hosts]`, `[[presence]]`, `[[activation]]`) — scope: this node; freshness: read at start
- **Outcomes:**
  - Action: `unmet_demand_live`, `unmet_demand_shadow`
  - Deferral: `self_election_declined`, `already_started_this_round`
  - Refusal: `already_hosted`, `provenance_rejected`, `ineligible`, `rights_refused`
  - NoAction: `no_demand`, `provider_present`
- **Effects:** start_install (live) or the shadow lane · `mycelium_artifact_shadow_installs_total`
- **Guards:** Provenance, ResourceBudget, Authority
- **May trigger:** [`prov.install`](#provinstall)
- **Depends on:** [`prov.provenance`](#provprovenance), [`prov.loadable`](#provloadable), [`prov.eligible`](#proveligible), [`prov.self_election`](#provself_election), [`prov.rights_admission`](#provrights_admission)
- **Code:** `provisioner::Provisioner::provision_round` · **Docs:** docs/operations/capability-lifecycle.md · **Tests:** `a_hosting_stem_re_serves_its_verified_cache_to_a_late_joiner`, `a_proposed_entry_loads_only_into_the_shadow_lane_until_a_listed_reviewer_accepts_it`

## `prov.eligible`

rev 2 · `mycelium-wasm-host::provisioner` · Response · trace: instrumented

An entry providing an `llm-meta` ad is never hosted (a model's metadata is its serving node's own, post-360 row R); otherwise this node hosts the entry's kind, has install budget left, and has memory and disk headroom for it. The check has side effects (counters) and is never re-evaluated for a record.

- **Trigger:** a candidate entry that passed provenance
- **Reads:**
  - the signed catalogue (`installable/` as gossiped, or the library manifest) — scope: the fleet as this node sees it; freshness: as gossiped; verified on read
  - the unit file (`[hosts]`, `[[presence]]`, `[[activation]]`) — scope: this node; freshness: read at start
  - `ResourceProbe (memory, disk headroom)` — scope: this node; freshness: sampled at the decision
  - `Provisioner::hosted (lock-order row 21)` — scope: this node; freshness: local, atomic
- **Outcomes:**
  - Action: `eligible`
  - Refusal: `reserved_namespace`, `no_runtime`, `budget`, `memory`, `disk`
- **Effects:** `mycelium_artifact_ineligible_skips_total{reason}`
- **Guards:** ResourceBudget
- **May inhibit:** [`prov.demand_response`](#provdemand_response), [`prov.presence_floor`](#provpresence_floor)
- **Depends on:** [`prov.provenance`](#provprovenance)
- **Code:** `provisioner::Provisioner::eligible` · **Docs:** docs/operations/capability-lifecycle.md · **Tests:** `the_stem_fleet_fills_a_presence_floor_and_reheals`, `an_entry_providing_a_model_metadata_ad_is_never_installed`

## `prov.health_pass`

rev 1 · `mycelium-wasm-host::provisioner` · Response · trace: instrumented

A live install whose probe fails this round is withdrawn; restart ≡ provisioning brings it back.

- **Trigger:** every provisioning round, first
- **Reads:**
  - `Provisioner::hosted (lock-order row 21)` — scope: this node; freshness: local, atomic
  - each live install's `probe()` (file present, and the activation health flag) — scope: this node; freshness: sampled under row 21
- **Outcomes:**
  - Action: `probe_failed`
  - NoAction: `all_healthy`
- **Effects:** withdraw(): the capability's advertisement is tombstoned and the install removed · `mycelium_artifact_probe_withdrawals_total`
- **Guards:** Health
- **May trigger:** [`prov.presence_floor`](#provpresence_floor), [`prov.demand_response`](#provdemand_response)
- **Depends on:** [`prov.probe`](#provprobe)
- **Code:** `provisioner::Provisioner::provision_round` · **Docs:** docs/operations/capability-lifecycle.md · **Tests:** `a_declared_activation_runs_after_placement_and_its_probe_gates_the_capability`, `the_stem_fleet_fills_a_presence_floor_and_reheals`

## `prov.install`

rev 1 · `mycelium-wasm-host::provisioner` · Response · trace: instrumented

Fetch, verify, place and host the artifact under an install token; a superseded token tears its install down rather than advertising it.

- **Trigger:** an admitted install
- **Reads:**
  - the signed catalogue (`installable/` as gossiped, or the library manifest) — scope: the fleet as this node sees it; freshness: as gossiped; verified on read
  - `Provisioner::hosted (lock-order row 21)` — scope: this node; freshness: local, atomic
  - `the artifact bytes, by content address (library, mesh, or store)` — scope: wherever the source reaches; freshness: verified against the entry's hash
- **Outcomes:**
  - Action: `completed`
  - Refusal: `fetch`, `verify`, `place`, `activation`, `resources`, `host`
  - NoAction: `superseded`
- **Effects:** `HostedState::Installing` → `Live` under the install token · the `{ns}/loading` tier while bytes arrive · `mycelium_artifact_installs_started_total`, `_completed_total`, `_failed_total{stage}`
- **Guards:** Provenance
- **May trigger:** [`prov.activation`](#provactivation), [`prov.advertise`](#provadvertise)
- **Depends on:** [`prov.rights_admission`](#provrights_admission)
- **Code:** `provisioner::Provisioner::start_install_as` · **Docs:** docs/operations/capability-lifecycle.md · **Tests:** `the_stem_fleet_fills_a_presence_floor_and_reheals`, `an_installed_tool_component_is_bridged_as_an_mcp_tool`

## `prov.loadable`

rev 1 · `mycelium-wasm-host::catalog` · Authority · trace: catalogue only

A proposed entry loads live only once a listed reviewer has accepted it; until then it may load into the shadow lane only (D20).

- **Trigger:** a candidate entry
- **Reads:**
  - the signed catalogue (`installable/` as gossiped, or the library manifest) — scope: the fleet as this node sees it; freshness: as gossiped; verified on read
  - `[hosts].trusted_reviewers` — scope: this node; freshness: read at start
- **Outcomes:**
  - Action: `accepted`, `not_a_proposal`
  - Refusal: `proposed_unaccepted`
- **Guards:** Authority
- **Depends on:** [`prov.provenance`](#provprovenance)
- **Code:** `catalog::InstallableEntry::is_loadable` · **Docs:** docs/guide/16-guardrails.md · **Tests:** `a_proposed_entry_loads_only_into_the_shadow_lane_until_a_listed_reviewer_accepts_it`

## `prov.presence_floor`

rev 1 · `mycelium-wasm-host::provisioner` · Response · trace: instrumented

Fewer live providers than the declared floor, as this node sees it, makes this node a candidate installer of the cheapest loadable entry.

- **Trigger:** every provisioning round, per `[[presence]]` declaration
- **Reads:**
  - capabilities().demand(filter) — `demand/` and `cap/` as gossiped — scope: the fleet as this node sees it; freshness: as gossiped; may be stale or partial
  - `Provisioner::hosted (lock-order row 21)` — scope: this node; freshness: local, atomic
  - the signed catalogue (`installable/` as gossiped, or the library manifest) — scope: the fleet as this node sees it; freshness: as gossiped; verified on read
  - the unit file (`[hosts]`, `[[presence]]`, `[[activation]]`) — scope: this node; freshness: read at start
- **Outcomes:**
  - Action: `below_floor`
  - Deferral: `self_election_declined`
  - Refusal: `already_hosted`, `provenance_rejected`, `ineligible`, `no_loadable_candidate`
  - NoAction: `floor_met`
- **Effects:** start_install of the smallest loadable candidate
- **Guards:** Provenance, ResourceBudget
- **May trigger:** [`prov.install`](#provinstall)
- **Depends on:** [`prov.provenance`](#provprovenance), [`prov.loadable`](#provloadable), [`prov.eligible`](#proveligible), [`prov.self_election`](#provself_election)
- **Code:** `provisioner::Provisioner::provision_round` · **Docs:** docs/reference/unit-file.md · **Tests:** `the_stem_fleet_fills_a_presence_floor_and_reheals`

## `prov.probe`

rev 3 · `mycelium-wasm-host::activation` · Response · trace: instrumented

The probe gates the capability: a failing initial probe is an activation error (nothing advertised); a later failure flips the health flag the next health pass withdraws on. A confined re-probe re-resolves a profile whose declared path was replaced, every check repeated, and runs under the fresh one; only a refused re-resolution (`confinement_*`), or a path that changed under every attempt (`confinement_changed`), withdraws. Recorded under the install's token; a re-probe records only a change of verdict.

- **Trigger:** after activation, and on every re-probe tick
- **Reads:**
  - the unit file (`[hosts]`, `[[presence]]`, `[[activation]]`) — scope: this node; freshness: read at start
  - `the declared probe argv's exit status` — scope: this node; freshness: sampled
- **Outcomes:**
  - Action: `healthy`
  - Refusal: `initial_probe_failed`, `probe_failed`, `confinement_unavailable`, `confinement_failed`, `confinement_changed`
  - NoAction: `no_probe_declared`
- **Effects:** the install's health flag, which `prov.health_pass` reads
- **Guards:** Health
- **May trigger:** [`prov.health_pass`](#provhealth_pass)
- **May inhibit:** [`prov.advertise`](#provadvertise)
- **Depends on:** [`prov.activation`](#provactivation)
- **Code:** `activation::hook / activation::spawn_reprobe` · **Docs:** docs/reference/unit-file.md · **Tests:** `a_failing_initial_probe_is_an_activation_error`, `a_declared_activation_runs_after_placement_and_its_probe_gates_the_capability`, `a_replaced_write_directory_keeps_the_install_live_across_a_reprobe`, `a_declared_path_replaced_by_a_symlink_is_refused_at_the_reprobe_by_name`, `a_declared_path_swapped_during_the_probe_fails_closed`, `a_declared_path_swapped_during_the_initial_probe_fails_the_activation`, `a_symlink_planted_while_prepare_resolves_is_refused`, `a_flipped_ancestor_symlink_makes_the_profile_not_current`, `one_slow_reprobe_does_not_delay_the_others`, `an_intermediate_component_toggled_during_prepare_never_widens_the_profile`, `a_declared_path_spelt_in_another_case_or_normalisation_is_accepted`, `a_declared_fifo_socket_or_write_only_file_resolves_without_hanging`, `a_reprobe_round_runs_a_bounded_number_of_probes_at_once`, `an_operator_symlink_into_a_write_roots_interior_cannot_be_redirected`, `a_placement_directory_inside_a_write_root_cannot_be_redirected`, `a_symlink_on_a_second_mount_of_a_write_roots_filesystem_is_not_followed`

## `prov.promotion`

rev 1 · `mycelium-wasm-host::provisioner` · Response · trace: instrumented

A shadow install whose entry a listed reviewer has since accepted is withdrawn from the shadow lane (D20).

- **Trigger:** every provisioning round, after the health pass
- **Reads:**
  - `Provisioner::hosted (lock-order row 21)` — scope: this node; freshness: local, atomic
  - the signed catalogue (`installable/` as gossiped, or the library manifest) — scope: the fleet as this node sees it; freshness: as gossiped; verified on read
  - the unit file (`[hosts]`, `[[presence]]`, `[[activation]]`) — scope: this node; freshness: read at start
- **Outcomes:**
  - Action: `accepted_withdraw_shadow`
  - NoAction: `still_proposed`, `no_shadow_installs`
- **Effects:** withdraw() of the `{name}.shadow` install once the entry is accepted, so the live rule reinstalls it
- **Guards:** Provenance
- **May trigger:** [`prov.demand_response`](#provdemand_response)
- **Depends on:** [`prov.loadable`](#provloadable)
- **Code:** `provisioner::Provisioner::provision_round` · **Docs:** docs/guide/16-guardrails.md · **Tests:** `a_proposed_entry_loads_only_into_the_shadow_lane_until_a_listed_reviewer_accepts_it`

## `prov.provenance`

rev 1 · `mycelium-wasm-host::provisioner` · Authority · trace: catalogue only

An entry is a candidate only if a trusted publisher signed it; an empty trust list trusts everything, which the report says.

- **Trigger:** a candidate entry, before any other check
- **Reads:**
  - the signed catalogue (`installable/` as gossiped, or the library manifest) — scope: the fleet as this node sees it; freshness: as gossiped; verified on read
  - `[hosts].trusted_publishers` — scope: this node; freshness: read at start
- **Outcomes:**
  - Action: `signed_by_trusted_publisher`, `no_trust_list_configured`
  - Refusal: `signature_invalid`, `publisher_untrusted`
- **Guards:** Provenance
- **May inhibit:** [`prov.demand_response`](#provdemand_response), [`prov.presence_floor`](#provpresence_floor)
- **Code:** `provisioner::Provisioner::provenance_ok` · **Docs:** docs/operations/artifacts.md · **Tests:** `a_proposal_round_trips_as_v2_and_its_signature_is_its_own`, `the_stem_fleet_fills_a_presence_floor_and_reheals`

## `prov.rights_admission`

rev 1 · `mycelium-wasm-host::provisioner` · Authority · trace: instrumented

An install takes a right from this node's allocation first; under the enforcing profile a node with none left refuses, and records that it did.

- **Trigger:** an install about to start, when install rights are configured
- **Reads:**
  - `InstallRights::ledger (lock-order row 37)` — scope: this node's allocation; freshness: local; the head is published after
  - `Provisioner::hosted (lock-order row 21)` — scope: this node; freshness: local, atomic
- **Outcomes:**
  - Action: `admitted`
  - Refusal: `allocation_exhausted`
  - NoAction: `would_refuse_shadow_mode`, `no_rights_configured`
- **Effects:** the ledger's admit record and published head · `mycelium_artifact_installs_refused_by_rights_total`
- **Guards:** Authority, ResourceBudget
- **May inhibit:** [`prov.install`](#provinstall)
- **Code:** `provisioner::Provisioner::admit_install` · **Docs:** docs/guide/22-stability-and-control.md · **Tests:** `an_install_is_refused_and_recorded_when_the_node_holds_no_rights`

## `prov.self_election`

rev 1 · `mycelium-wasm-host::provisioner` · Response · trace: instrumented

Herd damping: a candidate acts this round with probability p, drawn after the guards so a decline costs nothing. Replay-covered since the draw went through the seam.

- **Trigger:** a candidate that passed every other check
- **Reads:**
  - `StemOptions::self_elect_p` and one draw from the `select` RNG stream (through the seam since 2026-10-03) — scope: this node; freshness: the draw
- **Outcomes:**
  - Action: `elected`
  - Deferral: `declined`
- **Guards:** Cooldown
- **May inhibit:** [`prov.demand_response`](#provdemand_response), [`prov.presence_floor`](#provpresence_floor)
- **Code:** `provisioner::Provisioner::self_elects` · **Docs:** docs/operations/capability-lifecycle.md · **Tests:** `the_stem_fleet_fills_a_presence_floor_and_reheals`

## `prov.shed`

rev 2 · `mycelium-wasm-host::provisioner` · Response · trace: instrumented

More live providers than the declared ceiling withdraws the hosts ranked beyond it by the band's rendezvous order, among the providers that advertise they will act on it (`prov-shed`; rev 2; rev 1 drew per host, and every host could withdraw at once).

- **Trigger:** every provisioning round, per `[[presence]]` declaration with a ceiling
- **Reads:**
  - capabilities().demand(filter) — `demand/` and `cap/` as gossiped — scope: the fleet as this node sees it; freshness: as gossiped; may be stale or partial
  - `Provisioner::hosted (lock-order row 21)` — scope: this node; freshness: local, atomic
  - `prov-shed/{ns}:{name}:{hash}` advertisements — which providers will act on this band's shed — scope: the fleet as this node sees it; freshness: as gossiped; may be stale or partial
  - when each unmarked peer was first seen unmarked (`Provisioner::unmarked_since`) — fixed only after FIXED_AFTER — scope: this node; freshness: local, monotonic clock through the replay seam
- **Outcomes:**
  - Action: `above_ceiling`
  - NoAction: `within_ceiling`, `not_hosting`, `ranked_within_ceiling`
- **Effects:** withdraw() of this node's install · `mycelium_artifact_presence_sheds_total` · `mycelium_artifact_presence_unranked_departures_total` and a warning when the band fell below its ceiling as unmarked providers left
- **May inhibit:** [`prov.presence_floor`](#provpresence_floor)
- **Code:** `provisioner::Provisioner::provision_round` · **Docs:** docs/reference/unit-file.md · **Tests:** `the_stem_fleet_fills_a_presence_floor_and_reheals`, `exactly_the_surplus_above_a_ceiling_withdraws`, `a_provider_that_does_not_shed_does_not_hold_the_band_above_its_ceiling`, `a_wrong_view_costs_at_most_its_wrong_entries_and_never_cascades`, `bands_that_differ_only_in_attributes_or_ceiling_have_different_shed_names`, `a_herd_of_new_installs_lands_on_the_ceiling`, `a_partition_heals_without_emptying_the_band`, `an_unmarked_peer_is_timed_from_first_sight_and_reset_by_its_mark`, `an_unmarked_provider_leaving_a_band_below_its_ceiling_is_named`, `the_departure_watch_sees_a_dip_that_arrives_in_two_steps`

## `prov.withdraw`

rev 1 · `mycelium-wasm-host::provisioner` · Propagation, Response · trace: catalogue only

Withdrawal removes the install and tombstones its advertisement; restart ≡ provisioning is how it comes back.

- **Trigger:** a health-pass failure, a promotion, a shed, or a superseded install
- **Reads:**
  - `Provisioner::hosted (lock-order row 21)` — scope: this node; freshness: local, atomic
- **Outcomes:**
  - Action: `withdrawn`
  - NoAction: `not_hosted`
- **Effects:** the install removed under row 21, then uninstalled outside it; the advertisement tombstoned
- **May trigger:** [`prov.presence_floor`](#provpresence_floor)
- **Code:** `provisioner::Provisioner::withdraw` · **Docs:** docs/operations/capability-lifecycle.md · **Tests:** `a_declared_activation_runs_after_placement_and_its_probe_gates_the_capability`

## `provider.enforcement`

rev 1 · `mycelium::agent::provider_enforcement` · Authority · trace: catalogue only

Authority where the work happens: a protected call is checked at the provider whichever door it came through; with enforcement on and no evaluator it is refused, with it off nothing is checked.

- **Trigger:** a protected RPC kind arriving at a provider with enforcement on
- **Reads:**
  - `the node's evaluator and execution authority; the carried mandate` — scope: this node; freshness: the decision clock
- **Outcomes:**
  - Action: `permit`
  - Refusal: `deny`, `no_evaluator`, `at_capacity`
  - NoAction: `enforcement_off`
- **Effects:** the evidence journal; the handler never runs on a refusal
- **Guards:** Authority, ResourceBudget
- **Code:** `provider_enforcement::check` · **Docs:** docs/guide/20-authorising-actions.md · **Tests:** `test_c7_the_bypass_matrix_no_door_runs_revoked_work`

## `signal.admission`

rev 1 · `mycelium-core::signal` · Admission · trace: partial — refusals and sheds only; an admitted signal is the hot path and records nothing

Admission is scoped (Cluster · Group · Individual) and shed under load — except Individual and the boundary transitions; shedding happens before the sender is recorded.

- **Trigger:** a signal arriving at this node, local or forwarded
- **Reads:**
  - the signal's scope and this node's boundary groups (`grp/`) — scope: this node; freshness: local
  - `the gossip shards' fill and the handlers' fill` — scope: this node; freshness: sampled
- **Outcomes:**
  - Action: `admitted`
  - Refusal: `scope_not_admitted`
  - Deferral: `shed_under_load`
- **Effects:** delivery to local handlers; the sender log
- **Guards:** Other
- **May trigger:** [`signal.suppression`](#signalsuppression)
- **Depends on:** [`signal.forwarding`](#signalforwarding)
- **Code:** `ops::deliver_locally` · **Docs:** docs/guide/03-signals.md · **Tests:** `individual_scope_bypasses_opacity`, `boundary_transition_signals_are_never_locally_shed`, `test_group_quorum_excludes_ex_member`

## `signal.forwarding`

rev 1 · `mycelium-core::ops` · Propagation · trace: catalogue only

Forwarding is unconditional (flood fallback); only admission is scoped, and a frame addressed to this node terminates here.

- **Trigger:** a signal emitted or received with TTL left
- **Reads:**
  - `an incoming wire frame` — scope: one peer connection; freshness: as received
- **Outcomes:**
  - Action: `forwarded`
  - NoAction: `ttl_exhausted`, `terminates_here`, `seen`
- **Effects:** the gossip shards
- **May trigger:** [`signal.admission`](#signaladmission)
- **Code:** `ops::emit_signal / forward_hint` · **Docs:** docs/wiki/dev/architecture/runtime-invariants.md · **Tests:** `forwarding_is_unconditional_through_non_member_relay`

## `signal.suppression`

rev 1 · `mycelium-core::signal` · Admission · trace: catalogue only

A kind can be held until a time; held signals are released on flush. Suppression never changes forwarding.

- **Trigger:** an admitted signal of a suppressed kind
- **Reads:**
  - `the suppression table` — scope: this node; freshness: local
- **Outcomes:**
  - Deferral: `suppressed_until`
  - Action: `delivered`, `released_on_flush`
- **Effects:** held signals delivered on flush
- **Guards:** Cooldown
- **Depends on:** [`signal.admission`](#signaladmission)
- **Code:** `signal::SignalHandlers::deliver / flush_expired` · **Docs:** docs/guide/03-signals.md · **Tests:** `is_suppressed`, `is_suppressed_at`, `flush_expired_delivers_held_signals`
