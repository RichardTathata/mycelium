//! The `grant` class of `mycelium test --faults` — **a grant that is default-deny and holds across a restart**:
//! adopter-surface X3's exit on the declaration's own fleet, and the one-hour job's step 4.
//!
//! The declaration names what the class drives. The first `[[rule]]` with `requires_grant = true` is the tool
//! (`resource = "tool:<name>"`) and the agent (the unit that declares it); the actor of a `[[rule]]` admitting
//! `grant.issue` on that resource — or on `*` — is the issuer the class plays. The agent's node is that unit's node
//! of the fleet, started with an evaluator built from every declared `[[rule]]` (as `policy-diff` builds one), an
//! evidence journal, and an execution authority entitled to the node's own grants; the other nodes gossip. Then:
//!
//! 1. **default-deny** — a plain transition to the tool is refused by name; `transition_invoking` writes a request
//!    under `grant/{agent}/` and, nobody answering, refuses on silence within the window;
//! 2. **issuing is the declaration's rule** — a principal no rule names is refused the issue by the evaluator and
//!    nothing is written; the declared issuer's grant is written, signed by the node, and admits the waiting call
//!    once: the request consumed, the state `Invoking`;
//! 3. **the grant holds across a restart** — a request is written and the agent's node stopped before it reads the
//!    answer; the issuer answers the standing request; the node restarts over its own WAL; the request and the grant
//!    are back; the restarted agent re-waits on its request — it does not ask twice — and the grant issued before the
//!    restart verifies after it;
//! 4. **bound to its arguments** — the same tool with other arguments asks again, and is refused on silence;
//! 5. **on the record** — the evidence journal holds the refused and the permitted issue decisions, and the request
//!    record carries the arguments a human read beside the digest the signed grant binds.
//!
//! **What it is not.** The state machine's door and the issuing door, in one process: the gateway's and the
//! provider's doors are the door tests' (`src/agent/http/grant_tests.rs`), and so is revocation with its drain bound.
//! The stop is graceful, as in the restart test. The class issues through the route's own decision
//! ([`decide_grant`](crate::agent::decide_grant)), not over HTTP — the fleet has no gateway port — so the route's
//! scope rule (`grant:issue`) is outside its view and the evaluator's rule is what it exercises. A declared
//! `tool:<name>` is also admitted at `tool:<name>@<the agent's node>`, the form the request record and the doors use
//! (the report says so).

use super::*;

/// The guarantees the class drives: the grant and its request replayed from the node's own WAL, the identity that
/// signs the grant and proves possession.
pub(super) const GUARANTEES: &[&str] = &["persist.configured", "persist.sync_mode", "mesh.tls"];
/// The decision point: who may issue, decided by the evaluator at `gateway:grants/issue`.
pub(super) const RULES: &[&str] = &["ae.preflight"];

/// The principal the class presents as nobody the rules name.
#[cfg(all(feature = "gateway", feature = "tls", feature = "compliance"))]
const STRANGER: &str = "test-faults-stranger";

#[cfg(not(all(feature = "gateway", feature = "tls", feature = "compliance")))]
pub(super) fn run(rep: &mut ClassReport, base: &GossipConfig, _units: &[Unit], _dir: &Path) {
    // The feature the build lacks, by name: a grant needs the gateway's doors, the node's identity, and the member
    // keys (`compliance`) a grant's checkpoint is verified against.
    let feature = if !cfg!(feature = "gateway") { "gateway" } else if !cfg!(feature = "tls") { "tls" } else { "compliance" };
    rep.state = ClassState::NotInBuild { feature };
    let why = class_reason(&rep.state);
    not_exercised(rep, unstarted_report(base).as_ref(), GUARANTEES, RULES, &why);
}

/// Start node `i` as the agent's node: the evaluator, the journal at `journal`, and an execution authority entitled
/// to grants issued for `principal` (the node's own), each attached before the lifecycle boundary.
#[cfg(all(feature = "gateway", feature = "tls", feature = "compliance"))]
fn start_agent_node(fleet: &mut Fleet<'_>, i: usize, journal: &Path, evaluator: &Arc<dyn crate::ActionEvaluator>, principal: &str) -> Result<(), String> {
    use crate::agent::gateway_authority::ExecutionAuthority;
    use crate::knowledge::issuer::TrustedExternalIssuers;
    use crate::mandate::authority::{ClockModel, ExecutionGate, FreshnessPolicy, ResourceTier};
    use crate::mandate::grant::{EntitlementTable, GrantVerifier};
    use crate::mandate::{PrincipalId, ResourceAuthority};

    let failed: Arc<std::sync::Mutex<Option<String>>> = Arc::default();
    let (ev, slot, path, principal) = (Arc::clone(evaluator), Arc::clone(&failed), journal.to_path_buf(), principal.to_string());
    fleet.start_with(i, move |a| {
        // The journal before the evaluator: an evaluator attached first warns that its decisions will not be
        // recorded, which this fleet then disproves (#680's review, finding 6).
        match crate::EvidenceJournal::open(&path, crate::EvidenceProfile::Strict) {
            Ok(j) => a.with_evidence_journal(j),
            Err(e) => *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(format!("opening {}: {e}", path.display())),
        }
        a.with_action_evaluator(ev);
        let mut entitlements = EntitlementTable::new();
        match PrincipalId::new(&principal) {
            Some(p) => entitlements.entitle_grants(p),
            None => *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(format!("{principal} is not a principal")),
        }
        let gate = ExecutionGate::strict(
            ResourceAuthority::new("test-faults", 1),
            ResourceTier::Serialised,
            ClockModel { skew_ms: 100 },
            FreshnessPolicy { freshness_ms: 120_000, interval_ms: 30_000, delivery_ms: 10_000 },
        );
        match gate {
            Ok(gate) => a.with_execution_authority(Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(entitlements), TrustedExternalIssuers::new()))),
            Err(e) => *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(format!("the execution gate: {e:?}")),
        }
    })?;
    match failed.lock().unwrap_or_else(|e| e.into_inner()).take() {
        Some(reason) => Err(reason),
        None => Ok(()),
    }
}

/// A transition's outcome, in the report's words.
#[cfg(all(feature = "gateway", feature = "tls", feature = "compliance"))]
fn said<T>(r: &Result<T, crate::PolicyViolation>) -> String {
    match r {
        Ok(_) => "proceeded".to_string(),
        Err(e) => format!("refused — {e}"),
    }
}

#[cfg(all(feature = "gateway", feature = "tls", feature = "compliance"))]
pub(super) fn run(rep: &mut ClassReport, base: &GossipConfig, units: &[Unit], dir: &Path) {
    use crate::agent::gateway_caller::{node_principal, ResolvedPrincipal};
    use crate::agent::{decide_grant, GrantDecision, GrantRefusal};
    use crate::{AgentPolicy, ExecutionState, PolicyViolation};
    use std::collections::HashSet;

    let not_applicable = |rep: &mut ClassReport, because: String| {
        rep.state = ClassState::NotApplicable { because };
        let why = class_reason(&rep.state);
        not_exercised(rep, unstarted_report(base).as_ref(), GUARANTEES, RULES, &why);
    };
    let not_configured = |rep: &mut ClassReport, missing: &str| {
        rep.state = ClassState::NotConfigured { missing: missing.into() };
        let why = class_reason(&rep.state);
        not_exercised(rep, unstarted_report(base).as_ref(), GUARANTEES, RULES, &why);
    };

    // ── What the declaration asks for ──
    let rules: Vec<&crate::capability_config::RuleDecl> = units.iter().flat_map(|u| u.config.rules.iter()).collect();
    let Some((unit_index, rule)) = units.iter().enumerate().find_map(|(i, u)| u.config.rules.iter().find(|r| r.requires_grant).map(|r| (i, r))) else {
        not_applicable(rep, "no [[rule]] has requires_grant = true: nothing in the declaration waits for a grant".into());
        return;
    };
    let Some(tool) = rule.resource.strip_prefix("tool:").map(|t| t.split('@').next().unwrap_or(t).to_string()).filter(|t| !t.is_empty()) else {
        not_applicable(rep, format!("the grant-required rule's resource {:?} is not a tool (tool:<name>): the grant mode guards tools/call", rule.resource));
        return;
    };
    let Some(issuer_rule) = rules.iter().find(|r| r.admits_grant_issue_for(&rule.resource)) else {
        not_applicable(rep, format!("no [[rule]] admits anyone to grant.issue on {}: the grant could never be issued, and the class would show only the refusal (wire-check's `unissuable grant`)", rule.resource));
        return;
    };
    // `*` admits anyone to issue — the stranger too — so that half of check 2 is not run, and the report says so
    // (#680's review, finding 1: the stranger's issue was written and the issuer's then refused `already_answered`).
    let anyone = rules.iter().any(|r| r.actor == "*" && r.admits_grant_issue_for(&rule.resource));
    let issuer = if issuer_rule.actor == "*" { "test-faults-supervisor".to_string() } else { issuer_rule.actor.clone() };
    if base.tls.is_none() {
        not_configured(rep, "[tls] (the node signs the grant and proves possession with its identity)");
        return;
    }
    if base.persistence.is_none() {
        not_configured(rep, "[persistence]");
        return;
    }

    // ── The fleet: the agent's node with its attachments, the rest gossiping ──
    let mut fleet = match Fleet::plan(base, units, dir, units.len().max(3)) {
        Ok(f) => f,
        Err(reason) => { rep.state = ClassState::Refused { reason }; return; }
    };
    let agent_node = fleet.nodes[unit_index].id.clone();
    let agent_principal = node_principal(&agent_node);
    // Every declared rule as written — and, for the forms the doors use, two more readings of each: a `tool:<name>`
    // resource also at the agent's node (`tool:<name>@<node>`, the request record's resource), and the grant-required
    // rule's actor also as the node's principal — at a door the agent presents as its node, the only principal that
    // can hold a grant (#680's review, finding 4). The declaration names the unit; the fleet pins it to a node.
    let evaluator: Arc<dyn crate::ActionEvaluator> = {
        let mut ev = crate::ReferenceEvaluator::new("declared").with_catalogue("declaration", "declared");
        for u in units {
            for r in u.config.rules.iter().filter(|r| !r.is_exposure()) {
                let built = match r.to_rule() {
                    Ok(b) => b,
                    Err(e) => { rep.state = ClassState::Refused { reason: format!("unit {}: [[rule]]: {e}", u.name) }; return; }
                };
                let mut forms = vec![built.clone()];
                if r.resource.starts_with("tool:") && !r.resource.contains('@') {
                    let mut at_node = built.clone();
                    at_node.resource = format!("{}@{agent_node}", r.resource);
                    forms.push(at_node);
                }
                if r.requires_grant && u.config.principal.as_deref() == Some(r.actor.as_str()) {
                    for f in forms.clone() {
                        let mut as_node = f;
                        as_node.actor = agent_principal.clone();
                        forms.push(as_node);
                    }
                }
                for f in forms {
                    // A `*` operation admits any; the catalogue mapping is per operation, so the operations this class
                    // drives are mapped for it — an unmapped operation is refused before any rule is read.
                    let mapped: Vec<String> = if f.operation == "*" {
                        ["grant.issue", "grant.refuse", "grant.revoke", "tools/call"].iter().map(|s| s.to_string()).collect()
                    } else {
                        vec![f.operation.clone()]
                    };
                    for op in mapped {
                        ev = ev.map_action(op, "*");
                    }
                    ev = ev.allow(f);
                }
            }
        }
        Arc::new(ev)
    };
    let journal_path = dir.join("journal").join("evidence.log");
    if let Err(reason) = start_agent_node(&mut fleet, unit_index, &journal_path, &evaluator, &agent_principal) {
        rep.state = ClassState::Refused { reason };
        return;
    }
    for j in (0..fleet.len()).filter(|j| *j != unit_index) {
        if let Err(reason) = fleet.start(j) {
            rep.state = ClassState::Refused { reason };
            return;
        }
    }
    exercised(rep, fleet.guarantees().as_ref(), GUARANTEES, RULES);
    let i = unit_index;
    let prefix = format!("grant/{agent_principal}/");
    let requests = |a: &GossipAgent| -> Vec<String> {
        crate::store::scan_kv_prefix(&a.task_ctx.kv_state, &prefix)
            .into_iter()
            .filter_map(|(k, _)| k.strip_prefix(prefix.as_str()).filter(|rest| !rest.contains('/')).map(str::to_string))
            .collect()
    };
    let marker = |a: &GossipAgent, request: &str, m: &str| mycelium_core::ops::kv_get(&a.task_ctx, &format!("{prefix}{request}/{m}")).is_some();
    let policy = |window_ms: u64| AgentPolicy { require_grant_for: vec![tool.clone()], grant_window: Some(Duration::from_millis(window_ms)), ..AgentPolicy::default() };
    let supervisor = |principal: &str| ResolvedPrincipal { principal: principal.to_string(), scopes: vec!["grant:issue".into(), "grant:read".into()] };
    let a = fleet.agent(i);

    // ── 1. default-deny ──
    let t = crate::sim_seam::mono_instant();
    const SILENCE_MS: u64 = 1_500;
    let args = serde_json::json!({ "ledger": "petty-cash", "amount": 42, "memo": "test --faults grant" });
    let sm = a.agent_state_machine(policy(SILENCE_MS));
    let planning = fleet.on(i, sm.transition(ExecutionState::Planning));
    let plain = fleet.on(i, sm.transition(ExecutionState::Invoking { tool: tool.clone() }));
    let silent = fleet.on(i, sm.transition_invoking(&tool, &args));
    let after_silence = requests(&a);
    let ok = planning.is_ok()
        && matches!(&plain, Err(PolicyViolation::GrantNotPresented(_)))
        && matches!(&silent, Err(PolicyViolation::GrantNotPresented(_)))
        && after_silence.len() == 1;
    check(
        rep,
        &t,
        "a grant-required tool is default-deny: a plain transition is refused, and silence within the window refuses",
        Some(ok),
        format!("plain transition to {tool}: {}; with arguments, after {SILENCE_MS} ms of silence: {}; {} request(s) on the record under {prefix}", said(&plain), said(&silent), after_silence.len()),
    );
    drop(sm);

    // ── 2. issuing is the declaration's rule ──
    let t = crate::sim_seam::mono_instant();
    let sm = a.agent_state_machine(policy(20_000));
    let _ = fleet.on(i, sm.transition(ExecutionState::Planning));
    let seen: HashSet<String> = after_silence.iter().cloned().collect();
    let (sm2, args2, tool2) = (Arc::clone(&sm), args.clone(), tool.clone());
    let waiting = fleet.spawn(i, async move { sm2.transition_invoking(&tool2, &args2).await });
    let fresh = |a: &GossipAgent, seen: &HashSet<String>| requests(a).into_iter().find(|r| !seen.contains(r));
    fleet.wait(i, Duration::from_secs(5), || fresh(&a, &seen).is_some());
    let Some(request) = fresh(&a, &seen) else {
        check(rep, &t, "issuing is the declaration's rule: a principal no rule names is refused, the declared issuer's grant admits the call once", Some(false), "no request was written within 5 s of the call".to_string());
        waiting.abort();
        return;
    };
    // The stranger asks only where a rule could refuse them: under a `*` actor the declaration admits anyone, and the
    // stranger's grant would be the one written.
    let (stranger_refused, stranger_text) = if anyone {
        (true, format!("the declaration admits anyone (`*`) to grant.issue on {}, so {STRANGER} is not refused — stated, not exercised", rule.resource))
    } else {
        let stranger = fleet.on(i, decide_grant(&a.task_ctx, Some(&supervisor(STRANGER)), &agent_principal, &request, GrantDecision::Issue { ttl_secs: Some(600) }));
        let text = match &stranger {
            Err(GrantRefusal::Policy { reason, detail, .. }) => format!("{STRANGER}'s issue refused by the evaluator ({reason}: {detail})"),
            Err(other) => format!("{STRANGER}'s issue refused at the door, not by the evaluator ({other})"),
            Ok(_) => format!("{STRANGER}'s issue was WRITTEN"),
        };
        (matches!(&stranger, Err(GrantRefusal::Policy { .. })) && !marker(&a, &request, "issued"), text)
    };
    let issued = fleet.on(i, decide_grant(&a.task_ctx, Some(&supervisor(&issuer)), &agent_principal, &request, GrantDecision::Issue { ttl_secs: Some(600) }));
    let issued_text = match &issued {
        Ok(o) => format!("written at {} ({})", o.receipt["key"].as_str().unwrap_or("?"), o.receipt["local_durability"].as_str().unwrap_or("?")),
        Err(e) => format!("REFUSED — {e}"),
    };
    fleet.wait(i, Duration::from_secs(10), || waiting.is_finished());
    let admitted = fleet.on(i, waiting).map_err(|e| e.to_string()).and_then(|r| r.map_err(|e| e.to_string()));
    let consumed = marker(&a, &request, "consumed");
    let state = sm.state();
    let ok = stranger_refused && issued.is_ok() && matches!(&admitted, Ok(Some(_))) && consumed && state == (ExecutionState::Invoking { tool: tool.clone() });
    check(
        rep,
        &t,
        "issuing is the declaration's rule: a principal no rule names is refused, the declared issuer's grant admits the call once",
        Some(ok),
        format!(
            "{stranger_text}; {issuer}'s issue: {issued_text}; the waiting call: {}; request {request} consumed: {consumed}; state {state:?}",
            match &admitted { Ok(Some(_)) => "admitted under the grant".to_string(), Ok(None) => "proceeded WITHOUT a grant".to_string(), Err(e) => format!("refused — {e}") },
        ),
    );
    drop(sm);

    // ── 3. the grant holds across a restart ──
    let t = crate::sim_seam::mono_instant();
    let sm = a.agent_state_machine(policy(60_000));
    let _ = fleet.on(i, sm.transition(ExecutionState::Planning));
    let args_b = serde_json::json!({ "ledger": "petty-cash", "amount": 7, "memo": "across a restart" });
    let seen: HashSet<String> = requests(&a).into_iter().collect();
    let (sm2, args2, tool2) = (Arc::clone(&sm), args_b.clone(), tool.clone());
    let waiting = fleet.spawn(i, async move { sm2.transition_invoking(&tool2, &args2).await });
    fleet.wait(i, Duration::from_secs(5), || fresh(&a, &seen).is_some());
    let Some(request_b) = fresh(&a, &seen) else {
        check(rep, &t, "the grant holds across a graceful restart (the exit-snapshot path; the WAL alone is proved by the WAL-only restart test): the request and the grant are back, the restarted agent re-waits and does not ask twice", Some(false), "no request was written within 5 s of the call".to_string());
        waiting.abort();
        return;
    };
    // The agent is stopped before it can read the answer; the issuer answers the standing request.
    waiting.abort();
    let _ = fleet.on(i, waiting);
    let issued_b = fleet.on(i, decide_grant(&a.task_ctx, Some(&supervisor(&issuer)), &agent_principal, &request_b, GrantDecision::Issue { ttl_secs: Some(600) }));
    let issued_b_text = match &issued_b {
        Ok(o) => format!("written ({})", o.receipt["local_durability"].as_str().unwrap_or("?")),
        Err(e) => format!("REFUSED — {e}"),
    };
    // The restart's revocation view starts later than the grant's checkpoint was issued by more than two clock skews,
    // as any real restart does — so the start rule is in question and the exemption #678's review added is what holds
    // the grant (the plant that removes it fails here; without this wait a restart inside 200 ms never asks).
    fleet.wait(i, Duration::from_millis(2 * 100 + 100), || false);
    // Every handle on the node goes before the stop: the state machine holds its context, and with it the journal.
    drop(sm);
    drop(a);
    fleet.stop(i);
    if let Err(e) = start_agent_node(&mut fleet, i, &journal_path, &evaluator, &agent_principal) {
        check(rep, &t, "the grant holds across a graceful restart (the exit-snapshot path; the WAL alone is proved by the WAL-only restart test): the request and the grant are back, the restarted agent re-waits and does not ask twice", Some(false), format!("the agent's node did not restart over its own directory: {e}"));
        return;
    }
    let a = fleet.agent(i);
    let back = requests(&a).contains(&request_b) && marker(&a, &request_b, "issued");
    let sm = a.agent_state_machine(policy(10_000));
    let _ = fleet.on(i, sm.transition(ExecutionState::Planning));
    let count_before = requests(&a).len();
    let after = fleet.on(i, sm.transition_invoking(&tool, &args_b));
    let count_after = requests(&a).len();
    let consumed_b = marker(&a, &request_b, "consumed");
    let ok = issued_b.is_ok() && back && matches!(&after, Ok(Some(_))) && count_after == count_before && consumed_b;
    check(
        rep,
        &t,
        "the grant holds across a graceful restart (the exit-snapshot path; the WAL alone is proved by the WAL-only restart test): the request and the grant are back, the restarted agent re-waits and does not ask twice",
        Some(ok),
        format!(
            "issued before the stop: {issued_b_text}; after the restart the request and the grant are {}; the re-wait: {}; requests {count_before} → {count_after} ({}); request {request_b} consumed: {consumed_b}",
            if back { "back from the WAL" } else { "MISSING" },
            match &after { Ok(Some(_)) => "admitted under the grant issued before the restart".to_string(), Ok(None) => "proceeded WITHOUT a grant".to_string(), Err(e) => format!("refused — {e}") },
            if count_after == count_before { "did not ask twice" } else { "ASKED AGAIN" },
        ),
    );
    drop(sm);

    // ── 4. bound to its arguments ──
    let t = crate::sim_seam::mono_instant();
    let sm = a.agent_state_machine(policy(SILENCE_MS));
    let _ = fleet.on(i, sm.transition(ExecutionState::Planning));
    let count_before = requests(&a).len();
    let other = serde_json::json!({ "ledger": "capital", "amount": 7_000 });
    let bound = fleet.on(i, sm.transition_invoking(&tool, &other));
    let count_after = requests(&a).len();
    let ok = matches!(&bound, Err(PolicyViolation::GrantNotPresented(_))) && count_after == count_before + 1;
    check(
        rep,
        &t,
        "the grant is bound to its arguments: other arguments ask again and are refused on silence",
        Some(ok),
        format!("the same tool with other arguments: {}; requests {count_before} → {count_after}", said(&bound)),
    );
    drop(sm);

    // ── 5. on the record ──
    let t = crate::sim_seam::mono_instant();
    let records: Vec<serde_json::Value> = crate::read_evidence_journal(&journal_path)
        .map(|rs| rs.iter().filter_map(|b| serde_json::from_slice(b).ok()).collect())
        .unwrap_or_default();
    let decision_of = |r: &serde_json::Value| r["decision"].to_string().to_ascii_lowercase();
    // The decision records only: a permitted issue also records what its write did (an `execution` record).
    let issues: Vec<&serde_json::Value> = records.iter().filter(|r| r["operation"] == "grant.issue" && r["kind"] == "decided").collect();
    let permitted = issues.iter().filter(|r| r["subject"] == issuer.as_str() && decision_of(r).contains("permit")).count();
    let refused_count = issues.iter().filter(|r| r["subject"] == STRANGER && !decision_of(r).contains("permit")).count();
    let record: serde_json::Value = mycelium_core::ops::kv_get(&a.task_ctx, &format!("{prefix}{request_b}")).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let rendered = record["arguments"]["ledger"] == "petty-cash" && record["arguments"]["memo"] == "across a restart";
    let digest = record["arguments_digest"].as_str().unwrap_or_default().to_string();
    let grant: Option<crate::mandate::grant::SignedMandateGrant> = mycelium_core::ops::kv_get(&a.task_ctx, &format!("{prefix}{request_b}/issued")).and_then(|b| serde_json::from_slice(&b).ok());
    let binds = digest.len() == 64 && grant.as_ref().is_some_and(|g| g.mandate.operations.iter().any(|op| op.contains(&digest)));
    let ok = permitted >= 2 && (anyone || refused_count >= 1) && rendered && binds;
    check(
        rep,
        &t,
        "the decisions are on the record: the refused and the permitted issue at gateway:grants/issue, the request carrying the arguments and the digest the grant binds",
        Some(ok),
        format!(
            "{} grant.issue decision(s) in {}: {permitted} permitted for {issuer}, {refused_count} refused for {STRANGER}{}; request {request_b}: arguments rendered for a human: {rendered}; digest {} bound in the signed grant's operations: {binds}",
            issues.len(),
            journal_path.display(),
            if anyone { " (anyone may issue here)" } else { "" },
            if digest.len() >= 12 { &digest[..12] } else { digest.as_str() },
        ),
    );
    rep.notes.push(format!(
        "the evaluator is the declaration's {} [[rule]](s), the grant-required one read as a selector: its resource names the tool the agent's policy requires a grant for (the state machine's own check is the default-deny shown), and it is also admitted at `tool:<name>@{agent_node}` and with the node's principal as its actor (the forms the request record and the doors use); the issuer is {issuer}, the actor of the declaration's rule admitting grant.issue on {}; issued through the route's own decision (decide_grant), not over HTTP — the fleet has no gateway port, so the route's scope rule is outside this view; the agent is unit {} on node {agent_node}; persistence under {}",
        rules.len(),
        rule.resource,
        units[unit_index].name,
        dir.join("persist").display()
    ));
    drop(a);
}
