//! **Permitted at the gateway, refused at the resource** — the composed effect, enforced
//! (`docs/design/composed-effect.md` §9).
//!
//! ```text
//! cargo run --example composed_commit --features tls,compliance
//! ```
//!
//! # The claim this exists to make checkable
//!
//! Until §9, the sentence *a durable, attributed, cross-domain effect* was **reconstructable**
//! from the evidence journal after the fact and enforced nowhere: a gateway decided authority and
//! dispatched, and whatever the tool then did at its resource was the tool's business. This example
//! puts the enforcement where the effect commits. A tool handler receives the caller context the
//! gateway verified (`RequestPrincipal::Client`), builds the **composition** from it — the verified
//! principal, the mandate the caller carried, the operation a grant must enumerate — and hands the
//! effect *with* its composition to a destination. The destination refuses unless the effect is
//! attributed (the principal is the mandate's holder) and authorised (the mandate covers this
//! operation at this resource, **now**, under the epoch the resource has installed).
//!
//! Three acts, one call shape:
//!
//! 1. A composed effect that holds **commits** (`Fresh`); the same operation again **replays**
//!    (`Replayed`) — one effect, one row, the receipt names the destination.
//! 2. The resource installs a newer epoch. The gateway's own gate still holds the old one, so the
//!    gateway **permits** the same call — and the destination **refuses** it as superseded. Nothing
//!    is applied; the refusal goes to the evidence journal as a denied decision at enforcement
//!    point `destination`, beside the gateway's permit, so a reader sees which point said no.
//! 3. The journal is read back: the gateway's permit and the destination's refusal, both there.
//!
//! # What this demonstrates, and what it does not
//!
//! **Demonstrates:** a real `POST /mcp` through a real gateway with a real evaluator, a real
//! presented mandate verified at the provider, and a destination that refuses on its own
//! authority state after the gateway has said yes.
//!
//! **Does not:** the domain leg — this is one node, no federation, and a destination holds no
//! trust bundle to re-verify one; the origin is carried, not checked. And the destination's
//! `ResourceAuthority` is this example's own value: a real resource installs epochs from the
//! signed announcements the substrate delivers, which this example stands in for with one call.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use ed25519_dalek::{Signer, SigningKey};
use mycelium::config::{GatewayNamedToken, TlsConfig};
use mycelium::knowledge::issuer::TrustedExternalIssuers;
use mycelium::knowledge::IssuerId;
use mycelium::mandate::authority::{
    ClockModel, ExecutionGate, FreshnessPolicy, ResourceTier, RevocationCheckpoint, SignedRevocationCheckpoint,
};
use mycelium::mandate::grant::{possession_message, EntitlementTable, GrantVerifier, SignedMandateGrant};
use mycelium::mandate::{Mandate, PrincipalId, ResourceAuthority, TermId};
use mycelium::{
    arguments_digest, possession_request, read_evidence_journal, ActionEnvelope, AeEvidence, DecisionKind,
    EvidenceJournal, EvidenceProfile, Execution, ExecutionAuthority, GossipAgent, GossipConfig, NodeId,
    PresentedMandate, RecordKind, ReferenceEvaluator, RequestPrincipal, Rule,
};
use mycelium_effects::{
    AttemptId, ComposedEffect, Composition, DedupOutcome, DestinationCommit, Effect, EffectDestination,
    EffectRefusal, OperationId,
};

const AUTHORITY: &str = "operator:acme";
const CALLER: &str = "token:gw/worker-a";
const TOKEN: &str = "worker-a-secret";
const SCOPE: &str = "depot-ledger";
const SKEW_MS: u64 = 100;

fn wall_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
}

/// The resource: a ledger that commits each operation exactly once. The dedup row and the business
/// row change together, the way the SQLite reference destination does it, in memory here.
struct Ledger {
    dedup: Mutex<HashMap<String, u64>>,
    rows:  Mutex<Vec<(String, u64)>>,
}

impl EffectDestination for Ledger {
    fn identity(&self) -> &str {
        "depot-ledger"
    }
    fn apply(&self, effect: &Effect) -> Result<DestinationCommit, EffectRefusal> {
        let mut dedup = self.dedup.lock().unwrap();
        match dedup.get(effect.operation_id.as_str()) {
            Some(h) if *h == effect.content_hash => Ok(DestinationCommit::new("depot-ledger", DedupOutcome::Replayed)),
            Some(h) => Err(EffectRefusal::Conflict {
                operation_id: effect.operation_id.clone(),
                committed_hash: *h,
                presented_hash: effect.content_hash,
            }),
            None => {
                let kg: u64 = String::from_utf8_lossy(&effect.payload).parse().map_err(|_| EffectRefusal::Failed("not a figure".into()))?;
                self.rows.lock().unwrap().push((effect.operation_id.as_str().to_string(), kg));
                dedup.insert(effect.operation_id.as_str().to_string(), effect.content_hash);
                Ok(DestinationCommit::new("depot-ledger", DedupOutcome::Fresh))
            }
        }
    }
}

fn step(n: u8, title: &str) {
    println!("\n\x1b[1m{n}. {title}\x1b[0m");
}
fn note(s: impl AsRef<str>) {
    println!("   {}", s.as_ref());
}

#[tokio::main]
async fn main() {
    let gossip: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(57900);
    let http = gossip + 1;
    let me = NodeId::new("127.0.0.1", gossip).expect("node id");
    let cert_dir = std::env::temp_dir().join(format!("myc-composed-commit-{gossip}"));
    let journal_path = std::env::temp_dir().join(format!("myc-composed-commit-{gossip}-journal"));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let _ = std::fs::remove_dir_all(&journal_path);
    let _ = std::fs::remove_file(&journal_path);

    // ── the node: a gateway, an evaluator, an execution authority, provider enforcement ─────────
    let authority_key = SigningKey::from_bytes(&[93u8; 32]);
    let holder_key = SigningKey::from_bytes(&[94u8; 32]);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip;
    cfg.http_port = Some(http);
    cfg.gateway_identity_issuer = Some("gw".into());
    cfg.gateway_named_tokens =
        vec![GatewayNamedToken { name: "worker-a".into(), token: TOKEN.into(), scopes: vec!["*".into()] }];
    cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
    let agent = Arc::new(GossipAgent::new(me.clone(), cfg));
    let resource = format!("tool:ledger.record@{me}");

    agent.with_action_evaluator(Arc::new(
        ReferenceEvaluator::new("rev-ledger")
            .with_catalogue("cat-ledger", "1")
            .map_action("tools/call", resource.clone())
            .allow(Rule::new("*", "tools/call", resource.clone()).requiring_mandate(SCOPE)),
    ));
    let mut entitlements = EntitlementTable::new();
    entitlements.entitle(SCOPE, PrincipalId::new(AUTHORITY).unwrap());
    let mut external = TrustedExternalIssuers::new();
    external.trust(IssuerId::new(AUTHORITY).unwrap(), authority_key.verifying_key().to_bytes()).unwrap();
    external.trust(IssuerId::new(CALLER).unwrap(), holder_key.verifying_key().to_bytes()).unwrap();
    let freshness = FreshnessPolicy { freshness_ms: 2_000, interval_ms: 1_000, delivery_ms: 500 };
    let gate = ExecutionGate::strict(ResourceAuthority::new(SCOPE, 1), ResourceTier::Serialised, ClockModel { skew_ms: SKEW_MS }, freshness)
        .expect("a valid profile");
    agent.with_execution_authority(Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(entitlements), external)));
    agent.with_provider_enforcement();
    let journal = EvidenceJournal::open(&journal_path, EvidenceProfile::Strict).expect("journal");
    agent.with_evidence_journal(Arc::clone(&journal));
    agent.start().await.expect("start");

    // Revocation checkpoints keep the strict gate's view fresh; nothing is revoked here.
    {
        let (agent, key, seq) = (Arc::clone(&agent), authority_key.clone(), Arc::new(AtomicU64::new(0)));
        let issue = move || {
            let c = RevocationCheckpoint {
                authority: PrincipalId::new(AUTHORITY).unwrap(),
                scope: SCOPE.into(),
                seq: seq.fetch_add(1, Ordering::SeqCst) + 1,
                issued_at_ms: wall_ms(),
                revoked: Default::default(),
            };
            let signed = SignedRevocationCheckpoint { signature: key.sign(&c.canonical_bytes()).to_bytes().to_vec(), checkpoint: c };
            agent.offer_revocation_checkpoint(&signed);
        };
        issue();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(1_000)).await;
                issue();
            }
        });
    }

    // ── the tool: commits at the ledger WITH the composition built from its caller ─────────────
    let ledger = Arc::new(Ledger { dedup: Mutex::default(), rows: Mutex::default() });
    // The resource's own authority state: what epoch it has installed. Separate from the
    // gateway's gate on purpose — act 2 moves this one and not that one.
    let ledger_authority = Arc::new(Mutex::new(ResourceAuthority::new(SCOPE, 1)));
    let _tool = {
        let (ledger, ledger_authority, journal, resource, via) =
            (Arc::clone(&ledger), Arc::clone(&ledger_authority), Arc::clone(&journal), resource.clone(), me.clone());
        agent.mcp().register_mcp_tool_with_principal(
            "ledger.record",
            serde_json::json!({"type": "object", "properties": {"operation_id": {"type": "string"}, "kg": {"type": "integer"}}}),
            move |principal, args| {
                let (ledger, ledger_authority, journal, resource, via) =
                    (Arc::clone(&ledger), Arc::clone(&ledger_authority), Arc::clone(&journal), resource.clone(), via.clone());
                async move {
                    let RequestPrincipal::Client(caller) = principal else {
                        return Err("this ledger records only for a gateway client with a mandate".to_string());
                    };
                    // The composition, from what the gateway already established. No mandate → no
                    // composition, said by name, before anything touches the ledger.
                    let composition = Composition::from_caller(&caller, "tools/call", &resource, None)?;
                    let op = args["operation_id"].as_str().unwrap_or("").to_string();
                    let kg = args["kg"].as_u64().unwrap_or(0);
                    let operation = OperationId::new(op.clone());
                    let attempt = AttemptId::of(&operation, 1);
                    let composed = ComposedEffect { effect: Effect::new(operation, attempt, kg.to_string().into_bytes()), composition };
                    let now = wall_ms();
                    let outcome = {
                        let authority = ledger_authority.lock().unwrap();
                        ledger.apply_composed(&composed, &authority, now)
                    };
                    match outcome {
                        Ok(commit) => Ok(serde_json::json!({
                            "dedup": format!("{:?}", commit.dedup).to_lowercase(),
                            "rows": ledger.rows.lock().unwrap().len(),
                        })),
                        Err(EffectRefusal::Unauthorised { leg, reason }) => {
                            // The refusal is evidence: a denied decision at enforcement point
                            // `destination`, beside the gateway's permit.
                            let envelope = ActionEnvelope::builder(caller.principal.clone(), via, "tools/call", resource.clone())
                                .identities(op.clone(), format!("{op}#1"))
                                .validity(now, now + 60_000)
                                .build();
                            let record = AeEvidence::for_destination_refusal(&envelope, &leg.to_string(), reason.clone());
                            let _ = journal.append(serde_json::to_vec(&record).expect("a record serialises")).await;
                            Err(format!("refused at the destination ({leg}): {reason}"))
                        }
                        Err(other) => Err(other.to_string()),
                    }
                }
            },
        )
    };
    tokio::time::sleep(Duration::from_millis(300)).await;

    // ── the client: a named token, a signed grant, a possession proof over each call ─────────────
    let mandate = Mandate {
        holder: PrincipalId::new(CALLER).unwrap(),
        established_by: PrincipalId::new(AUTHORITY).unwrap(),
        purpose: "record donations at the depot ledger".into(),
        scope: SCOPE.into(),
        operations: vec!["tools/call:tool:ledger.record".into()],
        epoch: 1,
        term: TermId::new("t1").unwrap(),
        valid_from_ms: 0,
        valid_until_ms: wall_ms() + 600_000,
    };
    let grant = SignedMandateGrant { signature: authority_key.sign(&mandate.canonical_bytes()).to_bytes().to_vec(), mandate };
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{http}/mcp");
    let health = format!("http://127.0.0.1:{http}/health");
    for _ in 0..60 {
        if client.get(&health).send().await.is_ok_and(|r| r.status().is_success()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let call = |args: serde_json::Value| {
        let (c, u, grant, resource, holder) = (client.clone(), url.clone(), grant.clone(), resource.clone(), holder_key.clone());
        async move {
            let proof = holder.sign(&possession_message(&grant.mandate, &possession_request("tools/call", &resource, &arguments_digest(&args))));
            let presented = serde_json::to_value(PresentedMandate {
                grant,
                possession: base64::engine::general_purpose::STANDARD.encode(proof.to_bytes()),
            })
            .unwrap();
            let body = serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {"name": "ledger.record", "arguments": args, "_meta": {"mandate": presented}},
            });
            c.post(&u).bearer_auth(TOKEN).json(&body).send().await.expect("mcp request")
                .json::<serde_json::Value>().await.expect("json reply")
        }
    };
    let text_of = |r: &serde_json::Value| -> serde_json::Value {
        r["result"]["content"][0]["text"].as_str().and_then(|t| serde_json::from_str(t).ok()).unwrap_or(serde_json::Value::Null)
    };

    println!("\x1b[1mOne gateway, one tool, one ledger — and the composition travels with the effect\x1b[0m");
    note(format!("caller {CALLER}, mandate for {SCOPE} epoch 1; the ledger has installed epoch 1 too"));

    step(1, "record collection-1, 120 kg — a composed effect that holds");
    let r = call(serde_json::json!({"operation_id": "collection-1", "kg": 120})).await;
    assert!(r.get("error").is_none(), "must commit: {r}");
    let out = text_of(&r);
    assert_eq!(out["dedup"], "fresh", "{r}");
    note(format!("gateway permitted; ledger committed — {out}"));
    let r = call(serde_json::json!({"operation_id": "collection-1", "kg": 120})).await;
    let out = text_of(&r);
    assert_eq!(out["dedup"], "replayed", "{r}");
    assert_eq!(out["rows"], 1);
    note(format!("the same operation again — {out}: one effect, one row"));

    step(2, "the ledger installs epoch 2; the gateway's gate still holds epoch 1");
    assert!(ledger_authority.lock().unwrap().install(2));
    let r = call(serde_json::json!({"operation_id": "collection-2", "kg": 80})).await;
    let err = r.get("error").expect("the destination must refuse a superseded mandate");
    let msg = err["message"].as_str().unwrap_or("");
    assert!(msg.contains("refused at the destination (authority)"), "{r}");
    assert!(msg.contains("superseded"), "{r}");
    assert_eq!(ledger.rows.lock().unwrap().len(), 1, "nothing was applied");
    note(format!("gateway permitted (its gate has epoch 1); ledger refused — {msg}"));
    note("✓ permitted at the gateway, refused at the resource: exclusivity is enforced where the effect commits");

    step(3, "the journal holds both decisions");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let records: Vec<AeEvidence> = read_evidence_journal(&journal_path)
        .expect("read journal")
        .into_iter()
        .filter_map(|b| serde_json::from_slice(&b).ok())
        .collect();
    let gateway_permits = records
        .iter()
        .filter(|e| e.kind == RecordKind::Decided && e.decision == DecisionKind::Permit && e.enforcement_point != "destination")
        .count();
    let destination_refusals: Vec<&AeEvidence> = records
        .iter()
        .filter(|e| e.enforcement_point == "destination")
        .collect();
    assert!(gateway_permits >= 3, "the gateway's permits are in the journal: {gateway_permits}");
    assert_eq!(destination_refusals.len(), 1, "one refusal at the destination");
    let refusal = destination_refusals[0];
    assert_eq!(refusal.decision, DecisionKind::Deny);
    assert_eq!(refusal.execution, Execution::None);
    assert_eq!(refusal.subject, CALLER);
    note(format!("{} permit(s) at the gateway/provider, 1 denial at `destination` for {} — execution {:?}", gateway_permits, refusal.subject, refusal.execution));
    note("✓ a reader of the journal sees which point said no; the refused attempt left no row and no dedup entry");

    println!("\n\x1b[1mThe composed sentence, enforced: attributed and authorised at the resource, recorded either way.\x1b[0m");
    println!("Not shown here: the domain leg — carried, not re-verified, because a destination holds no trust bundle.");
    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
    let _ = std::fs::remove_dir_all(&journal_path);
    let _ = std::fs::remove_file(&journal_path);
}
