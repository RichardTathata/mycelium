//! The adapter contract (docs/plans/adopter-surface.md, X7 and X8): what each runtime adapter carries, by
//! property, and the test that shows it.
//!
//! One row per adapter — the native Rust embedding, `mycelium-py`, `mycelium-ts`, `langgraph-checkpoint-mycelium`,
//! `strands-session-mycelium`, `mycelium-reason`, the A2A door, the MCP door — and one cell per property: session
//! persistence, cancellation, tracing (X4's correlation and parent), identity propagation, grants (X3),
//! provider-side enforcement, receipts. A cell is *carried* and names the test CI runs that shows it, *not carried*
//! with the reason (and the adapter's own *What this does not establish* section), or *not applicable* with the
//! reason. **The table records; it does not make an adapter carry anything.**
//!
//! `docs/reference/adapter-contract.md` is generated from [`ADAPTERS`] by
//! `the_checked_in_adapter_contract_is_current` (regenerate with `UPDATE_ADAPTER_CONTRACT=1 cargo test --lib
//! adapter_contract`), and `scripts/check-feature-pages.py` holds the page to the code: a row per adapter, a test
//! per *carried* cell, and — in CI's `--executed` mode — every such test executed in the run. `wire-check` reads a
//! unit's `runtime` key against the same rows (X8) and says, per unit, what its runtime carries.

use serde::Serialize;

/// The seven properties, in column order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Property {
    SessionPersistence,
    Cancellation,
    Tracing,
    IdentityPropagation,
    Grants,
    ProviderSideEnforcement,
    Receipts,
}

impl Property {
    pub const ALL: [Property; 7] = [
        Property::SessionPersistence,
        Property::Cancellation,
        Property::Tracing,
        Property::IdentityPropagation,
        Property::Grants,
        Property::ProviderSideEnforcement,
        Property::Receipts,
    ];

    /// The column's name, as the table and `wire-check` print it.
    pub fn name(self) -> &'static str {
        match self {
            Property::SessionPersistence => "session persistence",
            Property::Cancellation => "cancellation",
            Property::Tracing => "tracing",
            Property::IdentityPropagation => "identity propagation",
            Property::Grants => "grants",
            Property::ProviderSideEnforcement => "provider-side enforcement",
            Property::Receipts => "receipts",
        }
    }

    /// What the property means, once, for the table's legend.
    pub fn meaning(self) -> &'static str {
        match self {
            Property::SessionPersistence => "an agent's session or checkpoint written through the adapter is read back whole on another node, or after a restart",
            Property::Cancellation => "work started through the adapter can be cancelled through it, and the node stops it — which no adapter carries today: `tasks/cancel` ends a record, a dropped future tells no provider, and running work stops only when its authority lapses (`authority_drain`)",
            Property::Tracing => "X4's correlation id and parent hash ride from the caller into the records the fleet writes",
            Property::IdentityPropagation => "the caller's principal — not the node's — reaches the enforcement point that decides",
            Property::Grants => "a call the policy holds for approval can carry X3's grant, verified at the door",
            Property::ProviderSideEnforcement => "the call is decided at the provider that runs it, not only at the gateway it entered",
            Property::Receipts => "an acknowledgement names its durability rung and nothing above it",
        }
    }
}

/// One cell of the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Cell {
    /// Shown by `test` in `file` (repository-relative). A Rust or pytest name is a snake_case identifier; a jest
    /// title is the test's string.
    Carried { test: &'static str, file: &'static str, note: &'static str },
    /// The adapter could carry it and does not; `why` names what is missing, and the adapter's *What this does not
    /// establish* section says the rest.
    NotCarried { why: &'static str },
    /// The property has no meaning for this adapter.
    NotApplicable { why: &'static str },
}

impl Cell {
    pub fn is_carried(&self) -> bool {
        matches!(self, Cell::Carried { .. })
    }
}

/// One adapter.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Adapter {
    /// The row's name.
    pub name: &'static str,
    /// The unit file's `runtime` value that names it (X8); `None` for a door or a companion no unit declares.
    pub runtime: Option<&'static str>,
    /// One line: what the adapter is.
    pub what: &'static str,
    /// The cells, in [`Property::ALL`]'s order.
    pub cells: [Cell; 7],
    /// The guarantee ids (the guarantee catalogue's) the adapter's door resolves; empty for an adapter outside the
    /// node, which registers none (an SDK cannot register a guarantee).
    pub guarantees: &'static [&'static str],
    /// The adapter's *What this does not establish* section, repository-relative with its anchor.
    pub limits: &'static str,
}

impl Adapter {
    /// The properties this adapter carries.
    pub fn carried(&self) -> Vec<Property> {
        Property::ALL.iter().copied().zip(self.cells.iter()).filter(|(_, c)| c.is_carried()).map(|(p, _)| p).collect()
    }
    /// The properties this adapter could carry and does not.
    pub fn not_carried(&self) -> Vec<Property> {
        Property::ALL.iter().copied().zip(self.cells.iter()).filter(|(_, c)| matches!(c, Cell::NotCarried { .. })).map(|(p, _)| p).collect()
    }
}

const PY_LIMITS: &str = "mycelium-py/README.md#what-this-does-not-establish";
const TS_LIMITS: &str = "mycelium-ts/README.md#what-this-does-not-establish";
const LG_LIMITS: &str = "langgraph-checkpoint-mycelium/README.md#what-this-does-not-establish";
const STRANDS_LIMITS: &str = "strands-session-mycelium/README.md#what-this-does-not-establish";
const DOOR_LIMITS: &str = "docs/features/adapter-contract.md#what-it-guarantees-and-what-it-does-not";

/// The rows. The order is the table's.
pub const ADAPTERS: &[Adapter] = &[
    Adapter {
        name: "native",
        runtime: None,
        what: "a Rust program embedding the library: a member node that calls providers directly, with no gateway between (the Rust scaffold)",
        cells: [
            Cell::NotApplicable { why: "a Rust embedding holds its own state; what it writes to the KV persists under the receipt's rung" },
            Cell::NotCarried { why: "no cancel verb: dropping a direct call's future abandons the wait, and the provider is not told" },
            Cell::Carried { test: "a_members_direct_call_carries_the_fields_in_its_own_bytes_and_the_gateway_s_raw_door_refuses_the_kind", file: "src/agent/http/causal_door_tests.rs", note: "the fields ride in the member's own payload bytes" },
            Cell::Carried { test: "test_c7_the_bypass_matrix_no_door_runs_revoked_work", file: "src/lib_tests.rs", note: "a member's identity is its TLS key, `node:{id}` at the provider" },
            Cell::Carried { test: "a_plain_transition_to_a_grant_required_tool_is_refused", file: "src/agent/state_machine.rs", note: "the agent state machine waits for the grant before the tool runs" },
            Cell::Carried { test: "test_c7_the_bypass_matrix_no_door_runs_revoked_work", file: "src/lib_tests.rs", note: "`with_provider_enforcement`: a direct call is checked where it runs" },
            Cell::Carried { test: "a_receipt_names_its_rung_and_nothing_above_it", file: "mycelium-core/src/receipt.rs", note: "`set_with_receipt`, `set_requiring_sync`" },
        ],
        guarantees: &["prov.enforcement", "ae.authorised_at_seam", "ae.recorded_before_dispatch"],
        limits: DOOR_LIMITS,
    },
    Adapter {
        name: "mycelium-py",
        runtime: Some("python"),
        what: "the Python SDK: a gateway client over HTTP and SSE (the Python scaffold, `declare_from`)",
        cells: [
            Cell::NotApplicable { why: "the SDK keeps no session; the checkpointer and the Strands store are the session adapters" },
            Cell::NotCarried { why: "`A2aClient.cancel` reaches `tasks/cancel`, which ends a task's record and stops no work (a task runs inside its request); `tools/call` has no cancel; a stream's end closes the client's side only" },
            Cell::Carried { test: "test_set_carries_the_causal_fields_and_the_receipt_echoes_them", file: "mycelium-py/tests/test_kv_receipt.py", note: "`correlation` and `parent` on `set`; the prompt-skill and federation calls have their own tests in `test_causal_fields.py`" },
            Cell::Carried { test: "test_agent_sync_calls_send_bearer", file: "mycelium-py/tests/test_gateway_token.py", note: "the bearer names the principal the gateway resolves" },
            Cell::Carried { test: "test_a_presented_mandate_travels_in_meta_and_is_absent_otherwise", file: "mycelium-py/tests/test_mandate.py", note: "a grant is a mandate (D1) and rides in `_meta.mandate`; the supervisor answers requests with `grants_pending` and `grant_decide`; no pending-grants stream" },
            Cell::Carried { test: "the_causal_fields_ride_in_the_payload_to_the_gateway_s_and_the_provider_s_records", file: "src/agent/http/causal_tests.rs", note: "through the MCP door, whose provider record the test reads" },
            Cell::Carried { test: "test_set_returns_the_receipt", file: "mycelium-py/tests/test_kv_receipt.py", note: "`set()` returns `KvReceipt`" },
        ],
        guarantees: &[],
        limits: PY_LIMITS,
    },
    Adapter {
        name: "mycelium-ts",
        runtime: Some("typescript"),
        what: "the TypeScript SDK: a gateway client over HTTP and SSE (`declareUnits`)",
        cells: [
            Cell::NotApplicable { why: "the SDK keeps no session" },
            Cell::NotCarried { why: "`A2aClient.cancel` reaches `tasks/cancel`, which ends a task's record and stops no work; `tools/call` has no cancel; ending a stream's generator closes the client's side only" },
            Cell::Carried { test: "set carries the causal fields and the receipt echoes them", file: "mycelium-ts/tests/causal.test.ts", note: "`correlation` and `parent` on `set`; the prompt-skill and federation calls have their own tests in the same file" },
            Cell::Carried { test: "MyceliumAgent GET / POST / DELETE", file: "mycelium-ts/tests/auth.test.ts", note: "the bearer on every request" },
            Cell::Carried { test: "a presented mandate travels in _meta, and is absent otherwise", file: "mycelium-ts/tests/mandate.test.ts", note: "a grant is a mandate (D1) and rides in `_meta.mandate`; the supervisor answers requests with `grantsPending` and `grantDecide`; no pending-grants stream" },
            Cell::Carried { test: "the_causal_fields_ride_in_the_payload_to_the_gateway_s_and_the_provider_s_records", file: "src/agent/http/causal_tests.rs", note: "through the MCP door, whose provider record the test reads" },
            Cell::Carried { test: "set carries the causal fields and the receipt echoes them", file: "mycelium-ts/tests/causal.test.ts", note: "`set()` returns `KvReceipt`" },
        ],
        guarantees: &[],
        limits: TS_LIMITS,
    },
    Adapter {
        name: "langgraph-checkpoint-mycelium",
        runtime: Some("langgraph"),
        what: "LangGraph's checkpoint saver over the gateway's KV and blob routes",
        cells: [
            Cell::Carried { test: "test_langgraph_run_checkpointed_on_a_resumes_on_b", file: "langgraph-checkpoint-mycelium/tests/test_checkpointer.py", note: "a run checkpointed through node A resumes through node B" },
            Cell::NotApplicable { why: "a checkpointer is a store; cancellation is LangGraph's" },
            Cell::NotCarried { why: "checkpoint rows carry no correlation or parent; the SDK's fields are not set by the saver" },
            Cell::Carried { test: "test_an_explicit_token_rides_both_clients", file: "langgraph-checkpoint-mycelium/tests/test_connection.py", note: "the bearer on the sync and async clients" },
            Cell::NotApplicable { why: "a store makes no governed call" },
            Cell::NotApplicable { why: "KV writes, not provider calls" },
            Cell::Carried { test: "test_put_hands_back_the_gateways_receipt", file: "langgraph-checkpoint-mycelium/tests/test_receipt.py", note: "every write's `WriteReceipt` on `last_receipt` and `on_receipt`; `require_on_disk` refuses a write the gateway node did not establish" },
        ],
        guarantees: &[],
        limits: LG_LIMITS,
    },
    Adapter {
        name: "strands-session-mycelium",
        runtime: Some("strands"),
        what: "a Strands Agents session repository over the gateway's KV, with fleet-wide claims",
        cells: [
            Cell::Carried { test: "test_a_conversation_and_its_state_resume_in_another_manager", file: "strands-session-mycelium/tests/test_store.py", note: "a session and its state resume in another manager over one gateway; across nodes, the live suite (`MYCELIUM_STRANDS_LIVE_REQUIRED`)" },
            Cell::NotApplicable { why: "a session store; cancellation is Strands'" },
            Cell::NotCarried { why: "session rows carry no correlation or parent" },
            Cell::Carried { test: "test_the_bearer_rides_every_request_the_store_makes", file: "strands-session-mycelium/tests/test_bearer.py", note: "the bearer on every request the store makes; the gateway resolves and records its principal" },
            Cell::NotApplicable { why: "a store makes no governed call" },
            Cell::NotApplicable { why: "KV writes, not provider calls" },
            Cell::Carried { test: "test_every_write_hands_back_the_sdks_receipt", file: "strands-session-mycelium/tests/test_store.py", note: "`last_receipt`, `on_receipt`, `require_on_disk`" },
        ],
        guarantees: &[],
        limits: STRANDS_LIMITS,
    },
    Adapter {
        name: "mycelium-reason",
        runtime: None,
        what: "routed inference: the OpenAI-compatible façade, the router, the reason trace keyed by `run_id`",
        cells: [
            Cell::NotApplicable { why: "a routing companion keeps no agent session" },
            Cell::NotCarried { why: "an abandoned call releases the router's local reservation and tells no provider; the façade has no cancel" },
            Cell::Carried { test: "a_confidential_request_is_routed_to_an_in_domain_model_and_the_external_one_is_passed_over", file: "mycelium-reason/tests/gateway.rs", note: "`run_id` is X4's correlation on the façade's evidence records" },
            Cell::Carried { test: "guide_20_routing_policy", file: "mycelium-reason/tests/gateway.rs", note: "the caller's principal selects its data-class floor (`floor_for`)" },
            Cell::NotCarried { why: "the façade carries no grant; a routed call held for approval has no door here" },
            Cell::Carried { test: "the_facade_runs_the_action_preflight", file: "mycelium-reason/tests/gateway.rs", note: "the façade runs the action preflight before it routes" },
            Cell::NotApplicable { why: "an inference answer is not a durable write" },
        ],
        guarantees: &["ae.authorised_at_seam", "ae.recorded_before_dispatch"],
        limits: "docs/operations/companions.md#mycelium-reason--routed-inference-the-openai-façade-fleet-traces",
    },
    Adapter {
        name: "A2A door",
        runtime: None,
        what: "`/a2a`: an unmodified A2A client's tasks, owned by the identity that created them",
        cells: [
            Cell::NotCarried { why: "by design: a task lives in the node that accepted it, and the work it tracks is not durable either — a restart loses both, a peer serves neither; durable work is the tuple space's and the commitment net's" },
            Cell::NotCarried { why: "`tasks/cancel` ends a task's record (the owner's only) and stops no work: a task runs inside its request and is done before a cancel can arrive" },
            Cell::Carried { test: "the_a2a_door_carries_the_fields_on_its_own_record_and_forwards_none", file: "src/agent/http/causal_door_tests.rs", note: "on the door's own record; a skill call's payload is text, so the provider's record is a root" },
            Cell::Carried { test: "a_task_is_readable_and_cancellable_only_by_the_identity_that_created_it", file: "src/agent/a2a.rs", note: "the caller's principal owns the task" },
            Cell::NotCarried { why: "the A2A `input-required` state is not built (X3's second PR); a held call is refused, not parked" },
            Cell::Carried { test: "test_c7_the_bypass_matrix_no_door_runs_revoked_work", file: "src/lib_tests.rs", note: "`/a2a` send and stream under `with_provider_enforcement`: nothing runs after a revocation" },
            Cell::NotApplicable { why: "a task result is not a durable write" },
        ],
        guarantees: &["a2a.admission", "ae.authorised_at_seam", "ae.recorded_before_dispatch"],
        limits: DOOR_LIMITS,
    },
    Adapter {
        name: "MCP door",
        runtime: None,
        what: "`/mcp`: `tools/call` decided at the gateway and again at the provider that runs the tool",
        cells: [
            Cell::NotApplicable { why: "a tool call keeps no session" },
            Cell::NotCarried { why: "no cancel for `tools/call`; a client that leaves does not stop the tool" },
            Cell::Carried { test: "the_causal_fields_ride_in_the_payload_to_the_gateway_s_and_the_provider_s_records", file: "src/agent/http/causal_tests.rs", note: "`params._meta` to the gateway's and the provider's records" },
            Cell::Carried { test: "a_mandate_travels_in_the_envelope_and_one_without_is_unchanged", file: "src/agent/gateway_caller.rs", note: "the signed caller envelope names the principal at the provider" },
            Cell::Carried { test: "an_issued_grant_lets_the_call_run_and_binds_it_to_its_arguments", file: "src/agent/http/grant_tests.rs", note: "a grant bound to the call's arguments and policy revision" },
            Cell::Carried { test: "the_causal_fields_ride_in_the_payload_to_the_gateway_s_and_the_provider_s_records", file: "src/agent/http/causal_tests.rs", note: "the provider's own record of the decision" },
            Cell::NotApplicable { why: "a tool result is not a durable write; the KV route's receipt is the KV route's" },
        ],
        guarantees: &["prov.enforcement", "ae.authorised_at_seam", "ae.recorded_before_dispatch"],
        limits: DOOR_LIMITS,
    },
];

/// The adapter a unit's `runtime` names, if any.
pub fn for_runtime(runtime: &str) -> Option<&'static Adapter> {
    ADAPTERS.iter().find(|a| a.runtime == Some(runtime))
}

/// The runtimes a unit file may name, in table order.
pub fn runtimes() -> Vec<&'static str> {
    ADAPTERS.iter().filter_map(|a| a.runtime).collect()
}

/// The adapter a unit with no `runtime` key is: the native embedding.
pub fn native() -> &'static Adapter {
    &ADAPTERS[0]
}

fn link(file: &str) -> String {
    format!("[{file}](../../{file})")
}

fn cell_markdown(cell: &Cell) -> String {
    match cell {
        Cell::Carried { test, file, note } => {
            let name = if test.contains(' ') { format!("\"{test}\"") } else { format!("`{test}`") };
            format!("carried — {name} ({}); {note}", link(file))
        }
        Cell::NotCarried { why } => format!("not carried — {why}"),
        Cell::NotApplicable { why } => format!("not applicable — {why}"),
    }
}

/// The checked-in reference page, `docs/reference/adapter-contract.md`.
pub fn render_markdown() -> String {
    let mut s = String::new();
    s.push_str("# The adapter contract\n\n");
    s.push_str("**Generated** from `ADAPTERS` (`src/adapter_contract.rs`); do not edit. Regenerate with `UPDATE_ADAPTER_CONTRACT=1 cargo test --lib adapter_contract`. ");
    s.push_str("One row per runtime adapter and one column per property; a cell is *carried* and names the test CI runs that shows it, *not carried* with the reason (the adapter's *What this does not establish* section says the rest), or *not applicable*. ");
    s.push_str("`scripts/check-feature-pages.py` holds this page to the code — a row per adapter, a test per carried cell, and in CI's `--executed` mode every such test executed in the run. ");
    s.push_str("A unit file's `runtime` key names a row, and `wire-check` prints what that runtime carries, per unit ([the unit file](unit-file.md#runtime)). ");
    s.push_str("**The table records; it does not make an adapter carry anything** (`docs/plans/adopter-surface.md`, X7 and X8).\n\n");
    s.push_str("## The table\n\n");
    s.push_str("| Adapter | `runtime` |");
    for p in Property::ALL {
        s.push_str(&format!(" {} |", p.name()));
    }
    s.push_str("\n|---|---|");
    for _ in Property::ALL {
        s.push_str("---|");
    }
    s.push('\n');
    for a in ADAPTERS {
        s.push_str(&format!("| **{}** | {} |", a.name, a.runtime.map(|r| format!("`{r}`")).unwrap_or_else(|| "—".into())));
        for c in &a.cells {
            s.push_str(&format!(" {} |", cell_markdown(c)));
        }
        s.push('\n');
    }
    s.push_str("\n## The properties\n\n");
    for p in Property::ALL {
        s.push_str(&format!("- **{}** — {}.\n", p.name(), p.meaning()));
    }
    s.push_str("\n## The adapters\n\n");
    for a in ADAPTERS {
        s.push_str(&format!("### {}\n\n{}. ", a.name, a.what));
        match a.runtime {
            Some(r) => s.push_str(&format!("A unit file names it with `runtime = \"{r}\"`. ")),
            None => s.push_str("No unit file names it: a unit without `runtime` is native, and the doors are reached, not declared. "),
        }
        if a.guarantees.is_empty() {
            s.push_str("It registers no guarantee: an adapter outside the node cannot, and what it carries is what the doors it calls carry. ");
        } else {
            let ids: Vec<String> = a.guarantees.iter().map(|g| format!("[`{g}`](guarantee-catalogue.md)")).collect();
            s.push_str(&format!("Its door resolves {}. ", ids.join(", ")));
        }
        s.push_str(&format!("Carries: {}. Not carried: {}. What this does not establish: [{}](../../{}).\n\n",
            join_or_none(a.carried()), join_or_none(a.not_carried()), a.limits.split('#').next().unwrap_or(a.limits), a.limits));
    }
    s
}

fn join_or_none(props: Vec<Property>) -> String {
    if props.is_empty() { "none".to_string() } else { props.iter().map(|p| p.name()).collect::<Vec<_>>().join(", ") }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The checked-in page is the code's rendering, byte for byte.
    #[test]
    fn the_checked_in_adapter_contract_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/reference/adapter-contract.md");
        let want = render_markdown();
        if std::env::var("UPDATE_ADAPTER_CONTRACT").is_ok() {
            std::fs::write(&path, &want).unwrap();
            return;
        }
        let have = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(have == want, "docs/reference/adapter-contract.md is stale: regenerate with UPDATE_ADAPTER_CONTRACT=1 cargo test --lib adapter_contract");
    }

    /// Every carried cell's file exists and holds the test it names (a Rust `fn`, a pytest `def`, or a jest
    /// title); every runtime is named once; every limits link resolves; every guarantee id is the catalogue's.
    #[test]
    fn every_carried_cell_names_a_test_that_exists() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let catalogue = std::fs::read_to_string(root.join("docs/reference/guarantee-catalogue.json")).unwrap();
        let mut runtimes = std::collections::HashSet::new();
        for a in ADAPTERS {
            if let Some(r) = a.runtime {
                assert!(runtimes.insert(r), "runtime `{r}` named twice");
            }
            for g in a.guarantees {
                assert!(catalogue.contains(&format!("\"id\": \"{g}\"")), "{}: `{g}` is not in the guarantee catalogue", a.name);
            }
            let limits_file = a.limits.split('#').next().unwrap();
            assert!(root.join(limits_file).is_file(), "{}: limits file {limits_file} does not exist", a.name);
            for c in &a.cells {
                if let Cell::Carried { test, file, .. } = c {
                    let text = std::fs::read_to_string(root.join(file)).unwrap_or_else(|e| panic!("{}: {file}: {e}", a.name));
                    let found = if file.ends_with(".rs") {
                        text.contains(&format!("fn {test}("))
                    } else if file.ends_with(".py") {
                        text.contains(&format!("def {test}("))
                    } else {
                        text.contains(&format!("\"{test}\""))
                    };
                    assert!(found, "{}: `{test}` is not in {file}", a.name);
                }
            }
        }
        assert_eq!(native().name, "native");
        assert_eq!(for_runtime("strands").map(|a| a.name), Some("strands-session-mycelium"));
        assert!(for_runtime("rust").is_none(), "a unit without `runtime` is native; `rust` is not a value");
    }
}
