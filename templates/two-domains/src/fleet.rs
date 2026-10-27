//! `federation.toml` read once, and what each node attaches before `start()` — the lifecycle boundary, after which
//! `secure-multi-domain` ignores a federation attachment rather than trust one its check never saw.
use ed25519_dalek::SigningKey;
use mycelium::federation::call::CallPolicy;
use mycelium::federation::client::{FederationClient, GatewayEndpoint};
use mycelium::federation::edge::FederationEdge;
use mycelium::federation::{DomainId, DomainPolicy, TrustBundle};
use mycelium::GossipAgent;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Domain {
    signing_seed: PathBuf,
    operator_key: PathBuf,
    gateway: String,
    exports: Vec<String>,
    #[serde(default)]
    grants: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    trust: BTreeMap<String, PathBuf>,
    #[serde(default)]
    max_in_flight_per_partner: usize,
    #[serde(default)]
    permit_actor: Option<String>,
    #[serde(default)]
    partner: BTreeMap<String, Partner>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Partner {
    endpoints: Vec<String>,
    pin_file: PathBuf,
    key_file: PathBuf,
    permit: BTreeMap<String, Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fleet {
    domains: BTreeMap<String, Domain>,
}

/// A configuration this program will not run under: said by name, exit 3 (the code `start()`'s refusal uses here).
pub fn refuse(why: &str) -> ! {
    eprintln!("refused: {why}");
    std::process::exit(3)
}

pub fn node_path(node: &str) -> PathBuf {
    PathBuf::from(format!("nodes/{node}.toml"))
}

fn read(path: &PathBuf) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| refuse(&format!("{}: {e} (run `bash fleet.sh setup` first)", path.display())))
}

fn hex32(path: &PathBuf) -> [u8; 32] {
    let s = read(path);
    let s = s.trim();
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        refuse(&format!("{}: expected 64 hex characters", path.display()));
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).expect("checked hex");
    }
    out
}

/// Write a secret readable by its owner only (0600 on unix), refusing to replace one: a key or a token is made once.
fn write_secret(path: &PathBuf, contents: &str) {
    use std::io::Write as _;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut f = opts.open(path).unwrap_or_else(|e| refuse(&format!("{}: {e} (a secret is made once)", path.display())));
    f.write_all(contents.as_bytes()).unwrap_or_else(|e| refuse(&format!("{}: {e}", path.display())));
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn domain_id(name: &str) -> DomainId {
    DomainId::new(name).unwrap_or_else(|e| refuse(&format!("domain {name:?}: {e:?}")))
}

/// Which domain a node is in: its file's `cluster_name`, which here names the domain (the boundary is the CA).
fn domain_of(node: &str) -> String {
    let cfg = mycelium::GossipConfig::load_from_file(node_path(node)).unwrap_or_else(|e| refuse(&e.to_string()));
    cfg.cluster_name.unwrap_or_else(|| refuse(&format!("nodes/{node}.toml names no cluster_name (its domain)")))
}

/// Every node of a domain, as its id: the providers a gateway's policy names.
fn members_of(domain: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir("nodes").unwrap_or_else(|e| refuse(&format!("nodes/: {e}"))).flatten() {
        let name = entry.file_name().to_string_lossy().trim_end_matches(".toml").to_string();
        let cfg = mycelium::GossipConfig::load_from_file(entry.path()).unwrap_or_else(|e| refuse(&e.to_string()));
        if cfg.cluster_name.as_deref() == Some(domain) {
            out.push((name, format!("{}:{}", cfg.bind_address, cfg.bind_port)));
        }
    }
    out.sort();
    out
}

impl Fleet {
    pub fn load() -> Result<Self, String> {
        toml::from_str(&std::fs::read_to_string("federation.toml").map_err(|e| format!("federation.toml: {e}"))?)
            .map_err(|e| format!("federation.toml: {e}"))
    }

    fn domain(&self, name: &str) -> &Domain {
        self.domains.get(name).unwrap_or_else(|| refuse(&format!("federation.toml has no [domains.\"{name}\"]")))
    }

    /// A member that provides its domain's exports (every node but the gateway).
    pub fn serves(&self, node: &str) -> bool {
        let d = self.domain(&domain_of(node));
        d.gateway != node && !d.exports.is_empty()
    }

    /// The other nodes of `node`'s domain, as ids.
    pub fn domain_peers(&self, node: &str) -> Vec<mycelium::NodeId> {
        members_of(&domain_of(node)).into_iter().filter(|(n, _)| n != node).filter_map(|(_, id)| id.parse().ok()).collect()
    }

    pub fn exports_of(&self, node: &str) -> Vec<String> {
        self.domain(&domain_of(node)).exports.clone()
    }

    /// What `node` attaches: the authority every node carries (evaluator, evidence journal, execution authority), and
    /// on a gateway its side of the edge — the serving edge where the domain exports, the client where it has a partner.
    pub fn attach(&self, node: &str, agent: GossipAgent) -> GossipAgent {
        let name = domain_of(node);
        let d = self.domain(&name);
        let me = domain_id(&name);
        let providers: Vec<String> = members_of(&name).into_iter().filter(|(n, _)| *n != d.gateway).map(|(_, id)| id).collect();
        let mut rules = Vec::new();
        if let Some(actor) = &d.permit_actor {
            for export in &d.exports {
                for p in &providers {
                    rules.push((actor.clone(), "skill.invoke".to_string(), format!("skill:{export}@{p}")));
                }
            }
        }
        for (partner, p) in &d.partner {
            for (caller, exports) in &p.permit {
                for export in exports {
                    rules.push((caller.clone(), "federation.call".to_string(), format!("export:{export}@{partner}")));
                }
            }
        }
        // A member holds only the operator's public key, the root its mandates verify under; the domain's signing key
        // is read on the gateway alone, which is the one node that signs (#695's review, finding 2).
        crate::authority::attach(&agent, node, &name, hex32(&d.operator_key), rules);
        if d.gateway != node {
            return agent;
        }
        let seed = SigningKey::from_bytes(&hex32(&d.signing_seed));
        let mut agent = agent;
        if !d.exports.is_empty() {
            let grants = d.grants.iter().flat_map(|(p, es)| es.iter().map(move |e| (domain_id(p), e.clone()))).collect();
            let trust = TrustBundle::trusting(d.trust.iter().map(|(p, key)| (domain_id(p), hex32(key))));
            let edge = FederationEdge::new(
                me.clone(),
                d.exports.clone(),
                DomainPolicy { domain: me.clone(), revision: 1, grants },
                trust,
                CallPolicy { require_body_binding: true, max_in_flight_per_partner: d.max_in_flight_per_partner, ..CallPolicy::default() },
            )
            .with_signing_key(seed.clone());
            agent = agent.with_a2a().with_federation_edge(Arc::new(edge));
        }
        let clients: Vec<Arc<FederationClient>> = d.partner.iter().map(|(partner, p)| {
            let endpoints = p.endpoints.iter().enumerate().map(|(i, url)| GatewayEndpoint { id: format!("{partner}#{i}"), base_url: url.clone() }).collect();
            // The principal named here is a fallback for calls made in code; a call through `/gateway/federation/call`
            // names the authenticated local caller instead (`fed.caller_named`).
            Arc::new(
                FederationClient::new(me.clone(), format!("node:{node}"), seed.clone(), domain_id(partner), endpoints, 8, Duration::from_secs(60))
                    .with_partner_key(hex32(&p.key_file))
                    .with_tls_pins(vec![hex32(&p.pin_file)]),
            )
        }).collect();
        if !clients.is_empty() {
            agent = agent.with_federation_clients(clients);
        }
        agent
    }
}

/// `keys <domain>`: the domain's two keys, each made once. The **signing** key signs its catalogue and its calls'
/// credentials: its seed is for the gateway alone (`secrets/<domain>.seed`), its public half for the partner
/// (`exchange/<domain>.pub`). The **operator** key is the root the domain's own mandates verify under: its seed stays
/// with the operator (`secrets/<domain>.operator.seed`) and no node reads it; every node reads its public half
/// (`exchange/<domain>.operator.pub`).
pub fn keys(domain: &str) -> i32 {
    std::fs::create_dir_all("secrets").and_then(|_| std::fs::create_dir_all("exchange")).unwrap_or_else(|e| refuse(&e.to_string()));
    for (what, seed) in [("signing", format!("secrets/{domain}.seed")), ("operator", format!("secrets/{domain}.operator.seed"))] {
        let key = SigningKey::generate(&mut rand_core::OsRng);
        write_secret(&PathBuf::from(&seed), &hex(&key.to_bytes()));
        let public = seed.replace("secrets/", "exchange/").replace(".seed", ".pub");
        std::fs::write(&public, hex(&key.verifying_key().to_bytes())).unwrap_or_else(|e| refuse(&e.to_string()));
        println!("{domain}: {what} key {seed} (secret) and {public}");
    }
    0
}

/// `tokens <gateway>`: the gateway's named-token table, one line in the `GOSSIP_GATEWAY_NAMED_TOKENS` form
/// (`name|token|scope,…;…`) — each local caller its domain's `permit` names, with the federation scopes, and an
/// `operator` that may only read the fleet. The node and `doctor` read it from the environment (`fleet.sh`).
pub fn tokens(node: &str) -> i32 {
    let fleet = Fleet::load().unwrap_or_else(|e| refuse(&e));
    let d = fleet.domain(&domain_of(node));
    let token = || { let mut b = [0u8; 16]; rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut b); hex(&b) };
    let mut entries: Vec<String> = d.partner.values().flat_map(|p| p.permit.keys())
        .map(|caller| caller.rsplit('/').next().unwrap_or(caller).to_string())
        .map(|name| format!("{name}|{}|federation:invoke,federation:read,fleet:read", token()))
        .collect();
    entries.push(format!("operator|{}|fleet:read", token()));
    let path = format!("secrets/{node}.tokens");
    write_secret(&PathBuf::from(&path), &entries.join(";"));
    println!("{node}: {path} ({} named caller(s))", entries.len());
    0
}

/// `tamper <node> <export>`: a credential `node`'s domain signed for one body, presented to its partner's edge with
/// another — what a payload rewritten between the two domains looks like there. It holds the domain's own key, so it
/// shows the edge's check and nothing about who might be on the path. Exits 0 when the edge refuses the rewritten body.
pub async fn tamper(node: &str, export: &str) -> i32 {
    use mycelium::federation::{call::FederatedCaller, edge::PresentedCall};
    let fleet = Fleet::load().unwrap_or_else(|e| refuse(&e));
    let name = domain_of(node);
    let d = fleet.domain(&name);
    let (partner, p) = d.partner.iter().next().unwrap_or_else(|| refuse(&format!("{name} has no partner")));
    let caller = p.permit.keys().next().cloned().unwrap_or_else(|| refuse(&format!("{name} permits no local caller")));
    let body = |text: &str| serde_json::to_vec(&serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tasks/send",
        "params": { "skillId": export, "message": { "role": "user", "parts": [{ "type": "text", "text": text }] } },
    })).expect("serialise");
    let (signed_for, sent) = (body("stock?"), body("reserve every crate you hold"));
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_millis() as u64;
    let presented = PresentedCall::sign(
        &FederatedCaller {
            origin_domain: domain_id(&name),
            principal: caller,
            export: export.into(),
            issued_at_ms: now,
            expires_at_ms: now + 60_000,
            body_sha256: Some(FederatedCaller::digest_of(&signed_for)),
        },
        &SigningKey::from_bytes(&hex32(&d.signing_seed)),
    );
    let tls = mycelium::federation::pinning::pinned_client_config(vec![hex32(&p.pin_file)]);
    let http = reqwest::Client::builder().use_preconfigured_tls(tls).build().expect("a pinned client");
    let url = format!("{}/a2a", p.endpoints[0]);
    let reply = http.post(&url).header("x-mycelium-federation-call", presented.to_header_value())
        .header("content-type", "application/json").body(sent).send().await;
    let text = match reply {
        Ok(r) => r.text().await.unwrap_or_default(),
        Err(e) => refuse(&format!("{url}: {e}")),
    };
    println!("{partner}'s edge answered a credential signed for another body: {text}");
    if text.contains("binds a different request body") { 0 } else { 1 }
}

/// `get|post <node> <path> [json]`: what a local caller does at its own domain's gateway — HTTPS verified against that
/// domain's CA (`ca/<domain>/ca-cert.pem`), its named token as the bearer (`secrets/<node>.tokens`). Prints the status
/// and the body; exits 0 on a 2xx.
pub async fn local_call(node: &str, path: &str, body: Option<&str>) -> i32 {
    let cfg = mycelium::GossipConfig::load_from_file(node_path(node)).unwrap_or_else(|e| refuse(&e.to_string()));
    let port = cfg.http_port.unwrap_or_else(|| refuse(&format!("{node} runs no gateway")));
    // The CA certificate the local caller is given — the one in the gateway's own directory, never the issuer's `ca/`.
    let ca = read(&PathBuf::from(format!("certs/{node}/ca-cert.pem")));
    // The first entry: the local caller the domain's policy permits across the edge (`tokens`), or `operator`.
    let tokens = read(&PathBuf::from(format!("secrets/{node}.tokens")));
    let token = tokens.split(';').next().and_then(|e| e.split('|').nth(1))
        .unwrap_or_else(|| refuse(&format!("secrets/{node}.tokens: expected name|token|scopes")));
    // rustls, as the node's own clients use: the platform's TLS stack may not speak the Ed25519 the node's key is.
    let http = reqwest::Client::builder()
        .use_rustls_tls()
        .add_root_certificate(reqwest::Certificate::from_pem(ca.as_bytes()).unwrap_or_else(|e| refuse(&format!("ca-cert.pem: {e}"))))
        .build()
        .expect("an HTTPS client");
    let url = format!("https://{}:{port}{path}", cfg.http_addr);
    let req = match body {
        Some(b) => http.post(&url).header("content-type", "application/json").body(b.to_string()),
        None => http.get(&url),
    };
    match req.bearer_auth(token.trim()).send().await {
        Ok(r) => {
            let status = r.status();
            println!("{} {}", status.as_u16(), r.text().await.unwrap_or_default());
            if status.is_success() { 0 } else { 1 }
        }
        Err(e) => {
            let mut why = e.to_string();
            let mut cause = std::error::Error::source(&e);
            while let Some(c) = cause {
                why = format!("{why}: {c}");
                cause = c.source();
            }
            refuse(&format!("{url}: {why}"))
        }
    }
}
