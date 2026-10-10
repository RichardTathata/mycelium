//! Declarative local capability configuration.
//!
//! A node declares what external services it hosts and what mesh capabilities
//! to advertise when those services respond to health probes. The probe-and-
//! advertise lifecycle is started by calling [`run_capability_probes`].
//!
//! ## Separation of concerns
//!
//! [`GossipConfig`](crate::GossipConfig) owns *how* this node connects to the
//! mesh (ports, peers, TTL).  [`NodeCapabilityConfig`] owns *what* this node
//! offers — which external services are co-located and what capability shape
//! to advertise when they are healthy.
//!
//! ## TOML format
//!
//! ```toml
//! # One [[capability]] block per advertised capability.
//! # Multiple blocks with the same ns/name are valid (e.g. several installable models).
//!
//! [[capability]]
//! ns                 = "llm"
//! name               = "inference"
//! probe_url          = "http://localhost:11434/api/tags"
//! probe_timeout_secs = 3
//! ttl_secs           = 30
//!
//!   [capability.attrs]
//!   model    = "llama3.2"
//!   context  = 8192
//!   backend  = "ollama"
//!   endpoint = "http://localhost:11434/v1"
//!
//! [[capability]]
//! ns       = "data"
//! name     = "realtime"
//! ttl_secs = 60
//!   # no probe_url → always-alive
//! ```
//!
//! `probe_url` is optional. When absent the capability is treated as
//! always-alive — useful for in-process capabilities (MCP tool handlers,
//! compute functions) that don't have a separate health endpoint.
//!
//! ## The unit file (design-time tooling, W1 / L1 / R1)
//!
//! The same file can carry everything a deployable **unit** declares — what it *requires*, the
//! *groups* it defines, the *lanes* it feeds or drains, the *authority* it expects, what it
//! *hosts*, and the *presence* it keeps — beside the capabilities above. Every section is optional
//! and every existing `[[capability]]`-only file loads unchanged. The plan that argues the format
//! is `docs/plans/design-time-tooling.md` §3; this is that format, and the loader is its only parser.
//!
//! ```toml
//! principal = "planner"                  # the principal this unit presents; the join key (D9)
//!
//! [[requirement]]                        # declare_requirement, in a file
//! ns   = "llm"
//! name = "inference"
//! schema_id = "llm.inference.v2"         # exact match, as CapFilter::schema_id
//!   [requirement.attrs]
//!   model   = "llama3.2"                 # a bare value is Eq
//!   context = { gte = 8192 }             # gt | gte | lt | lte | ne | eq
//!   engine  = { version = "2.1.0" }      # a Version value, anywhere a value goes
//!   [requirement.ranking]
//!   attribute = "context"
//!   order = "descending"
//!
//! [[group]]                              # define_capability_group, in a file
//! name = "routers"
//!   [group.filter]
//!   ns   = "plan"
//!   name = "route"
//!   [[group.provides]]
//!   ns   = "plan"
//!   name = "routing"
//!   [[group.requires]]
//!   ns   = "data"
//!   name = "realtime"
//!
//! [[lane]]                               # a tuple-space stage this unit touches
//! name = "stage-b"
//! role = "consumes"                      # produces | consumes
//!
//! [[mandate]]                            # the authority vocabulary this unit expects
//! holder = "planner"
//! scope  = "routers"
//! operations = ["plan.route", "plan.reroute"]
//!
//! [[rule]]                               # reference-evaluator rules, the same fields as Rule
//! actor     = "planner"
//! operation = "plan.route"
//! resource  = "*"
//! requires_mandate = "routers"
//!
//! [hosts]                                # this unit runs a provisioner (D15)
//! kinds = ["wasm-component", "blob"]
//! install_budget_bytes = 8589934592
//! headroom = 0.8
//! # a stem refuses anything but ed25519:<64 hex>; this key is the public half of the test seed 42…42
//! trusted_publishers  = ["ed25519:2152f8d19b791d24453242e15f2eab6cb7cffa7b6a5ed30097960e069881db12"]
//! operator_publishers = ["ed25519:2152f8d19b791d24453242e15f2eab6cb7cffa7b6a5ed30097960e069881db12"]
//! fuel_per_call = 50000000               # every trusted key not in operator_publishers is an agent: metered (D19)
//! placement_root = "/var/lib/mycelium/artifacts"
//!
//! [[activation]]                         # a placed blob, handed to the local runtime (X2)
//! ns   = "llm"
//! name = "storyteller-deploy"
//! command = ["ollama", "create", "storyteller", "-f", "{rendered}"]
//! probe   = ["ollama", "show", "storyteller"]
//! resolve_artifact_refs = true           # `FROM artifact:<hex>` → the placed path
//!
//! [[serve]]                              # a routable skill while a local install is live (X2)
//! name     = "storyteller"               # ns defaults to "llm"
//! endpoint = "http://localhost:11434/v1"
//! model    = "storyteller"
//!   [serve.while_live]
//!   ns   = "llm"
//!   name = "storyteller-deploy"
//!
//! [[presence]]                           # keep 2–4 route optimizers alive fleet-wide (D17)
//! ns   = "route"
//! name = "optimize"
//! min_providers = 2
//! max_providers = 4
//! ```
//!
//! Every field, with its type and default, is in `docs/reference/unit-file.md`.
//!
//! What the loader **does**: parse, then [`NodeCapabilityConfig::validate`]. Each of these is
//! refused **by name**: an empty `principal`; an unknown constraint operator, or a constraint table
//! naming more than one; a `Version` that does not parse; a ranking with an unknown order; a group,
//! lane or rule with an empty name or field; a mandate with an empty holder or scope, or one that
//! enumerates nothing; a presence floor of zero or a ceiling below it; a `[hosts]` table naming an
//! unknown kind, a headroom outside `(0, 1]`, a `fuel_per_call`, `operator_fuel_per_call` or
//! `call_deadline_ms` of `0`,
//! or `operator_publishers` that are not all in `trusted_publishers`; an `[[activation]]` with an
//! empty `command`, `timeout_secs = 0`, a placeholder outside `{path} {dir} {artifact} {ns} {name}
//! {rendered}`, `{rendered}` without `resolve_artifact_refs = true`, or in a unit whose `[hosts]`
//! does not name the `blob` kind; and a `[[serve]]` with an empty `endpoint` or `model`, or whose
//! `while_live` names the skill itself.
//!
//! Who reads the file at runtime: a stem (`mycelium-stem --units <file>`, a binary of
//! `mycelium-wasm-host` built with feature `stem`) declares every section at startup. An SDK agent
//! declares the non-hosting sections through `POST /gateway/units/declare`, which refuses
//! `[hosts]`, `[[presence]]`, `[[activation]]` and `[[serve]]` and reports lanes, mandates and rules
//! as not enforced. The `mycelium` node binary does not read the file; an application drives the
//! `[[capability]]` probe loop itself with [`run_capability_probes`]. `mycelium wire-check` reads
//! every section except `[[activation]]`, and counts a `[[serve]]` skill as an offer.

use crate::error::GossipError;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::Path,
};
use crate::{CapConstraint, CapFilter, CapRanking, CapValue, RankingOrder};
#[cfg(feature = "gateway")]
use crate::{Capability, CapabilityReg, GossipAgent};
#[cfg(feature = "gateway")]
use std::{
    sync::{atomic::{AtomicBool, Ordering}, Arc},
    time::Duration,
};

// ── Config types ──────────────────────────────────────────────────────────────

/// Top-level node capability configuration.
///
/// Load with [`NodeCapabilityConfig::load_from_file`], then drive the probe
/// loop with [`run_capability_probes`].
#[derive(Debug, Default, Clone, Deserialize)]
pub struct NodeCapabilityConfig {
    /// The issuer-qualified principal this unit presents at a gateway — the one key a runtime
    /// observation also carries, and so the join between a declaration and what the fleet later
    /// reports (plan D9). Not an address.
    #[serde(default)]
    pub principal:    Option<String>,
    /// All capability probe entries declared for this node.
    #[serde(default, rename = "capability")]
    pub capabilities: Vec<CapabilityProbeEntry>,
    /// What this unit requires — each a [`CapFilter`] once converted.
    #[serde(default, rename = "requirement")]
    pub requirements: Vec<FilterDecl>,
    /// The capability groups this unit defines.
    #[serde(default, rename = "group")]
    pub groups:       Vec<GroupDecl>,
    /// The tuple-space lanes this unit feeds or drains, by name.
    #[serde(default, rename = "lane")]
    pub lanes:        Vec<LaneDecl>,
    /// The mandates this unit expects to hold or be checked against.
    #[serde(default, rename = "mandate")]
    pub mandates:     Vec<MandateDecl>,
    /// The reference-evaluator rules this unit expects to be admitted by.
    #[serde(default, rename = "rule")]
    pub rules:        Vec<RuleDecl>,
    /// What this unit can host — present only for a unit that runs a provisioner (D15).
    #[serde(default)]
    pub hosts:        Option<HostsDecl>,
    /// Presence policies this unit publishes — keep N..M providers alive (D17).
    #[serde(default, rename = "presence")]
    pub presence:     Vec<PresenceDecl>,
    /// How a placed blob is handed to the node-local runtime, per capability (X2): the command
    /// run after placement and the probe that gates the capability. A stem only — the offline
    /// check does not read it.
    #[serde(default, rename = "activation")]
    pub activations:  Vec<ActivationDecl>,
    /// Routable model skills this unit serves while a local install is live (X2): the declared
    /// form of `mycelium-reason`'s `serve_model` bridge. A stem only, built with `llm`.
    #[serde(default, rename = "serve")]
    pub serves:       Vec<ServeDecl>,
}

/// A `[[serve]]` — register the prompt skill `{ns}/{name}` (routable over `llm.invoke`, what an
/// `InferenceRouter` resolves) backed by an OpenAI-compatible `endpoint` serving `model`, **while**
/// this node hosts the install named by `while_live` (absent = always). When that install is
/// withdrawn — its probe failed, the floor moved — the skill is retracted with it, so a router
/// fails over to a node that still has the model. `max_tokens` / `temperature` are the template's.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServeDecl {
    #[serde(default = "default_serve_ns")]
    pub ns:   String,
    pub name: String,
    pub endpoint: String,
    pub model:    String,
    #[serde(default)]
    pub while_live: Option<CapDecl>,
    #[serde(default)]
    pub api_key: Option<String>,
    /// The name of an environment variable holding the bearer, read **once at stem start**; an unset
    /// variable refuses the start by name. One of `api_key` / `api_key_env`, never both (zero-gaps Z2).
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
}

fn default_serve_ns() -> String {
    "llm".into()
}

/// An `[[activation]]` — after a blob providing `ns/name` is placed and verified, run `command`
/// (argv, no shell unless you name one) and gate the capability on `probe` (argv; exit 0 is
/// healthy; absent = the placed file exists). Placeholders: `{path}` the placed file, `{dir}` the
/// placement root, `{artifact}` its content address, `{ns}`, `{name}`, and — with
/// `resolve_artifact_refs` — `{rendered}`, a copy of the placed file in which every
/// `artifact:<64 hex>` reference is replaced by that artifact's placed path (a Modelfile's
/// `FROM artifact:…`). A reference not yet placed fails the activation, and the next round
/// retries: ordering without a dependency resolver, as the model demo does in code.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ActivationDecl {
    pub ns:   String,
    pub name: String,
    pub command: Vec<String>,
    #[serde(default)]
    pub probe: Vec<String>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub resolve_artifact_refs: bool,
}

/// The placeholders an `[[activation]]` may use.
pub const ACTIVATION_PLACEHOLDERS: &[&str] = &["path", "dir", "artifact", "ns", "name", "rendered"];

impl ActivationDecl {
    fn placeholders(argv: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        for a in argv {
            let mut rest = a.as_str();
            while let Some(i) = rest.find('{') {
                let after = &rest[i + 1..];
                let Some(j) = after.find('}') else { break };
                out.push(after[..j].to_string());
                rest = &after[j + 1..];
            }
        }
        out
    }
}

/// A semantic-version triple written as `"major.minor.patch"`; parsed at load so a
/// malformed one is refused by the loader, not discovered by a filter that never matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SemverTriple(pub [u32; 3]);

impl<'de> Deserialize<'de> for SemverTriple {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let parts: Vec<&str> = s.trim().split('.').collect();
        if parts.len() != 3 {
            return Err(serde::de::Error::custom(format!(
                "version {s:?} is not major.minor.patch"
            )));
        }
        let mut out = [0u32; 3];
        for (i, part) in parts.iter().enumerate() {
            out[i] = part.parse().map_err(|_| {
                serde::de::Error::custom(format!("version {s:?}: {part:?} is not a number"))
            })?;
        }
        Ok(Self(out))
    }
}

/// A single probe-and-advertise declaration.
///
/// On startup and every 10 s thereafter, [`run_capability_probes`] GETs
/// `probe_url` (if set).  A 2xx response causes the capability to be
/// advertised (or kept alive); any other outcome tombstones it until the
/// probe recovers.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CapabilityProbeEntry {
    /// Capability namespace, e.g. `"llm"`.
    pub ns: String,
    /// Capability name, e.g. `"inference"`.
    pub name: String,
    /// HTTP GET URL to probe for liveness. 2xx = service alive.
    /// Absent = always-alive (no external dependency to check).
    pub probe_url: Option<String>,
    /// Probe request timeout. Default: 3 s.
    #[serde(default = "default_probe_timeout_secs")]
    pub probe_timeout_secs: u64,
    /// Capability re-assertion TTL passed to [`GossipAgent::advertise_capability`].
    /// Default: 30 s.
    #[serde(default = "default_ttl_secs")]
    pub ttl_secs: u64,
    /// Static capability attributes announced to the mesh on probe success.
    #[serde(default)]
    pub attrs: BTreeMap<String, TomlCapValue>,
    /// The schema this capability advertises (guide 12), if any — the exact id a requirement's
    /// `schema_id` must match. Absent means *no schema*, which a requirement with a `schema_id`
    /// never matches.
    #[serde(default)]
    pub schema_id: Option<String>,
}

fn default_probe_timeout_secs() -> u64 { 3 }
fn default_ttl_secs()            -> u64 { 30 }

/// TOML-deserializable capability attribute value.
///
/// Converts directly into [`CapValue`] via [`From`].
///
/// Variant order is load-bearing: serde's untagged deserialization tries
/// each variant in declaration order, so `Bool` must precede `Integer`
/// (otherwise `true`/`false` would be attempted as integers first).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum TomlCapValue {
    /// `{ version = "2.1.0" }` — a [`CapValue::Version`]. A bare string stays `Text`, so every
    /// existing file keeps its meaning; a version is a version only when written as one.
    Version { version: SemverTriple },
    Bool(bool),
    Integer(i64),
    Float(f64),
    Text(String),
}

impl From<TomlCapValue> for CapValue {
    fn from(v: TomlCapValue) -> Self {
        match v {
            TomlCapValue::Version { version } => CapValue::Version(version.0),
            TomlCapValue::Bool(b)    => CapValue::Bool(b),
            TomlCapValue::Integer(n) => CapValue::Integer(n),
            TomlCapValue::Float(f)   => CapValue::Float(f),
            TomlCapValue::Text(s)    => CapValue::Text(s.into()),
        }
    }
}

// ── Unit declarations ─────────────────────────────────────────────────────────

fn invalid(field: &'static str, reason: impl Into<String>) -> GossipError {
    GossipError::InvalidField { field, reason: reason.into() }
}

/// One attribute constraint as written: a bare value means `Eq`; an operator table
/// (`{ gte = 8192 }`) names one of `eq · ne · gt · gte · lt · lte`. The two forms are told apart
/// by shape; an operator the substrate does not have is refused by [`NodeCapabilityConfig::validate`].
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum TomlConstraint {
    Bare(TomlCapValue),
    Op(BTreeMap<String, TomlCapValue>),
}

impl TomlConstraint {
    /// The [`CapConstraint`] this writes, or why it cannot.
    pub fn to_constraint(&self) -> Result<CapConstraint, String> {
        match self {
            Self::Bare(v) => Ok(CapConstraint::Eq(v.clone().into())),
            Self::Op(map) => {
                if map.len() != 1 {
                    return Err(format!(
                        "a constraint table names exactly one operator; got {} keys ({})",
                        map.len(),
                        map.keys().cloned().collect::<Vec<_>>().join(", ")
                    ));
                }
                let (op, v) = map.iter().next().expect("one entry");
                let v: CapValue = v.clone().into();
                Ok(match op.as_str() {
                    "eq"  => CapConstraint::Eq(v),
                    "ne"  => CapConstraint::Ne(v),
                    "gt"  => CapConstraint::Gt(v),
                    "gte" => CapConstraint::Gte(v),
                    "lt"  => CapConstraint::Lt(v),
                    "lte" => CapConstraint::Lte(v),
                    other => return Err(format!(
                        "unknown constraint operator {other:?} (eq · ne · gt · gte · lt · lte)"
                    )),
                })
            }
        }
    }
}

/// A ranking over one attribute, `order = "ascending" | "descending"`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RankingDecl {
    pub attribute: String,
    pub order:     String,
}

/// A filter as written — a `[[requirement]]`, a `[group.filter]`, a `[[group.requires]]`, or the
/// filter half of a `[[presence]]`. Converts to a [`CapFilter`] with [`to_filter`](Self::to_filter).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FilterDecl {
    pub ns:   String,
    pub name: String,
    #[serde(default)]
    pub schema_id: Option<String>,
    #[serde(default)]
    pub attrs:   BTreeMap<String, TomlConstraint>,
    #[serde(default)]
    pub ranking: Option<RankingDecl>,
}

impl FilterDecl {
    /// The [`CapFilter`] this declares — the same value `declare_requirement` takes, so
    /// `CapFilter::matches` on it is the mesh's own rule and not a re-implementation.
    pub fn to_filter(&self) -> Result<CapFilter, String> {
        let mut f = CapFilter::new(self.ns.as_str(), self.name.as_str());
        for (attr, c) in &self.attrs {
            f = f.with(attr.as_str(), c.to_constraint().map_err(|e| format!("attr {attr:?}: {e}"))?);
        }
        if let Some(sid) = &self.schema_id {
            f = f.with_schema(sid.as_str());
        }
        if let Some(r) = &self.ranking {
            let order = match r.order.as_str() {
                "ascending"  => RankingOrder::Ascending,
                "descending" => RankingOrder::Descending,
                other => return Err(format!("ranking order {other:?} is not ascending | descending")),
            };
            f.ranking = Some(CapRanking { attribute: r.attribute.as_str().into(), order });
        }
        Ok(f)
    }
}

/// A capability a group asserts once it has members (`[[group.provides]]`).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CapDecl {
    pub ns:   String,
    pub name: String,
    #[serde(default)]
    pub schema_id: Option<String>,
    #[serde(default)]
    pub attrs: BTreeMap<String, TomlCapValue>,
}

impl CapDecl {
    /// The [`Capability`](crate::Capability) this declares.
    pub fn to_capability(&self) -> crate::Capability {
        let mut cap = crate::Capability::new(self.ns.as_str(), self.name.as_str());
        for (k, v) in &self.attrs {
            cap = cap.with(k.as_str(), v.clone().into());
        }
        if let Some(sid) = &self.schema_id {
            cap = cap.with_schema_id(sid.as_str());
        }
        cap
    }
}

/// A `[[group]]` — the fields of [`CapabilityGroupDef`](crate::CapabilityGroupDef) plus its name.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GroupDecl {
    pub name:   String,
    pub filter: FilterDecl,
    #[serde(default)]
    pub topology_policy: Option<crate::GroupTopologyPolicy>,
    #[serde(default)]
    pub provides: Vec<CapDecl>,
    #[serde(default)]
    pub requires: Vec<FilterDecl>,
}

impl GroupDecl {
    /// The definition `define_capability_group` takes.
    pub fn to_def(&self) -> Result<crate::CapabilityGroupDef, String> {
        Ok(crate::CapabilityGroupDef {
            filter:          self.filter.to_filter().map_err(|e| format!("group {:?} filter: {e}", self.name))?,
            topology_policy: self.topology_policy.clone(),
            provides:        self.provides.iter().map(CapDecl::to_capability).collect(),
            requires:        self
                .requires
                .iter()
                .enumerate()
                .map(|(i, r)| r.to_filter().map_err(|e| format!("group {:?} requires[{i}]: {e}", self.name)))
                .collect::<Result<_, _>>()?,
        })
    }
}

/// Which side of a lane a unit is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LaneRole {
    Produces,
    Consumes,
}

/// A `[[lane]]` — a tuple-space stage by name and the role this unit takes on it. Lanes match
/// nothing on content, so a declaration is a name and a side and nothing more (plan D5).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LaneDecl {
    pub name: String,
    pub role: LaneRole,
}

/// A `[[mandate]]` — the authority vocabulary a unit expects: who holds it, over which scope,
/// which **enumerated** operations. Absence is denial and there is no wildcard, as in
/// [`Mandate`](crate::mandate::Mandate). Epoch, term and validity are runtime facts a
/// declaration does not carry.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MandateDecl {
    pub holder: String,
    pub scope:  String,
    #[serde(default)]
    pub operations: Vec<String>,
    #[serde(default)]
    pub established_by: Option<String>,
    #[serde(default)]
    pub purpose: Option<String>,
}

impl MandateDecl {
    /// Does the declared mandate enumerate `operation`?
    pub fn permits(&self, operation: &str) -> bool {
        self.operations.iter().any(|o| o == operation)
    }
}

/// A `[[rule]]` — the reference evaluator's [`Rule`](crate::Rule), as written: `*` is any;
/// `requires_values` are compared for equality by name.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RuleDecl {
    pub actor:     String,
    pub operation: String,
    pub resource:  String,
    #[serde(default)]
    pub requires_scopes: Vec<String>,
    #[serde(default)]
    pub requires_facts: Vec<String>,
    #[serde(default)]
    pub requires_mandate: Option<String>,
    #[serde(default)]
    pub requires_values: BTreeMap<String, toml::Value>,
}

impl RuleDecl {
    /// The evaluator's own rule type, where the evaluator exists (a gateway that can attest).
    #[cfg(all(feature = "gateway", feature = "tls"))]
    pub fn to_rule(&self) -> Result<crate::Rule, String> {
        let mut rule = crate::Rule::new(self.actor.as_str(), self.operation.as_str(), self.resource.as_str());
        rule.requires_scopes = self.requires_scopes.clone();
        rule.requires_facts = self.requires_facts.clone();
        rule.requires_mandate = self.requires_mandate.clone();
        rule.requires_values = self
            .requires_values
            .iter()
            .map(|(k, v)| serde_json::to_value(v).map(|j| (k.clone(), j)).map_err(|e| format!("requires_values.{k}: {e}")))
            .collect::<Result<_, _>>()?;
        Ok(rule)
    }
}

/// The artifact kinds a hosting unit may name — the names of `ArtifactKind` in the wasm-host
/// crate, which this crate does not depend on.
pub const HOSTABLE_KINDS: &[&str] = &["wasm-component", "blob"];

/// `[hosts]` — what a unit that runs a provisioner can host (plan D15): the kinds it has runtimes
/// for, its install budget, its headroom fraction, the publisher keys it trusts, its placement
/// root, and the fuel rule (D19). These are the provisioner's own settings written down; the
/// stem (`mycelium-stem`, the wasm-host crate) reads them at start.
///
/// **Fuel (D19).** `fuel_per_call` is the budget, in wasm instructions, that an entry published by
/// an *agent* principal runs under: a call that runs past it is stopped and recorded as *fuel
/// exhausted*. `operator_publishers` names the keys whose entries are the operator's own; they
/// run under `operator_fuel_per_call` (absent = unbounded). The classification is the entry's
/// verified signer, so `operator_publishers` must be a subset of `trusted_publishers` — without
/// provenance a signer is a claim, and `validate()` refuses the combination by name.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct HostsDecl {
    #[serde(default)]
    pub kinds: Vec<String>,
    #[serde(default)]
    pub install_budget_bytes: Option<u64>,
    #[serde(default)]
    pub headroom: Option<f64>,
    #[serde(default)]
    pub trusted_publishers: Vec<String>,
    #[serde(default)]
    pub placement_root: Option<String>,
    #[serde(default)]
    pub fuel_per_call: Option<u64>,
    #[serde(default)]
    pub operator_publishers: Vec<String>,
    #[serde(default)]
    pub operator_fuel_per_call: Option<u64>,
    /// Row D: the wall-clock bound, in milliseconds, on each call into a hosted component (and on
    /// its instantiation), whoever published it — a call past it is interrupted and recorded as
    /// *deadline exceeded*. Absent = the wasm host's default (5 s). `0` is refused.
    #[serde(default)]
    pub call_deadline_ms: Option<u64>,
    /// D20: the reviewer keys whose acceptance promotes a proposed entry from the shadow lane
    /// to a real load. Absent = a proposal never loads for real on this host.
    #[serde(default)]
    pub trusted_reviewers: Vec<String>,
    /// Say on purpose that this host installs entries with no provenance. An empty
    /// `trusted_publishers` is otherwise refused by `validate()` — with no key, any entry any peer
    /// gossips installs here — and a stem warns once at start when this is set. Refused beside a
    /// non-empty `trusted_publishers` (one or the other names the policy).
    #[serde(default)]
    pub accept_unsigned: bool,
}

impl HostsDecl {
    /// Can this host run an entry of `kind`, with a signed footprint of `bytes`? Eligibility only:
    /// which host installs stays the provisioner's runtime self-election.
    pub fn can_host(&self, kind: &str, bytes: u64) -> bool {
        self.kinds.iter().any(|k| k == kind) && self.install_budget_bytes.is_none_or(|b| bytes <= b)
    }
}

/// A `[[presence]]` — keep between `min_providers` and `max_providers` live providers of a filter
/// across the fleet, independent of demand (the wasm-host `SupervisionPolicy`, plan D17).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PresenceDecl {
    #[serde(flatten)]
    pub filter: FilterDecl,
    pub min_providers: usize,
    #[serde(default)]
    pub max_providers: Option<usize>,
}

impl NodeCapabilityConfig {
    /// Every refusal the loader makes, by name. Called by [`load_from_file`](Self::load_from_file)
    /// after parsing; call it yourself on a config built in code.
    pub fn validate(&self) -> Result<(), GossipError> {
        if let Some(p) = &self.principal
            && p.trim().is_empty()
        {
            return Err(invalid("principal", "must not be empty when present"));
        }
        for (i, r) in self.requirements.iter().enumerate() {
            r.to_filter().map_err(|e| invalid("requirement", format!("[{i}] {}/{}: {e}", r.ns, r.name)))?;
        }
        for g in &self.groups {
            if g.name.trim().is_empty() {
                return Err(invalid("group", "a group needs a name"));
            }
            g.to_def().map_err(|e| invalid("group", e))?;
        }
        for l in &self.lanes {
            if l.name.trim().is_empty() {
                return Err(invalid("lane", "a lane needs a name"));
            }
        }
        for m in &self.mandates {
            if m.holder.trim().is_empty() || m.scope.trim().is_empty() {
                return Err(invalid("mandate", "holder and scope must not be empty"));
            }
            if m.operations.is_empty() {
                return Err(invalid("mandate", format!(
                    "{:?} over {:?} enumerates no operations — absence is denial, so it grants nothing",
                    m.holder, m.scope
                )));
            }
        }
        for r in &self.rules {
            if r.actor.is_empty() || r.operation.is_empty() || r.resource.is_empty() {
                return Err(invalid("rule", "actor, operation and resource must not be empty (use \"*\" for any)"));
            }
        }
        if let Some(h) = &self.hosts {
            for k in &h.kinds {
                if !HOSTABLE_KINDS.contains(&k.as_str()) {
                    return Err(invalid("hosts.kinds", format!(
                        "unknown artifact kind {k:?} ({})",
                        HOSTABLE_KINDS.join(" | ")
                    )));
                }
            }
            if let Some(hr) = h.headroom
                && !(hr > 0.0 && hr <= 1.0)
            {
                return Err(invalid("hosts.headroom", format!("{hr} is outside (0, 1]")));
            }
            if h.fuel_per_call == Some(0) {
                return Err(invalid("hosts.fuel_per_call", "a budget of 0 stops every call before its first instruction"));
            }
            if h.operator_fuel_per_call == Some(0) {
                return Err(invalid("hosts.operator_fuel_per_call", "a budget of 0 stops every call before its first instruction"));
            }
            if h.call_deadline_ms == Some(0) {
                return Err(invalid("hosts.call_deadline_ms", "a deadline of 0 stops every call before its first instruction"));
            }
            if !h.operator_publishers.is_empty() && h.trusted_publishers.is_empty() {
                return Err(invalid(
                    "hosts.operator_publishers",
                    "names operator keys but hosts.trusted_publishers is empty: without provenance a signer is a claim, so nothing could tell an operator's entry from an agent's",
                ));
            }
            for k in &h.operator_publishers {
                if !h.trusted_publishers.contains(k) {
                    return Err(invalid(
                        "hosts.operator_publishers",
                        format!("{k:?} is not in hosts.trusted_publishers (an operator key must be a trusted key)"),
                    ));
                }
            }
        }
        for (i, a) in self.activations.iter().enumerate() {
            let at = format!("[{i}] {}/{}", a.ns, a.name);
            if a.command.is_empty() {
                return Err(invalid("activation.command", format!("{at}: an activation with no command does nothing")));
            }
            if a.timeout_secs == Some(0) {
                return Err(invalid("activation.timeout_secs", format!("{at}: a timeout of 0 fails every activation")));
            }
            for p in ActivationDecl::placeholders(&a.command).into_iter().chain(ActivationDecl::placeholders(&a.probe)) {
                if !ACTIVATION_PLACEHOLDERS.contains(&p.as_str()) {
                    return Err(invalid("activation", format!("{at}: unknown placeholder {{{p}}} ({})", ACTIVATION_PLACEHOLDERS.join(" | "))));
                }
                if p == "rendered" && !a.resolve_artifact_refs {
                    return Err(invalid("activation", format!("{at}: {{rendered}} needs resolve_artifact_refs = true")));
                }
            }
            if !self.hosts.as_ref().is_some_and(|h| h.kinds.iter().any(|k| k == "blob")) {
                return Err(invalid("activation", format!("{at}: activations are for placed blobs, and this unit's [hosts] does not name the blob kind")));
            }
        }
        for (i, sv) in self.serves.iter().enumerate() {
            let at = format!("[{i}] {}/{}", sv.ns, sv.name);
            if sv.endpoint.trim().is_empty() || sv.model.trim().is_empty() {
                return Err(invalid("serve", format!("{at}: an endpoint and a model are both required")));
            }
            if sv.api_key.is_some() && sv.api_key_env.is_some() {
                return Err(invalid("serve.api_key", format!("{at}: set one of api_key (a literal) or api_key_env (a variable name), not both")));
            }
            if sv.api_key_env.as_deref().is_some_and(|v| v.trim().is_empty()) {
                return Err(invalid("serve.api_key_env", format!("{at}: api_key_env is empty — name the variable that holds the key")));
            }
            if let Some(w) = &sv.while_live
                && w.ns == sv.ns
                && w.name == sv.name
            {
                return Err(invalid("serve.while_live", format!(
                    "{at}: the skill and the install it waits for share a capability key and would overwrite each other — name the install differently (e.g. `{}-deploy`)", sv.name
                )));
            }
        }
        for (i, p) in self.presence.iter().enumerate() {
            p.filter.to_filter().map_err(|e| invalid("presence", format!("[{i}]: {e}")))?;
            if p.min_providers == 0 {
                return Err(invalid("presence.min_providers", format!(
                    "[{i}] {}/{}: a floor of zero keeps nothing alive", p.filter.ns, p.filter.name
                )));
            }
            if let Some(max) = p.max_providers
                && max < p.min_providers
            {
                return Err(invalid("presence.max_providers", format!(
                    "[{i}] {}/{}: ceiling {max} is below floor {}", p.filter.ns, p.filter.name, p.min_providers
                )));
            }
        }
        // Last, so every other `[hosts]` refusal keeps its own name: a hosting unit with no
        // provenance policy. An empty trusted list is not "provenance not required" — it is "any
        // entry any peer publishes installs here", and the 2.18.1 rule is that a setting a node
        // cannot be seen to mean is refused at start rather than run silently.
        if let Some(h) = &self.hosts {
            if h.accept_unsigned && !h.trusted_publishers.is_empty() {
                return Err(invalid(
                    "hosts.accept_unsigned",
                    "is set beside a non-empty hosts.trusted_publishers: one names the policy — list the keys, or accept unsigned entries, not both",
                ));
            }
            if h.trusted_publishers.is_empty() && !h.accept_unsigned {
                return Err(invalid(
                    "hosts.trusted_publishers",
                    "is empty: with no publisher key, any entry any peer gossips installs here with no provenance — list the keys this host installs from, or set hosts.accept_unsigned = true to run that way on purpose",
                ));
            }
        }
        Ok(())
    }
}

impl NodeCapabilityConfig {
    /// Loads and parses a TOML capability config file.
    ///
    /// The file uses the `[[capability]]` array-of-tables format described in
    /// the module documentation.
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self, GossipError> {
        let s = std::fs::read_to_string(path).map_err(GossipError::Io)?;
        Self::from_toml_str(&s)
    }

    /// Parse and [`validate`](Self::validate) a unit file's text.
    pub fn from_toml_str(s: &str) -> Result<Self, GossipError> {
        let cfg: Self = toml::from_str(s).map_err(GossipError::Toml)?;
        cfg.validate()?;
        Ok(cfg)
    }
}

impl CapabilityProbeEntry {
    /// The [`Capability`](crate::Capability) this entry advertises once its probe passes — the
    /// declared offer the offline check (W2) matches requirements against.
    pub fn build_capability_public(&self) -> crate::Capability {
        let mut cap = crate::Capability::new(self.ns.as_str(), self.name.as_str());
        for (k, v) in &self.attrs {
            cap = cap.with(k.as_str(), v.clone().into());
        }
        if let Some(sid) = &self.schema_id {
            cap = cap.with_schema_id(sid.as_str());
        }
        cap
    }

    #[cfg(feature = "gateway")]
    pub(crate) fn build_capability(&self) -> Capability {
        self.build_capability_public()
    }

    #[cfg(feature = "gateway")]
    pub(crate) async fn passes_probe(
        &self,
        client: &reqwest::Client,
        egress: &crate::config::EgressPolicy,
    ) -> bool {
        let Some(url) = &self.probe_url else { return true };
        // WS3 egress gate: a probe is an outbound reach the node *chooses*.
        if !egress.permits_url(url) {
            tracing::warn!(url = %url, "capability probe blocked by egress policy");
            return false;
        }
        client
            .get(url)
            .timeout(Duration::from_secs(self.probe_timeout_secs))
            .send().await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }
}

// ── Probe events ──────────────────────────────────────────────────────────────

/// Emitted by [`run_capability_probes`] whenever a capability transitions
/// between live and tombstoned.
pub struct ProbeEvent {
    /// Namespace of the capability that changed state.
    pub ns:    String,
    /// Name of the capability that changed state.
    pub name:  String,
    /// New state.
    pub state: ProbeState,
}

/// Whether the probe passed or failed.
pub enum ProbeState {
    /// Probe passed — capability is advertised on the mesh.
    Up,
    /// Probe failed — capability handle dropped (KV tombstoned).
    Down,
}

// ── Probe loop ────────────────────────────────────────────────────────────────

#[cfg(feature = "gateway")]
const HEALTH_INTERVAL_SECS: u64 = 10;

/// Runs the probe-and-advertise loop for all entries in `config`.
///
/// For each entry the loop:
/// - Probes the declared URL immediately on startup (always-alive when no URL).
/// - Calls [`GossipAgent::advertise_capability`] and fires
///   `on_event(ProbeEvent { state: Up })` when the probe passes.
/// - Re-probes every 10 s; drops the [`CapabilityReg`] (tombstoning the
///   KV entry) and fires `on_event(ProbeEvent { state: Down })` on failure.
/// - Re-advertises and fires `Up` on the next successful probe after a failure.
///
/// **`pause_flag`** — when set to `true` (e.g. by a manifest control watcher),
/// the loop drops all active handles (tombstoning their KV entries) and skips
/// re-advertisement until the flag is cleared.  On clearance the next iteration
/// runs a full probe pass and re-advertises any capabilities whose probes pass.
/// Pass `Arc::new(AtomicBool::new(false))` for normal always-on operation.
///
/// `on_event` is called synchronously within the probe loop — keep it cheap
/// (e.g., push to a channel or append to a `Mutex<Vec>`). It must be `Send`
/// since the loop runs inside a `tokio::spawn`.
///
/// This function never returns. Call it inside `tokio::spawn`:
///
/// ```no_run
/// # use std::sync::{Arc, atomic::AtomicBool};
/// # use mycelium::{GossipAgent, GossipConfig, NodeId, NodeCapabilityConfig, ProbeState, run_capability_probes};
/// # let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", 7000).unwrap(), GossipConfig::default()));
/// # let config = NodeCapabilityConfig::default();
/// # let pause_flag = Arc::new(AtomicBool::new(false));
/// tokio::spawn(run_capability_probes(agent, config, pause_flag, |e| {
///     println!("{}/{} is {}", e.ns, e.name,
///              if matches!(e.state, ProbeState::Up) { "up" } else { "down" });
/// }));
/// ```
#[cfg(feature = "gateway")]
pub async fn run_capability_probes<F>(
    agent:      Arc<GossipAgent>,
    config:     NodeCapabilityConfig,
    pause_flag: Arc<AtomicBool>,
    on_event:   F,
) where
    F: Fn(ProbeEvent) + Send + 'static,
{
    // Probe URLs are gated one by one; redirects are re-checked against the same policy
    // (realignment repairs R3).
    let client = crate::agent::egress_client::build_or_none(
        crate::agent::egress_client::with_policy(agent.egress_policy()),
    );
    let n = config.capabilities.len();
    let mut handles: Vec<Option<CapabilityReg>> = (0..n).map(|_| None).collect();

    // ── Initial probe pass ────────────────────────────────────────────────────
    if !pause_flag.load(Ordering::Relaxed) {
        for (i, entry) in config.capabilities.iter().enumerate() {
            if entry.passes_probe(&client, agent.egress_policy()).await {
                handles[i] = Some(agent.capabilities().advertise_capability(
                    entry.build_capability(),
                    Duration::from_secs(entry.ttl_secs),
                ));
                tracing::info!(ns = %entry.ns, name = %entry.name, "capability up");
                on_event(ProbeEvent {
                    ns: entry.ns.clone(), name: entry.name.clone(), state: ProbeState::Up,
                });
            } else {
                tracing::warn!(
                    ns = %entry.ns, name = %entry.name,
                    probe_url = ?entry.probe_url,
                    "capability probe failed — will retry every {HEALTH_INTERVAL_SECS}s",
                );
                on_event(ProbeEvent {
                    ns: entry.ns.clone(), name: entry.name.clone(), state: ProbeState::Down,
                });
            }
        }
    }

    // ── Health loop ───────────────────────────────────────────────────────────
    loop {
        tokio::time::sleep(Duration::from_secs(HEALTH_INTERVAL_SECS)).await;

        if pause_flag.load(Ordering::Relaxed) {
            // Paused: tombstone all live handles.
            for (i, entry) in config.capabilities.iter().enumerate() {
                if handles[i].is_some() {
                    handles[i] = None;
                    tracing::info!(ns = %entry.ns, name = %entry.name,
                                   "capability paused — tombstoned");
                    on_event(ProbeEvent {
                        ns: entry.ns.clone(), name: entry.name.clone(), state: ProbeState::Down,
                    });
                }
            }
            continue;
        }

        for (i, entry) in config.capabilities.iter().enumerate() {
            let was_up = handles[i].is_some();
            let is_up  = entry.passes_probe(&client, agent.egress_policy()).await;

            match (was_up, is_up) {
                (true, false) => {
                    handles[i] = None;
                    tracing::warn!(ns = %entry.ns, name = %entry.name,
                                   "capability probe failed — tombstoned");
                    on_event(ProbeEvent {
                        ns: entry.ns.clone(), name: entry.name.clone(), state: ProbeState::Down,
                    });
                }
                (false, true) => {
                    handles[i] = Some(agent.capabilities().advertise_capability(
                        entry.build_capability(),
                        Duration::from_secs(entry.ttl_secs),
                    ));
                    tracing::info!(ns = %entry.ns, name = %entry.name,
                                   "capability recovered — re-advertised");
                    on_event(ProbeEvent {
                        ns: entry.ns.clone(), name: entry.name.clone(), state: ProbeState::Up,
                    });
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod unit_file_tests {
    use super::*;
    use crate::Capability;

    /// The three shipped `[[capability]]`-only files load exactly as before: the same entries,
    /// and every new section empty. This is the pin that says the format grew without moving.
    #[test]
    fn existing_capability_files_load_unchanged() {
        for (file, ns, name, ttl) in [
            ("examples/node_n0.toml", "data", "realtime", 60),
            ("examples/node_n1.toml", "", "", 0),
            ("examples/node_n2.toml", "", "", 0),
        ] {
            let cfg = NodeCapabilityConfig::load_from_file(file).unwrap_or_else(|e| panic!("{file}: {e}"));
            assert!(!cfg.capabilities.is_empty(), "{file} declares capabilities");
            if !ns.is_empty() {
                let e = &cfg.capabilities[0];
                assert_eq!((e.ns.as_str(), e.name.as_str(), e.ttl_secs), (ns, name, ttl));
            }
            assert!(cfg.principal.is_none() && cfg.requirements.is_empty() && cfg.groups.is_empty()
                && cfg.lanes.is_empty() && cfg.mandates.is_empty() && cfg.rules.is_empty()
                && cfg.hosts.is_none() && cfg.presence.is_empty(), "{file}: no new section appears from nowhere");
        }
    }

    const PLAN_EXAMPLE: &str = r#"
principal = "planner"

[[capability]]
ns = "plan"
name = "route"
ttl_secs = 30
  [capability.attrs]
  region = "north"
  engine = { version = "2.1.0" }

[[requirement]]
ns = "llm"
name = "inference"
schema_id = "llm.inference.v2"
  [requirement.attrs]
  model = "llama3.2"
  context = { gte = 8192 }
  [requirement.ranking]
  attribute = "context"
  order = "descending"

[[group]]
name = "routers"
  [group.filter]
  ns = "plan"
  name = "route"
  [[group.provides]]
  ns = "plan"
  name = "routing"
  [[group.requires]]
  ns = "data"
  name = "realtime"

[[lane]]
name = "stage-b"
role = "consumes"

[[mandate]]
holder = "planner"
scope = "routers"
operations = ["plan.route", "plan.reroute"]

[[rule]]
actor = "planner"
operation = "plan.route"
resource = "*"
requires_mandate = "routers"
  [rule.requires_values]
  region = "north"

[hosts]
kinds = ["wasm-component", "blob"]
install_budget_bytes = 8589934592
headroom = 0.8
trusted_publishers = ["ed25519:3f"]
placement_root = "/var/lib/mycelium/artifacts"

[[presence]]
ns = "route"
name = "optimize"
min_providers = 2
max_providers = 4
"#;

    /// The plan's §3 example loads, and its requirement — through `CapFilter::matches`, the
    /// mesh's own rule — accepts the capability it names and refuses the ones it does not.
    #[test]
    fn the_plan_example_loads_and_its_filters_match_with_the_mesh_rule() {
        let cfg = NodeCapabilityConfig::from_toml_str(PLAN_EXAMPLE).expect("loads");
        assert_eq!(cfg.principal.as_deref(), Some("planner"));

        // A Version attribute in a capability's attrs is a Version, not Text.
        let cap = cfg.capabilities[0].build_capability_public();
        assert_eq!(cap.attributes.get("engine"), Some(&CapValue::Version([2, 1, 0])));
        assert_eq!(cap.attributes.get("region"), Some(&CapValue::Text("north".into())));

        let req = cfg.requirements[0].to_filter().unwrap();
        let offered = Capability::new("llm", "inference")
            .with("model", CapValue::Text("llama3.2".into()))
            .with("context", CapValue::Integer(8192))
            .with_schema_id("llm.inference.v2");
        assert!(req.matches(&offered), "the plan's requirement binds the capability it names");
        assert!(!req.matches(&offered.clone().with("context", CapValue::Integer(4096))), "gte refuses a smaller context");
        assert!(!req.matches(&Capability::new("llm", "inference")
            .with("model", CapValue::Text("llama3.2".into()))
            .with("context", CapValue::Integer(8192))
            .with_schema_id("llm.inference.v1")), "the schema id is exact");
        assert_eq!(req.ranking.as_ref().map(|r| (r.attribute.as_ref(), r.order)), Some(("context", RankingOrder::Descending)));

        let def = cfg.groups[0].to_def().unwrap();
        assert_eq!(def.provides[0].name.as_ref(), "routing");
        assert_eq!(def.requires[0].namespace.as_ref(), "data");
        assert!(def.filter.matches(&Capability::new("plan", "route")));

        assert_eq!((cfg.lanes[0].name.as_str(), cfg.lanes[0].role), ("stage-b", LaneRole::Consumes));
        assert!(cfg.mandates[0].permits("plan.reroute") && !cfg.mandates[0].permits("plan.delete"));
        let hosts = cfg.hosts.as_ref().unwrap();
        assert!(hosts.can_host("blob", 1 << 30) && !hosts.can_host("blob", 1 << 34) && !hosts.can_host("native", 1));
        assert_eq!((cfg.presence[0].min_providers, cfg.presence[0].max_providers), (2, Some(4)));
        assert!(cfg.presence[0].filter.to_filter().unwrap().matches(&Capability::new("route", "optimize")));
        #[cfg(all(feature = "gateway", feature = "tls"))]
        {
            let rule = cfg.rules[0].to_rule().unwrap();
            assert_eq!(rule.requires_mandate.as_deref(), Some("routers"));
            assert_eq!(rule.requires_values[0], ("region".to_string(), serde_json::json!("north")));
        }
    }

    /// The unit-file example in this module's doc is the one most readers copy, so it must be a
    /// file the loader accepts: parse the first fenced block after "## The unit file" and
    /// validate it.
    #[test]
    fn the_module_doc_unit_file_example_loads() {
        let src = include_str!("capability_config.rs");
        let doc: Vec<&str> = src
            .lines()
            .take_while(|l| l.starts_with("//!"))
            .map(|l| l.strip_prefix("//! ").or_else(|| l.strip_prefix("//!")).unwrap_or(l))
            .collect();
        let start = doc.iter().position(|l| l.starts_with("## The unit file")).expect("unit-file section");
        let open = start + doc[start..].iter().position(|l| l.trim() == "```toml").expect("a toml fence");
        let close = open + 1 + doc[open + 1..].iter().position(|l| l.trim() == "```").expect("a closing fence");
        let block = doc[open + 1..close].join("\n");
        let cfg = NodeCapabilityConfig::from_toml_str(&block)
            .unwrap_or_else(|e| panic!("the module doc's unit-file example must load: {e}\n{block}"));
        cfg.validate().expect("and validate");
        assert!(cfg.principal.is_some() && !cfg.groups.is_empty() && !cfg.rules.is_empty());
        assert!(!cfg.activations.is_empty() && !cfg.serves.is_empty() && !cfg.presence.is_empty());
        assert_eq!(cfg.groups[0].provides.len(), 1);
        assert_eq!(cfg.groups[0].requires.len(), 1);
    }

    fn refused(toml: &str) -> String {
        match NodeCapabilityConfig::from_toml_str(toml) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("must be refused:\n{toml}"),
        }
    }

    #[test]
    fn the_loader_refuses_by_name() {
        let e = refused("[[requirement]]\nns = \"a\"\nname = \"b\"\n[requirement.attrs]\nx = { between = 3 }\n");
        assert!(e.contains("unknown constraint operator") && e.contains("between"), "{e}");

        let e = refused("[[capability]]\nns = \"a\"\nname = \"b\"\n[capability.attrs]\nv = { version = \"2.1\" }\n");
        assert!(e.contains("major.minor.patch") || e.contains("version"), "{e}");

        let e = refused("[[requirement]]\nns = \"a\"\nname = \"b\"\n[requirement.ranking]\nattribute = \"x\"\norder = \"sideways\"\n");
        assert!(e.contains("sideways"), "{e}");

        let e = refused("[[mandate]]\nholder = \"h\"\nscope = \"s\"\n");
        assert!(e.contains("enumerates no operations"), "{e}");

        let e = refused("[[serve]]\nname = \"m\"\nendpoint = \"\"\nmodel = \"x\"\n");
        assert!(e.contains("an endpoint and a model"), "{e}");
        let e = refused("[[serve]]\nname = \"m\"\nendpoint = \"http://o/v1\"\nmodel = \"x\"\n[serve.while_live]\nns = \"llm\"\nname = \"m\"\n");
        assert!(e.contains("share a capability key"), "{e}");
        // Zero-gaps Z2: one key form, and a variable name that is a name.
        let e = refused("[[serve]]\nname = \"m\"\nendpoint = \"http://o/v1\"\nmodel = \"x\"\napi_key = \"sk\"\napi_key_env = \"KEY\"\n");
        assert!(e.contains("api_key") && e.contains("api_key_env"), "{e}");
        let e = refused("[[serve]]\nname = \"m\"\nendpoint = \"http://o/v1\"\nmodel = \"x\"\napi_key_env = \"\"\n");
        assert!(e.contains("api_key_env") && e.contains("empty"), "{e}");

        let e = refused("[hosts]\nkinds = [\"blob\"]\n[[activation]]\nns = \"a\"\nname = \"b\"\ncommand = []\n");
        assert!(e.contains("activation.command"), "{e}");
        let e = refused("[hosts]\nkinds = [\"blob\"]\n[[activation]]\nns = \"a\"\nname = \"b\"\ncommand = [\"run\", \"{weights}\"]\n");
        assert!(e.contains("unknown placeholder {weights}"), "{e}");
        let e = refused("[hosts]\nkinds = [\"blob\"]\n[[activation]]\nns = \"a\"\nname = \"b\"\ncommand = [\"run\", \"{rendered}\"]\n");
        assert!(e.contains("resolve_artifact_refs"), "{e}");
        let e = refused("[hosts]\nkinds = [\"wasm-component\"]\n[[activation]]\nns = \"a\"\nname = \"b\"\ncommand = [\"run\"]\n");
        assert!(e.contains("does not name the blob kind"), "{e}");
        let e = refused("[hosts]\nkinds = [\"blob\"]\n[[activation]]\nns = \"a\"\nname = \"b\"\ncommand = [\"run\"]\ntimeout_secs = 0\n");
        assert!(e.contains("timeout of 0"), "{e}");

        let e = refused("[[presence]]\nns = \"a\"\nname = \"b\"\nmin_providers = 0\n");
        assert!(e.contains("floor of zero"), "{e}");

        let e = refused("[[presence]]\nns = \"a\"\nname = \"b\"\nmin_providers = 3\nmax_providers = 2\n");
        assert!(e.contains("below floor"), "{e}");

        let e = refused("[hosts]\nkinds = [\"native\"]\n");
        assert!(e.contains("unknown artifact kind") && e.contains("native"), "{e}");

        let e = refused("[hosts]\nheadroom = 1.5\n");
        assert!(e.contains("outside (0, 1]"), "{e}");

        let e = refused("[hosts]\nfuel_per_call = 0\n");
        assert!(e.contains("hosts.fuel_per_call") && e.contains("budget of 0"), "{e}");

        let e = refused("[hosts]\ncall_deadline_ms = 0\n");
        assert!(e.contains("hosts.call_deadline_ms") && e.contains("deadline of 0"), "{e}");

        let e = refused("[hosts]\noperator_publishers = [\"ed25519:aa\"]\n");
        assert!(e.contains("hosts.operator_publishers") && e.contains("trusted_publishers is empty"), "{e}");

        let e = refused("[hosts]\ntrusted_publishers = [\"ed25519:bb\"]\noperator_publishers = [\"ed25519:aa\"]\n");
        assert!(e.contains("hosts.operator_publishers") && e.contains("not in hosts.trusted_publishers"), "{e}");

        let e = refused("[[requirement]]\nns = \"a\"\nname = \"b\"\n[requirement.attrs]\nx = { gte = 1, lte = 9 }\n");
        assert!(e.contains("exactly one operator"), "{e}");
    }

    /// A `[hosts]` table with no `trusted_publishers` installs anything any peer publishes
    /// (`Provisioner::provenance_ok` is true for an empty list), so it is refused by name unless
    /// the unit says `accept_unsigned = true` on purpose. Seen failing first: the table validated.
    #[test]
    fn a_hosts_table_without_trusted_publishers_is_refused_unless_it_opts_out() {
        let e = refused("[hosts]\nkinds = [\"wasm-component\"]\n");
        assert!(e.contains("hosts.trusted_publishers") && e.contains("accept_unsigned"), "{e}");
        // The opt-out, said on purpose, validates.
        NodeCapabilityConfig::from_toml_str("[hosts]\nkinds = [\"wasm-component\"]\naccept_unsigned = true\n")
            .expect("the opt-out validates");
        // Keys listed: validates, as before.
        NodeCapabilityConfig::from_toml_str("[hosts]\nkinds = [\"wasm-component\"]\ntrusted_publishers = [\"ed25519:aa\"]\n")
            .expect("a trusted list validates");
        // Both at once name two policies.
        let e = refused("[hosts]\nkinds = [\"wasm-component\"]\naccept_unsigned = true\ntrusted_publishers = [\"ed25519:aa\"]\n");
        assert!(e.contains("hosts.accept_unsigned"), "{e}");
    }
}
