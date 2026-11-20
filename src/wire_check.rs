//! The offline wire-check — `mycelium wire-check` (`docs/plans/design-time-tooling.md` §4, W2;
//! A2's provisioning class; L1's and R1's hosting checks).
//!
//! A **pure function over declarations**: given a set of unit files (`NodeCapabilityConfig`) and,
//! optionally, a set of artifact descriptions, it applies the mesh's own match rule —
//! [`CapFilter::matches`] — and reports what *could not bind*. It reads no runtime state, starts
//! nothing, and knows no addresses. Its answer is **would bind under these declarations**, never
//! *is bound*: liveness, ranking over runtime attributes, locality, membership *now*, whether a
//! mandate is current, and load are runtime facts the mesh reports itself.
//!
//! What it reports, in order of severity (each a [`Finding`] with a `kind` a script can key on):
//!
//! | kind | meaning | severity |
//! |---|---|---|
//! | `unwired requirement` | no declared capability, in any unit, satisfies the filter | error |
//! | `schema-only mismatch` | it matches except for `schema_id` — the rollout-window case, named with both ids | error |
//! | `type-cross constraint` | a constraint whose value type can never compare with the offered attribute's type | error |
//! | `empty group` | no declared capability satisfies the group's filter, so nothing could join | error |
//! | `unknown schema` | a unit or an artifact names a `schema_id` the schema directory does not define (only with a schema directory — `--schemas`) | error |
//! | `group requires unmet` | a group's `requires` filter binds to nothing | error |
//! | `orphan lane` | a lane consumed and never produced, or produced and never consumed | error |
//! | `unhostable entry` | an artifact would satisfy the filter but no unit's `[hosts]` names its kind with budget for its footprint | error |
//! | `presence unhostable` | a `[[presence]]` floor cannot be met by deployed providers plus distinct hosting units | error |
//! | `presence bands overlap` | two `[[presence]]` bands, at least one capped, over the same capability and not provably disjoint, can trade a provider back and forth — a capped band sheds against its ceiling while the other's floor or ceiling brings a provider back | warning |
//! | `schedule without holders` | no unit that runs a `[[schedule]]` — declares it, with no `runtime` and no `[hosts]` — declares a capability its `holders` filter matches: no host can hold it, so it never runs | error |
//! | `schedule declared two ways` | two units declare one schedule id with a different period, phase, missed-run policy or holders; one id is one schedule fleet-wide, and its candidates refuse to hold while they disagree | error |
//! | `schedule in a stem unit` | a unit with `[hosts]` or `[[serve]]` — a stem's — declares a `[[schedule]]`, which a stem refuses to start with | error |
//! | `runtime with a schedule` | a unit an SDK agent loads (`runtime`) declares a `[[schedule]]`, which `POST /gateway/units/declare` refuses (422): an SDK agent runs one on `GET /gateway/schedules/run` (Y12), not in its unit | error |
//! | `unauthorisable edge` | the requirer declares authority vocabulary, and no declared rule could admit the call as a skill or a tool — or the rule that would requires a mandate scope no declared mandate enumerates for it. For an edge to a hosted component: no rule **declared by a unit that could host it** permits a calling node on the shape its provider decides — the finding names each rule form that does not, and a rule declared elsewhere | error |
//! | `would bind by provisioning` | no deployed provider, but a hostable artifact matches | warning (error with `strict_deployed`) |
//! | `would bind after acceptance` | no deployed provider, and the hostable artifact that matches is **proposed** (D20) — it loads only into a shadow lane until a reviewer accepts it | warning (error with `strict_deployed`) |
//! | `ungoverned edge` | the requirer declares no mandate and no rule while other units do | warning |
//! | `node-decided edge` | an edge to a hosted component is admitted only by a rule, or a required mandate, that names the requirer's **unit** principal — and its provider decides a member's direct call as the calling **node** (`node:{sender}`): a provider as built today refuses it ([`NODE_DECIDED`]) | warning |
//! | `host-dependent edge` | of the units that could host a component, one's own rules admit the call and another's do not ([`HOST_DEPENDENT`]) | warning |
//! | `mandate-dependent edge` | the rule that would admit a hosted edge requires a mandate scope a node-held declared mandate covers; a provider built from `[authority]` entitles no authority to a mandate scope and refuses ([`MANDATE_DEPENDENT`]) | warning |
//! | `unissuable grant` | a rule admits a call only under a grant, and no declared rule admits anyone to `grant.issue` on its resource | warning |
//! | `unranked ranking` | a ranking on an attribute no matching capability carries | warning |
//! | `single provider` | a requirement with exactly one possible provider | warning |
//!
//! The JSON output is a versioned document, [`DECLARATION_SCHEMA`], with the `revision` the caller
//! supplies (the git commit of the units directory) so a consumer can join it to runtime records
//! by `(principal, ns, name, schema_id)` — the plan's §9. A change to its shape is a schema version;
//! the pin is `docs/reference/declaration.schema.json` (W6), validated against the golden and a
//! fresh report by `tests/declaration_schema.rs`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::capability::{CapConstraint, CapFilter, CapValue, Capability};
use crate::capability_config::{CapDecl, LaneRole, NodeCapabilityConfig};
use crate::error::GossipError;

/// The JSON document's schema id; a consumer pins it. A field **added** keeps the id — the schema declares it
/// optional, and a consumer holding the pinned schema validates a newer document because it never required the
/// field (X8's `runtime`, `adapter`, `carries`, `not_carried`, 2026-10-24); a field removed or renamed, or a
/// field made required, bumps it.
pub const DECLARATION_SCHEMA: &str = "mycelium.design/declaration/1";

/// A unit: a name (the file's stem) and what it declares.
#[derive(Debug, Clone)]
pub struct Unit {
    pub name:   String,
    pub config: NodeCapabilityConfig,
}

/// An artifact **description** — the reviewable input the plan's D10 defines (`artifacts/*.toml`):
/// what the artifact would provide once installed, its kind, and its signed footprint. The
/// checker reads descriptions, not the binary manifest, because the description is the design-time
/// artefact and CI keeps the two equal.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ArtifactDescription {
    /// `wasm-component` | `blob` (the wasm-host crate's kinds, by name).
    pub kind: String,
    /// The capability it would advertise once installed.
    pub provides: CapDecl,
    /// The signed footprint (`disk_bytes`, `mem_bytes`); absent = undeclared = 0.
    #[serde(default)]
    pub requires: Footprint,
    /// Optional: where the bytes are, relative to the description. Not read here.
    #[serde(default)]
    pub bytes: Option<String>,
    #[serde(default)]
    pub est_install_secs: Option<u64>,
    /// D20: published as a proposal — a provisioner loads it only into a shadow lane until a
    /// reviewer accepts it, so the checker reports it as *would bind after acceptance*.
    #[serde(default)]
    pub proposed: bool,
}

/// The footprint an artifact declares (`artifacts.md` §2): disk at the placement root, memory
/// once live. The checker's **hosting footprint** is their sum — the most a host must have room for.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Footprint {
    #[serde(default)]
    pub disk_bytes: u64,
    #[serde(default)]
    pub mem_bytes:  u64,
}

impl ArtifactDescription {
    /// Parse a description's text.
    pub fn from_toml_str(s: &str) -> Result<Self, GossipError> {
        let d: Self = toml::from_str(s).map_err(GossipError::Toml)?;
        if !crate::capability_config::HOSTABLE_KINDS.contains(&d.kind.as_str()) {
            return Err(GossipError::InvalidField {
                field:  "artifact.kind",
                reason: format!("unknown artifact kind {:?}", d.kind),
            });
        }
        Ok(d)
    }

    /// The bytes a host must budget for.
    pub fn footprint_bytes(&self) -> u64 {
        self.requires.disk_bytes.saturating_add(self.requires.mem_bytes)
    }
}

/// Options for one check.
#[derive(Debug, Clone)]
pub struct CheckOptions {
    /// Treat *would bind by provisioning* as an error: the deployment must be wired by what is
    /// deployed, not by what could be installed.
    pub strict_deployed: bool,
    /// Run the **authority overlay** (W4): for every wired edge whose requirer has declared any
    /// authority vocabulary (a `[[mandate]]` or a `[[rule]]`), whether some declared rule could ever
    /// admit it — actor, operation and resource matching, and, where the rule requires a mandate
    /// scope, a declared mandate held by the requirer's principal that enumerates the operation. An
    /// edge to a hosted component is read as its provider decides it: on [`hosted_call_shape`], for
    /// the calling node. It does not evaluate policy; it finds edges that no declaration could ever
    /// authorise — *could admit*, never *is admitted*. On by default; `--no-authority` turns it off.
    pub authority: bool,
    /// The revision to stamp on the document (the git commit of the units directory), if known.
    pub revision: Option<String>,
    /// The schema ids the deployment's schema directory defines (guide 12's `schemas/` — each
    /// `.json` file's path relative to the directory, without the extension). When given, every
    /// `schema_id` a unit or an artifact declares must be one of them, or the finding is
    /// `unknown schema`. `None` skips the check.
    pub known_schemas: Option<BTreeSet<String>>,
}

impl Default for CheckOptions {
    fn default() -> Self {
        Self { strict_deployed: false, authority: true, revision: None, known_schemas: None }
    }
}

/// The operation a grant must enumerate for a call on a resource: `{operation}:{resource_key}`,
/// with the resource key being everything before a provider's `@`. This is the gateway's own rule
/// (`mycelium::mandate_operation`, gated behind the attesting build); the checker restates the
/// string so it can run without a gateway, and a test under that build pins the two equal.
pub fn mandate_operation_form(operation: &str, resource: &str) -> String {
    format!("{operation}:{}", resource.split('@').next().unwrap_or(resource))
}

/// The two call shapes a capability edge could take at a provider, as the provider enforcement
/// point derives them: a skill (`skill.invoke` on `skill:{ns}/{name}`) or an MCP tool
/// (`tools/call` on `tool:{name}`). The overlay admits an edge if either is authorised.
/// The two call shapes a requirement `ns/name` can take at a gateway — the operation and resource
/// a grant must enumerate and a rule must admit: `skill.invoke` on `skill:{ns}/{name}` (an A2A
/// skill) and `tools/call` on `tool:{name}` (an MCP tool). The authority overlay (W4) reasons over
/// these; the JSON carries them on every edge (W6) so a consumer joining runtime records never
/// re-derives them — and sees that the tool shape drops `ns`, which is why a tool-shaped
/// observation can match more than one edge. A hosted component has a third, [`hosted_call_shape`].
pub fn call_shapes(ns: &str, name: &str) -> [(String, String); 2] {
    [
        ("skill.invoke".to_string(), format!("skill:{ns}/{name}")),
        ("tools/call".to_string(), format!("tool:{name}")),
    ]
}

/// **The hosted shape** — the third call shape, for a capability a host serves from an installed component: the
/// operation and resource an enforcing provider decides a member's direct call of it on, `cap.invoke` on
/// `cap:{ns}/{name}` (`provider_enforcement::derive_call`; at the provider the resource carries `@{node}`, and
/// `[authority]` qualifies a bare rule to its node). `None` in the `tool` namespace: a `tool/{name}` component is
/// decided as the tool it is, the `tools/call` shape [`call_shapes`] already gives.
///
/// Separate from [`call_shapes`], whose two-shape return is a contract: an edge carries this one only where the
/// declaration shows the provider is a hosted component (a `wasm-component` artifact description).
pub fn hosted_call_shape(ns: &str, name: &str) -> Option<(String, String)> {
    (ns != "tool").then(|| {
        let (operation, resource) = crate::hosted_capability_call(ns, name);
        (operation.to_string(), resource)
    })
}

/// **The hosted-edge warnings, each kind with its severity, in one place** — the overlay pushes a finding of these
/// kinds only through these constants, so a change of severity is one line here (and the docs that table them).
///
/// `node-decided edge`: only a rule, or a required mandate, naming the requirer's **unit** principal would admit the
/// edge, and the provider decides a member's direct call for the calling **node**. A provider as built today refuses
/// that call; the kind is a warning by the maintainer's choice of 2026-11-18, with warning-versus-error open.
pub const NODE_DECIDED: (&str, Severity) = ("node-decided edge", Severity::Warning);
/// `host-dependent edge`: of the units that could host the component, one's own rules admit the call and another's do
/// not, and which of them installs it is decided at run time.
pub const HOST_DEPENDENT: (&str, Severity) = ("host-dependent edge", Severity::Warning);
/// `mandate-dependent edge`: the rule that would admit the call requires a mandate scope. A node holds a declared
/// mandate in it, but a provider built from `[authority]` entitles no authority to a mandate scope, so it refuses;
/// only an execution authority attached in code, entitled for that scope, can establish what the node presents.
pub const MANDATE_DEPENDENT: (&str, Severity) = ("mandate-dependent edge", Severity::Warning);

/// Is `provider` a component a host would serve — a `wasm-component` artifact description (the wasm host's serve
/// loop answers its `cap.invoke/{ns}/{name}` kind)? A `blob` is bytes a node-local runtime consumes, and a unit's
/// own `[[capability]]` or `[[serve]]` says nothing of how it is served, so neither is known to be one.
fn is_hosted_component(artifacts: &[(String, ArtifactDescription)], provider: &Provider) -> bool {
    matches!(provider, Provider::Artifact { artifact, .. } if artifacts.iter().any(|(name, a)| name == artifact && a.kind == "wasm-component"))
}

/// What one unit's own rules decide for **a member's direct call** of a component it would host — the reading a
/// provider built from `[authority]` makes, which loads that unit's rules and no other's.
enum HostReading {
    /// Some rule permits a calling node, and no earlier rule that matches the call stands in its way.
    Admits,
    /// The rule that would permit requires a mandate scope a declared mandate, held by a node, covers.
    MandateDependent(String),
    /// Only a rule, or a required mandate, naming the requirer's unit principal would permit.
    NodeDecided(String),
    /// Nothing permits; why each rule written for the call does not.
    Refuses(Vec<String>),
}

/// Is `s` a node id in the spelling a node's principal and a resource's `@{node}` carry (`NodeId`'s own: the socket
/// address as it prints)? The evaluator matches an actor and a resource exactly, so `*`, an empty string, a host name
/// or another spelling of the same address names no node.
fn names_a_node(s: &str) -> bool {
    s.parse::<crate::NodeId>().is_ok_and(|n| n.as_str() == s)
}

/// [`HostReading`] of `rules` — one unit's, in the order the file declares them, which is the order the reference
/// evaluator tries them in — for a member's direct call decided as `operation` on `resource@{node}` for the actor
/// `node:{sender}`, carrying no scopes and no signed decision. Each rule field is read as that provider reads it:
///
/// - `actor`, `operation`, `resource`: matched exactly, or `*`. The checker knows no address, so an actor `node:<id>`
///   and a resource `…@<id>` are each taken as possibly the caller and the provider — if `<id>` is a node id at all.
///   A bare resource is one `[authority]` qualifies to its node.
/// - `requires_scopes`, `signed_with`: the rule applies only to a caller holding the scopes, or to a signed grant
///   decision. A member's direct call is neither, so the rule does not apply.
/// - `cancellable`: the provider refuses — a hosted component is reached through a door that cannot stop a begun call.
/// - `requires_facts`: the reference evaluator cannot establish a fact, and decides *indeterminate*.
/// - `requires_mandate`: without a mandate the rule decides *indeterminate*; see [`MANDATE_DEPENDENT`].
/// - `requires_grant`, `requires_values`: decided on a grant issued, or on the call's arguments, at run time. Not
///   read: the rule is taken to permit.
///
/// **The first allowance that matches decides** (`ReferenceEvaluator::evaluate_with_history`), so a rule that
/// matches the call and does not permit it stands in the way of every later rule it covers — unless it names a value
/// (`requires_values`), which makes its match a run-time fact. The converse is a stated bound, not read here: a rule
/// taken to permit — one under a grant, or naming a value — that at run time matches and does not permit (no grant
/// presented, the value absent) shadows a later plain rule at the provider, and the check has passed the edge on it.
fn host_reading(rules: &[crate::capability_config::RuleDecl], mandates: &[&crate::capability_config::MandateDecl], principal: Option<&str>, operation: &str, resource: &str) -> HostReading {
    let enumerated = mandate_operation_form(operation, resource);
    let mut reasons: Vec<String> = Vec::new();
    let mut as_unit: Option<String> = None;
    let mut mandate_dependent: Option<String> = None;
    // The rules that match a member's call and do not permit it, in order.
    let mut blockers: Vec<&crate::capability_config::RuleDecl> = Vec::new();
    for r in rules.iter().filter(|r| !r.is_exposure()) {
        if !(r.operation == "*" || r.operation == operation) {
            continue;
        }
        let what = format!("{:?} {} on {}", r.actor, r.operation, r.resource);
        if !(r.resource == "*" || r.resource == resource) {
            match r.resource.strip_prefix(resource).and_then(|rest| rest.strip_prefix('@')) {
                Some(node) if names_a_node(node) => {}
                Some(_) => {
                    reasons.push(format!("the resource {:?} names no node after `@` (a node is `<ip>:<port>`, and a resource is matched exactly)", r.resource));
                    continue;
                }
                None => continue,
            }
        }
        // The requirer's unit principal, where it is not itself a node's: no caller the provider can see.
        let unit_actor = principal.is_some_and(|p| r.actor == p && !p.strip_prefix("node:").is_some_and(names_a_node));
        if !unit_actor && r.actor != "*" {
            match r.actor.strip_prefix("node:") {
                Some(node) if names_a_node(node) => {}
                Some(_) => {
                    reasons.push(format!("the actor {:?} names no node: an actor is matched exactly, so a node is `node:<ip>:<port>` and nothing after `node:` is a wildcard", r.actor));
                    continue;
                }
                None => continue,
            }
        }
        if !r.requires_scopes.is_empty() {
            reasons.push(format!("the rule for {what} requires scopes, and a member's direct call carries no scopes, so the rule does not apply to it"));
            continue;
        }
        if !r.signed_with.is_empty() {
            reasons.push(format!("the rule for {what} sets `signed_with`, which applies only to a signed grant decision, so it does not apply to a call"));
            continue;
        }
        // The rule matches the call (or would, if the provider read the unit). What it then decides.
        let (mut by_unit_mandate, mut by_node_mandate) = (false, false);
        let permits = if r.cancellable {
            reasons.push(format!("the rule for {what} is `cancellable`, and the door a hosted component is reached through cannot stop a begun call, so the provider refuses it (`door_cannot_stop`)"));
            false
        } else if !r.requires_facts.is_empty() {
            reasons.push(format!("the rule for {what} requires facts ({}) the reference evaluator `[authority]` builds cannot establish", r.requires_facts.join(", ")));
            false
        } else if let Some(scope) = &r.requires_mandate {
            // A trailing `*` names a scope family, as the evaluator reads it.
            let covers = |m: &&&crate::capability_config::MandateDecl| scope.strip_suffix('*').map_or(&m.scope == scope, |family| m.scope.starts_with(family)) && m.permits(&enumerated);
            let held: Vec<&str> = mandates.iter().filter(covers).map(|m| m.holder.as_str()).collect();
            if let Some(node) = held.iter().find(|h| h.strip_prefix("node:").is_some_and(names_a_node)) {
                by_node_mandate = true;
                if !unit_actor && mandate_dependent.is_none() {
                    mandate_dependent = Some(format!("the rule for {what} requires mandate scope {scope:?}, which the declared mandate held by {node:?} covers"));
                }
            } else if principal.is_some_and(|p| held.contains(&p)) {
                by_unit_mandate = true;
            } else {
                reasons.push(format!("the rule for {what} requires mandate scope {scope:?}, and no declared mandate in that scope enumerates {enumerated:?}"));
            }
            false
        } else {
            true
        };
        if unit_actor {
            // Never matched by a node's call, so it stands in no other rule's way; noted if it would have permitted.
            if (permits || by_unit_mandate || by_node_mandate) && as_unit.is_none() {
                as_unit = Some(format!("the rule for {} on {} names {:?} as its actor", r.operation, r.resource, r.actor));
            }
            continue;
        }
        if blockers.iter().any(|b| (b.actor == "*" || b.actor == r.actor) && (b.resource == "*" || b.resource == resource || b.resource == r.resource)) {
            reasons.push(format!("the rule for {what} is never reached: the evaluator decides by the first allowance that matches, and an earlier rule in the same unit matches the call and does not permit it"));
            continue;
        }
        if permits {
            return HostReading::Admits;
        }
        if by_unit_mandate && as_unit.is_none() {
            as_unit = Some(format!(
                "the rule for {} on {} requires mandate scope {:?}, and the declared mandate that enumerates {enumerated:?} is held by {:?}",
                r.operation,
                r.resource,
                r.requires_mandate.as_deref().unwrap_or_default(),
                principal.unwrap_or_default()
            ));
        }
        // A rule that names a value is skipped by the evaluator when the call carries that value with another one
        // (`Rule::matches`), so whether it is the first match is a run-time fact: it stands in no later rule's way.
        if r.requires_values.is_empty() {
            blockers.push(r);
        }
    }
    match (mandate_dependent, as_unit) {
        (Some(why), _) => HostReading::MandateDependent(why),
        (None, Some(why)) => HostReading::NodeDecided(why),
        (None, None) => HostReading::Refuses(reasons),
    }
}

/// The units that do not host the component and declare a rule that would admit a member's call of it if they did —
/// a rule no provider of the component loads.
fn admitting_elsewhere(e: &Edge, requirer: &Unit, hosts: &BTreeSet<&str>, units: &[Unit], mandates: &[&crate::capability_config::MandateDecl]) -> Vec<String> {
    let (operation, resource) = crate::hosted_capability_call(&e.ns, &e.name);
    let principal = requirer.config.principal.as_deref();
    units
        .iter()
        .filter(|u| !hosts.contains(u.name.as_str()) && !matches!(host_reading(&u.config.rules, mandates, principal, operation, &resource), HostReading::Refuses(_)))
        .map(|u| format!("`{}`", u.name))
        .collect()
}

/// The authority overlay's reading of **an edge to a hosted component**: what the provider that would host it
/// decides for a member's direct call, from the rules of each unit that could host it ([`host_reading`]).
fn hosted_edge(e: &Edge, requirer: &Unit, hosts: &BTreeSet<&str>, units: &[Unit], mandates: &[&crate::capability_config::MandateDecl], findings: &mut Vec<Finding>) {
    let (operation, resource) = crate::hosted_capability_call(&e.ns, &e.name);
    let principal = requirer.config.principal.as_deref();
    let read = |u: &Unit| host_reading(&u.config.rules, mandates, principal, operation, &resource);
    let readings: Vec<(&str, HostReading)> = hosts.iter().filter_map(|h| units.iter().find(|u| u.name == *h).map(|u| (*h, read(u)))).collect();
    let admitting: Vec<&str> = readings.iter().filter(|(_, r)| matches!(r, HostReading::Admits)).map(|(h, _)| *h).collect();
    if admitting.len() == readings.len() {
        return;
    }
    let at = format!("requirement {}/{}", e.ns, e.name);
    let mut warn = |(kind, severity): (&str, Severity), message: String| findings.push(Finding { severity, kind: kind.into(), unit: Some(e.requirer.clone()), message });
    if !admitting.is_empty() {
        let (refusing, why): (Vec<&str>, Vec<String>) = readings
            .iter()
            .filter(|(_, r)| !matches!(r, HostReading::Admits))
            .map(|(h, r)| {
                let why = match r {
                    HostReading::Refuses(reasons) => reasons.first().cloned().unwrap_or_else(|| "it declares no rule for it".into()),
                    HostReading::MandateDependent(why) | HostReading::NodeDecided(why) => why.clone(),
                    HostReading::Admits => String::new(),
                };
                (*h, format!("`{h}`: {why}"))
            })
            .unzip();
        return warn(
            HOST_DEPENDENT,
            format!(
                "{at}: the rules of {} admit a calling node to {operation} on {resource}, and the rules of {} do not ({}). A provider decides by its own unit's rules, so the call runs or is refused by which of them installs the component: declare the rule in each unit that could host it",
                admitting.join("|"),
                refusing.join("|"),
                why.join("; ")
            ),
        );
    }
    if let Some((h, why)) = readings.iter().find_map(|(h, r)| if let HostReading::MandateDependent(why) = r { Some((h, why)) } else { None }) {
        return warn(
            MANDATE_DEPENDENT,
            format!(
                "{at}: in `{h}`, {why}. A provider built from `[authority]` entitles no authority to a mandate scope, so it refuses this call; only a provider whose execution authority is attached in code, entitled for that scope, can establish the mandate a calling node presents"
            ),
        );
    }
    if let Some((h, why)) = readings.iter().find_map(|(h, r)| if let HostReading::NodeDecided(why) = r { Some((h, why)) } else { None }) {
        return warn(
            NODE_DECIDED,
            format!(
                "{at}: in `{h}`, {why} — the requirer's unit principal. The provider that hosts it decides a member's direct call as {operation} on {resource}@{{node}} for the actor node:{{sender}}: the calling node, never the unit it runs. No node is {:?}, so a provider built from `[authority]` refuses this call. (A call a gateway frames for a client — `/mcp` for a tool, a companion's route through `rpc_call_as` — is decided for that client's principal, not the node's.) Name the calling node, `node:<ip>:<port>`, or \"*\"",
                principal.unwrap_or_default()
            ),
        );
    }
    let mut reasons: Vec<String> = Vec::new();
    for (_, r) in &readings {
        if let HostReading::Refuses(why) = r {
            for w in why {
                if !reasons.contains(w) {
                    reasons.push(w.clone());
                }
            }
        }
    }
    // A rule that would admit, declared where no provider of this component loads it.
    let elsewhere = admitting_elsewhere(e, requirer, hosts, units, mandates);
    // A skill or tool rule written for the edge, which the hosting provider never consults.
    let unread = call_shapes(&e.ns, &e.name).into_iter().filter(|(o, r)| (o.as_str(), r) != (operation, &resource)).find(|(o, res)| units.iter().flat_map(|u| u.config.rules.iter()).any(|r| !r.is_exposure() && &r.operation == o && r.resource.split('@').next() == Some(res.as_str())));
    findings.push(Finding {
        severity: Severity::Error,
        kind:     "unauthorisable edge".into(),
        unit:     Some(e.requirer.clone()),
        message:  format!(
            "{at}: no rule declared by {} — the unit{} that could host it, whose own rules its provider reads (`[authority]`) — admits a calling node to {operation} on {resource}, what that provider decides a member's direct call as, for the actor node:{{sender}} (a rule's actor is \"*\" or `node:<ip>:<port>`){}{}{}",
            hosts.iter().copied().collect::<Vec<_>>().join("|"),
            if hosts.len() == 1 { "" } else { "s" },
            if reasons.is_empty() { String::new() } else { format!(": {}", reasons.join("; ")) },
            if elsewhere.is_empty() { String::new() } else { format!("; the rule declared in {} would admit it there, and no provider of this component loads a rule from there", elsewhere.join(", ")) },
            unread.map(|(o, r)| format!("; that provider does not consult the rule on {o} on {r}")).unwrap_or_default()
        ),
    });
}

/// How serious a finding is: an `Error` makes the check exit 1; a `Warning` does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// One thing the check found. `kind` is stable text a script can key on; `message` is for people.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub severity: Severity,
    pub kind:     String,
    /// The unit the finding is about, when there is one.
    pub unit:     Option<String>,
    pub message:  String,
}

/// Who would provide a capability: a unit's own capability, a group's asserted capability, or an
/// artifact that a host could install.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "via", rename_all = "lowercase")]
pub enum Provider {
    Unit { unit: String, principal: Option<String> },
    Group { group: String, defined_by: String },
    Artifact {
        artifact: String,
        hosts: Vec<String>,
        /// D20: a proposed artifact binds only after a reviewer accepts it.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        proposed: bool,
    },
}

/// One requirement and everything that would bind to it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub requirer:  String,
    pub principal: Option<String>,
    pub ns:        String,
    pub name:      String,
    pub schema_id: Option<String>,
    pub providers: Vec<Provider>,
    /// The call shapes this edge takes at a gateway ([`call_shapes`]), in the form a grant
    /// enumerates and an evidence record carries — the join to runtime records on the authority
    /// side (W6). Always both, first; a consumer maps each through its own reviewed catalogue. An
    /// edge whose provider is a hosted component outside the `tool` namespace carries a third,
    /// [`hosted_call_shape`] — the one the provider that hosts it decides (2026-11-18, additive).
    #[serde(default)]
    pub operations: Vec<CallShape>,
}

/// One call shape of an [`Edge`]: an operation on a resource, as the gateway names them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallShape {
    pub operation: String,
    pub resource:  String,
}

/// A lane and which units sit on each side.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lane {
    pub name:      String,
    pub producers: Vec<String>,
    pub consumers: Vec<String>,
}

/// A unit as the document lists it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnitSummary {
    pub name:        String,
    pub principal:   Option<String>,
    pub offers:      Vec<String>,
    pub requires:    Vec<String>,
    pub groups:      Vec<String>,
    pub hosts_kinds: Vec<String>,
    pub presence:    Vec<String>,
    /// The unit's `runtime` key (adopter-surface X8), or `null` for a native unit.
    #[serde(default)]
    pub runtime:     Option<String>,
    /// The adapter contract's row this unit's runtime names (`native` without a key), or `null` for an unknown
    /// runtime — which is also a finding.
    #[serde(default)]
    pub adapter:     Option<String>,
    /// The properties that adapter carries, as the contract names them.
    #[serde(default)]
    pub carries:     Vec<String>,
    /// The properties that adapter could carry and does not.
    #[serde(default)]
    pub not_carried: Vec<String>,
}

/// The whole answer: the versioned document the JSON renders, and what text and DOT draw from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub schema:    String,
    pub revision:  Option<String>,
    pub units:     Vec<UnitSummary>,
    pub artifacts: Vec<String>,
    pub edges:     Vec<Edge>,
    pub lanes:     Vec<Lane>,
    pub findings:  Vec<Finding>,
}

impl Report {
    /// `0` with no errors, `1` with any.
    pub fn exit_code(&self) -> i32 {
        if self.findings.iter().any(|f| f.severity == Severity::Error) { 1 } else { 0 }
    }

    /// The document, as JSON.
    pub fn render_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a report serialises")
    }

    /// For people. Every line says *would bind* or *could not bind*.
    pub fn render_text(&self) -> String {
        let mut out = String::new();
        let rev = self.revision.as_deref().unwrap_or("unknown");
        out.push_str(&format!(
            "wire-check — {} units, {} artifacts, revision {rev}\n",
            self.units.len(),
            self.artifacts.len()
        ));
        for e in &self.edges {
            let sid = e.schema_id.as_deref().map(|s| format!(" [{s}]")).unwrap_or_default();
            if e.providers.is_empty() {
                out.push_str(&format!("  {} requires {}/{}{sid}: could not bind\n", e.requirer, e.ns, e.name));
            } else {
                let by: Vec<String> = e
                    .providers
                    .iter()
                    .map(|p| match p {
                        Provider::Unit { unit, .. } => unit.clone(),
                        Provider::Group { group, .. } => format!("group {group}"),
                        Provider::Artifact { artifact, hosts, proposed } => format!(
                            "artifact {artifact}{} on {}",
                            if *proposed { " (proposed)" } else { "" },
                            hosts.join("|")
                        ),
                    })
                    .collect();
                out.push_str(&format!(
                    "  {} requires {}/{}{sid}: would bind via {}\n",
                    e.requirer,
                    e.ns,
                    e.name,
                    by.join(", ")
                ));
            }
        }
        // X8: what each unit's runtime carries, from the adapter contract — the overlay's answer to what a fleet
        // of mixed runtimes enforces where.
        for u in &self.units {
            let runtime = u.runtime.as_deref().unwrap_or("native");
            match &u.adapter {
                Some(adapter) => out.push_str(&format!(
                    "  unit {} (runtime {runtime}, {adapter}): carries {}; not carried: {}\n",
                    u.name,
                    if u.carries.is_empty() { "nothing".to_string() } else { u.carries.join(", ") },
                    if u.not_carried.is_empty() { "nothing".to_string() } else { u.not_carried.join(", ") },
                )),
                None => out.push_str(&format!("  unit {} (runtime {runtime}): not a row of the adapter contract\n", u.name)),
            }
        }
        for l in &self.lanes {
            out.push_str(&format!(
                "  lane {}: {} -> {}\n",
                l.name,
                if l.producers.is_empty() { "nobody".into() } else { l.producers.join("|") },
                if l.consumers.is_empty() { "nobody".into() } else { l.consumers.join("|") }
            ));
        }
        let errors = self.findings.iter().filter(|f| f.severity == Severity::Error).count();
        let warnings = self.findings.len() - errors;
        for f in &self.findings {
            let tag = match f.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            };
            let unit = f.unit.as_deref().map(|u| format!(" ({u})")).unwrap_or_default();
            out.push_str(&format!("{tag}: {}{unit}: {}\n", f.kind, f.message));
        }
        out.push_str(&format!("{errors} error(s), {warnings} warning(s)\n"));
        out
    }

    /// Graphviz: units as boxes, groups as ellipses, artifacts as notes, an edge per binding, a
    /// dashed red edge per requirement that could not bind.
    pub fn render_dot(&self) -> String {
        fn q(s: &str) -> String {
            format!("\"{}\"", s.replace('"', "\\\""))
        }
        let mut out = String::from("digraph wiring {\n  rankdir=LR;\n  node [shape=box];\n");
        for u in &self.units {
            out.push_str(&format!(
                "  {} [label={}];\n",
                q(&u.name),
                q(&format!("{}\\n{}", u.name, u.offers.join("\\n")))
            ));
        }
        for a in &self.artifacts {
            out.push_str(&format!("  {} [shape=note];\n", q(&format!("artifact {a}"))));
        }
        for e in &self.edges {
            let label = q(&format!("{}/{}", e.ns, e.name));
            if e.providers.is_empty() {
                out.push_str(&format!(
                    "  {} -> {} [label={label}, style=dashed, color=red];\n",
                    q(&e.requirer),
                    q("∅")
                ));
            }
            for p in &e.providers {
                let to = match p {
                    Provider::Unit { unit, .. } => q(unit),
                    Provider::Group { group, .. } => {
                        out.push_str(&format!("  {} [shape=ellipse];\n", q(&format!("group {group}"))));
                        q(&format!("group {group}"))
                    }
                    Provider::Artifact { artifact, .. } => q(&format!("artifact {artifact}")),
                };
                out.push_str(&format!("  {} -> {to} [label={label}];\n", q(&e.requirer)));
            }
        }
        for l in &self.lanes {
            let lane = q(&format!("lane {}", l.name));
            out.push_str(&format!("  {lane} [shape=cylinder];\n"));
            for p in &l.producers {
                out.push_str(&format!("  {} -> {lane};\n", q(p)));
            }
            for c in &l.consumers {
                out.push_str(&format!("  {lane} -> {};\n", q(c)));
            }
        }
        out.push_str("}\n");
        out
    }
}

/// A declared offer, with where it came from.
struct Offer {
    cap:    Capability,
    source: Provider,
}

fn value_kind(v: &CapValue) -> &'static str {
    match v {
        CapValue::Text(_) => "text",
        CapValue::Integer(_) => "integer",
        CapValue::Float(_) => "float",
        CapValue::Bool(_) => "bool",
        CapValue::Version(_) => "version",
    }
}

fn constraint_value(c: &CapConstraint) -> &CapValue {
    match c {
        CapConstraint::Eq(v)
        | CapConstraint::Ne(v)
        | CapConstraint::Gt(v)
        | CapConstraint::Gte(v)
        | CapConstraint::Lt(v)
        | CapConstraint::Lte(v) => v,
    }
}

fn join_or(set: &BTreeSet<String>, empty: &str) -> String {
    if set.is_empty() { empty.to_string() } else { set.iter().cloned().collect::<Vec<_>>().join("|") }
}

/// Everything one resolution needs to see.
struct Resolver<'a> {
    units:     &'a [Unit],
    artifacts: &'a [(String, ArtifactDescription)],
    offers:    &'a [Offer],
    opts:      &'a CheckOptions,
}

impl Resolver<'_> {
    /// The units whose `[hosts]` could take `a`.
    fn hosts_for(&self, a: &ArtifactDescription) -> Vec<String> {
        self.units
            .iter()
            .filter(|u| u.config.hosts.as_ref().is_some_and(|h| h.can_host(&a.kind, a.footprint_bytes())))
            .map(|u| u.name.clone())
            .collect()
    }

    /// Resolve one filter against everything, pushing the findings the resolution produces.
    fn resolve(&self, requirer: &str, filter: &CapFilter, label: &str, findings: &mut Vec<Finding>) -> Vec<Provider> {
        let mut providers: Vec<Provider> =
            self.offers.iter().filter(|o| filter.matches(&o.cap)).map(|o| o.source.clone()).collect();
        let same_name: Vec<&Offer> = self
            .offers
            .iter()
            .filter(|o| o.cap.namespace == filter.namespace && o.cap.name == filter.name)
            .collect();
        let finding = |severity: Severity, kind: &str, message: String| Finding {
            severity,
            kind: kind.into(),
            unit: Some(requirer.to_string()),
            message,
        };

        // type-cross: a constraint that can never compare with any same-name offer's attribute type
        for (attr, c) in &filter.attributes {
            let want = value_kind(constraint_value(c));
            let offered: BTreeSet<&str> =
                same_name.iter().filter_map(|o| o.cap.attributes.get(attr)).map(value_kind).collect();
            if !offered.is_empty() && !offered.contains(want) {
                findings.push(finding(
                    Severity::Error,
                    "type-cross constraint",
                    format!(
                        "{label} {}/{}: attribute {attr:?} is constrained as {want} but every offer carries it as {} — the comparison can never match",
                        filter.namespace,
                        filter.name,
                        offered.into_iter().collect::<Vec<_>>().join("|")
                    ),
                ));
            }
        }

        if providers.is_empty() {
            // artifacts a host could install
            for (aname, a) in self.artifacts {
                if !filter.matches(&a.provides.to_capability()) {
                    continue;
                }
                let hosts = self.hosts_for(a);
                if hosts.is_empty() {
                    findings.push(finding(
                        Severity::Error,
                        "unhostable entry",
                        format!(
                            "{label} {}/{}: artifact {aname:?} ({}, footprint {} B) would satisfy it, but no unit's [hosts] names that kind with budget for it",
                            filter.namespace,
                            filter.name,
                            a.kind,
                            a.footprint_bytes()
                        ),
                    ));
                } else if a.proposed {
                    findings.push(finding(
                        if self.opts.strict_deployed { Severity::Error } else { Severity::Warning },
                        "would bind after acceptance",
                        format!(
                            "{label} {}/{}: no deployed provider; artifact {aname:?} ({}) is proposed — {} would load it only into a shadow lane until a reviewer accepts it",
                            filter.namespace,
                            filter.name,
                            a.kind,
                            hosts.join("|")
                        ),
                    ));
                    providers.push(Provider::Artifact { artifact: aname.clone(), hosts, proposed: true });
                } else {
                    findings.push(finding(
                        if self.opts.strict_deployed { Severity::Error } else { Severity::Warning },
                        "would bind by provisioning",
                        format!(
                            "{label} {}/{}: no deployed provider; artifact {aname:?} ({}) could be installed by {}",
                            filter.namespace,
                            filter.name,
                            a.kind,
                            hosts.join("|")
                        ),
                    ));
                    providers.push(Provider::Artifact { artifact: aname.clone(), hosts, proposed: false });
                }
            }
        }

        if providers.is_empty() {
            let schema_only =
                filter.schema_id.is_some() && same_name.iter().any(|o| filter.matches_ignoring_schema(&o.cap));
            if schema_only {
                let offered: BTreeSet<String> = same_name
                    .iter()
                    .filter(|o| filter.matches_ignoring_schema(&o.cap))
                    .map(|o| o.cap.schema_id.as_deref().unwrap_or("<none>").to_string())
                    .collect();
                findings.push(finding(
                    Severity::Error,
                    "schema-only mismatch",
                    format!(
                        "{label} {}/{} wants schema {:?}; offers match except for schema ({}) — the rollout-window case",
                        filter.namespace,
                        filter.name,
                        filter.schema_id.as_deref().unwrap_or(""),
                        join_or(&offered, "")
                    ),
                ));
            } else {
                findings.push(finding(
                    Severity::Error,
                    "unwired requirement",
                    format!("{label} {}/{}: no declared capability in any unit satisfies it", filter.namespace, filter.name),
                ));
            }
        } else {
            if let Some(r) = &filter.ranking
                && !self
                    .offers
                    .iter()
                    .filter(|o| filter.matches(&o.cap))
                    .any(|o| o.cap.attributes.contains_key(&r.attribute))
            {
                findings.push(finding(
                    Severity::Warning,
                    "unranked ranking",
                    format!(
                        "{label} {}/{} ranks by {:?}, which no matching offer carries",
                        filter.namespace, filter.name, r.attribute
                    ),
                ));
            }
            let deployed = providers.iter().filter(|p| !matches!(p, Provider::Artifact { .. })).count();
            if deployed == 1 && label == "requirement" {
                findings.push(finding(
                    Severity::Warning,
                    "single provider",
                    format!("{label} {}/{}: exactly one possible provider across the deployment", filter.namespace, filter.name),
                ));
            }
        }
        providers
    }
}

/// Run the check. Pure: nothing is read from disk or the mesh.
pub fn check(units: &[Unit], artifacts: &[(String, ArtifactDescription)], opts: &CheckOptions) -> Report {
    let mut findings = Vec::new();

    // ── what is offered, by whom ──────────────────────────────────────────────────────────
    let mut offers: Vec<Offer> = Vec::new();
    for u in units {
        for c in &u.config.capabilities {
            offers.push(Offer {
                cap:    c.build_capability_public(),
                source: Provider::Unit { unit: u.name.clone(), principal: u.config.principal.clone() },
            });
        }
        // A `[[serve]]` skill is an offer too: the unit serves it (while its install is live —
        // which the runtime decides, as it decides whether any capability is up).
        for sv in &u.config.serves {
            offers.push(Offer {
                cap:    crate::Capability::new(sv.ns.as_str(), sv.name.as_str()),
                source: Provider::Unit { unit: u.name.clone(), principal: u.config.principal.clone() },
            });
        }
    }
    // A group asserts its `provides` when it has any member: at design time, when its filter
    // matches at least one declared unit capability.
    let mut group_offers: Vec<Offer> = Vec::new();
    for u in units {
        for g in &u.config.groups {
            let Ok(def) = g.to_def() else { continue }; // validate() already refused a bad one
            let has_member = offers.iter().any(|o| def.filter.matches(&o.cap));
            if has_member {
                for p in &def.provides {
                    group_offers.push(Offer {
                        cap:    p.clone(),
                        source: Provider::Group { group: g.name.clone(), defined_by: u.name.clone() },
                    });
                }
            } else {
                findings.push(Finding {
                    severity: Severity::Error,
                    kind:     "empty group".into(),
                    unit:     Some(u.name.clone()),
                    message:  format!(
                        "group {:?}: no declared capability satisfies its filter {}/{}, so nothing could join",
                        g.name, g.filter.ns, g.filter.name
                    ),
                });
            }
        }
    }
    offers.extend(group_offers);

    // ── schemas: every declared schema id must be one the directory defines (W3) ─────────
    if let Some(known) = &opts.known_schemas {
        let mut unknown = |unit: Option<&str>, what: &str, sid: &Option<String>| {
            if let Some(sid) = sid
                && !known.contains(sid)
            {
                findings.push(Finding {
                    severity: Severity::Error,
                    kind:     "unknown schema".into(),
                    unit:     unit.map(str::to_string),
                    message:  format!("{what} names schema {sid:?}, which the schema directory does not define"),
                });
            }
        };
        for u in units {
            for c in &u.config.capabilities {
                unknown(Some(&u.name), &format!("capability {}/{}", c.ns, c.name), &c.schema_id);
            }
            for r in &u.config.requirements {
                unknown(Some(&u.name), &format!("requirement {}/{}", r.ns, r.name), &r.schema_id);
            }
            for g in &u.config.groups {
                unknown(Some(&u.name), &format!("group {:?} filter", g.name), &g.filter.schema_id);
                for p in &g.provides {
                    unknown(Some(&u.name), &format!("group {:?} provides {}/{}", g.name, p.ns, p.name), &p.schema_id);
                }
                for (i, req) in g.requires.iter().enumerate() {
                    unknown(Some(&u.name), &format!("group {:?} requires[{i}]", g.name), &req.schema_id);
                }
            }
            for p in &u.config.presence {
                unknown(Some(&u.name), &format!("presence {}/{}", p.filter.ns, p.filter.name), &p.filter.schema_id);
            }
        }
        for (aname, a) in artifacts {
            unknown(None, &format!("artifact {aname:?} provides"), &a.provides.schema_id);
        }
    }

    let resolver = Resolver { units, artifacts, offers: &offers, opts };

    // ── requirements ───────────────────────────────────────────────────────────────────
    let mut edges = Vec::new();
    for u in units {
        for r in &u.config.requirements {
            let Ok(filter) = r.to_filter() else { continue };
            let providers = resolver.resolve(&u.name, &filter, "requirement", &mut findings);
            let hosted = if providers.iter().any(|p| is_hosted_component(artifacts, p)) { hosted_call_shape(&r.ns, &r.name) } else { None };
            edges.push(Edge {
                requirer:  u.name.clone(),
                principal: u.config.principal.clone(),
                ns:        r.ns.clone(),
                name:      r.name.clone(),
                schema_id: r.schema_id.clone(),
                providers,
                operations: call_shapes(&r.ns, &r.name)
                    .into_iter()
                    .chain(hosted)
                    .map(|(operation, resource)| CallShape { operation, resource })
                    .collect(),
            });
        }
        for g in &u.config.groups {
            for (i, req) in g.requires.iter().enumerate() {
                let Ok(filter) = req.to_filter() else { continue };
                let providers = resolver.resolve(&u.name, &filter, "group requires", &mut findings);
                if providers.is_empty() {
                    findings.push(Finding {
                        severity: Severity::Error,
                        kind:     "group requires unmet".into(),
                        unit:     Some(u.name.clone()),
                        message:  format!("group {:?} requires[{i}] {}/{} binds to nothing", g.name, req.ns, req.name),
                    });
                }
            }
        }
    }

    // ── presence ───────────────────────────────────────────────────────────────────────
    for u in units {
        for p in &u.config.presence {
            let Ok(filter) = p.filter.to_filter() else { continue };
            let deployed: BTreeSet<String> = offers
                .iter()
                .filter(|o| filter.matches(&o.cap))
                .filter_map(|o| match &o.source {
                    Provider::Unit { unit, .. } => Some(unit.clone()),
                    _ => None,
                })
                .collect();
            let hosting: BTreeSet<String> = artifacts
                .iter()
                .filter(|(_, a)| filter.matches(&a.provides.to_capability()))
                .flat_map(|(_, a)| resolver.hosts_for(a))
                .collect();
            let could: BTreeSet<&String> = deployed.iter().chain(hosting.iter()).collect();
            if could.len() < p.min_providers {
                findings.push(Finding {
                    severity: Severity::Error,
                    kind:     "presence unhostable".into(),
                    unit:     Some(u.name.clone()),
                    message:  format!(
                        "presence {}/{} needs {} providers; {} of {} could exist (deployed: {}; hosting: {})",
                        p.filter.ns,
                        p.filter.name,
                        p.min_providers,
                        could.len(),
                        p.min_providers,
                        join_or(&deployed, "none"),
                        join_or(&hosting, "none")
                    ),
                });
            }
        }
    }

    // ── schedules (adopter-surface Y2) ────────────────────────────────────────────────────────
    // The band is what the runtime elects from: units that run the schedule — declare it, with no `runtime` (an SDK
    // agent's `units/declare` refuses it) and no `[hosts]` (a stem refuses it) — and offer a capability `holders`
    // matches. With none, nothing could hold it and it would never run.
    // A stem is a unit with `[hosts]` or `[[serve]]` (the sections only a stem runs), and it refuses a `[[schedule]]`.
    let stem_unit = |u: &Unit| u.config.hosts.is_some() || !u.config.serves.is_empty();
    let runs_schedules = |u: &Unit| u.config.runtime.is_none() && !stem_unit(u);
    for u in units {
        if !u.config.schedules.is_empty() && stem_unit(u) {
            findings.push(Finding {
                severity: Severity::Error,
                kind:     "schedule in a stem unit".into(),
                unit:     Some(u.name.clone()),
                message:  "a unit with `[hosts]` or `[[serve]]` is a stem's, and a stem refuses to start with a [[schedule]]: the application that embeds the node runs it (GossipAgent::run_schedule)".into(),
            });
        }
    }
    let mut by_id: BTreeMap<&str, Vec<(&Unit, &crate::capability_config::ScheduleDecl)>> = BTreeMap::new();
    for u in units {
        for sc in &u.config.schedules {
            by_id.entry(sc.id.as_str()).or_default().push((u, sc));
        }
    }
    for (id, decls) in &by_id {
        let (u0, first) = decls[0];
        let differ = |a: &crate::capability_config::ScheduleDecl, b: &crate::capability_config::ScheduleDecl|
            (a.period_secs, a.phase_secs, a.on_missed) != (b.period_secs, b.phase_secs, b.on_missed)
                || a.holders.to_filter().ok().map(|f| f.encode()) != b.holders.to_filter().ok().map(|f| f.encode());
        if let Some((u1, _)) = decls.iter().find(|(_, d)| differ(first, d)) {
            findings.push(Finding {
                severity: Severity::Error,
                kind:     "schedule declared two ways".into(),
                unit:     Some(u1.name.clone()),
                message:  format!(
                    "schedule {id:?} is declared by {} and {} with a different period, phase, missed-run policy or holders: one id is one schedule fleet-wide (its candidates, its `last` and its keys), so neither would hold",
                    u0.name, u1.name
                ),
            });
            continue;
        }
        let Ok(filter) = first.holders.to_filter() else { continue };
        let band: Vec<&str> = decls.iter()
            .filter(|(u, _)| runs_schedules(u))
            .filter(|(u, _)| u.config.capabilities.iter().any(|c| filter.matches(&c.build_capability_public())))
            .map(|(u, _)| u.name.as_str())
            .collect();
        if band.is_empty() {
            findings.push(Finding {
                severity: Severity::Error,
                kind:     "schedule without holders".into(),
                unit:     Some(u0.name.clone()),
                message:  format!(
                    "schedule {id:?} names holders {}/{}, and no unit that runs it (declares it, with no `runtime` and no `[hosts]`) declares a capability that matches: no host can hold it",
                    first.holders.ns, first.holders.name
                ),
            });
        }
    }

    // Two bands over one capability, at least one capped, fight when a provider both can match is shed by one and
    // brought back by the other (its floor, or its own different ceiling). Identity is the runtime's (`shed_band`:
    // the filter with ranking and max age cleared), so one band spelled two ways is one band.
    let all: Vec<(&Unit, &crate::capability_config::PresenceDecl, Option<CapFilter>)> = units.iter()
        .flat_map(|u| u.config.presence.iter().map(move |p| (u, p, p.filter.to_filter().ok().map(presence_identity))))
        .collect();
    let mut named: BTreeSet<(String, String)> = BTreeSet::new();
    for (i, (ua, a, fa)) in all.iter().enumerate() {
        for (ub, b, fb) in &all[i + 1..] {
            let (Some(fa), Some(fb)) = (fa, fb) else { continue };
            if (fa.namespace.as_ref(), fa.name.as_ref()) != (fb.namespace.as_ref(), fb.name.as_ref())
                || (a.max_providers.is_none() && b.max_providers.is_none())
                || presence_disjoint(a, b)
            {
                continue;
            }
            if fa.encode() == fb.encode() {
                // One population: two ceilings fight only if they differ; a floor and a ceiling only if the floor is above it.
                let fights = match (a.max_providers, b.max_providers) {
                    (Some(x), Some(y)) => x != y,
                    (Some(max), None) => b.min_providers > max,
                    (None, Some(max)) => a.min_providers > max,
                    (None, None) => false,
                };
                if !fights {
                    continue;
                }
            }
            let (ka, kb) = (band_label(a), band_label(b));
            if !named.insert(if ka <= kb { (ka.clone(), kb.clone()) } else { (kb.clone(), ka.clone()) }) {
                continue;
            }
            findings.push(Finding {
                severity: Severity::Warning,
                kind:     "presence bands overlap".into(),
                unit:     Some(ua.name.clone()),
                message:  format!(
                    "presence {ka} (unit {}) and {kb} (unit {}) can match the same providers: a capped band sheds against \
                     its ceiling while the other's floor or ceiling brings a provider back, so they can trade one back and \
                     forth — give them room, drop a ceiling, or make their filters disjoint",
                    ua.name, ub.name
                ),
            });
        }
    }

    // ── authority overlay (W4) ─────────────────────────────────────────────────────────
    // For an edge whose requirer has declared any authority vocabulary, some declared rule must be
    // able to admit one of the call shapes the edge could take, for the actor its decider reads; a
    // rule that requires a mandate scope needs a declared mandate enumerating the operation.
    if opts.authority {
        // An exposure rule (adopter-surface Y5) is never an allowance: it admits no edge and governs no unit — the
        // runtime's evaluator never loads one.
        let rules: Vec<&crate::capability_config::RuleDecl> = units.iter().flat_map(|u| u.config.rules.iter()).filter(|r| !r.is_exposure()).collect();
        let mandates: Vec<&crate::capability_config::MandateDecl> = units.iter().flat_map(|u| u.config.mandates.iter()).collect();
        let governed = |u: &Unit| !u.config.mandates.is_empty() || u.config.rules.iter().any(|r| !r.is_exposure());
        let any_vocabulary = !rules.is_empty() || !mandates.is_empty();
        let unit_of = |name: &str| units.iter().find(|u| u.name == name);
        for e in &edges {
            if e.providers.is_empty() {
                continue; // already `could not bind`; authority over nothing is not the finding
            }
            let Some(requirer) = unit_of(&e.requirer) else { continue };
            // An edge to a hosted component is decided by the provider that hosts it, from that host's own rules,
            // for the calling node (`hosted_edge`) — so it is read whenever the requirer or a unit that could host it
            // declares authority vocabulary, and on that reading alone: a `blob` beside the component, of which
            // nothing says how it is called, brings no skill or tool reading back.
            let hosts: BTreeSet<&str> = e
                .providers
                .iter()
                .filter(|p| is_hosted_component(artifacts, p))
                .flat_map(|p| match p {
                    Provider::Artifact { hosts, .. } => hosts.iter().map(String::as_str).collect::<Vec<_>>(),
                    _ => Vec::new(),
                })
                .collect();
            if !hosts.is_empty() {
                if governed(requirer) || hosts.iter().any(|h| unit_of(h).is_some_and(governed)) {
                    hosted_edge(e, requirer, &hosts, units, &mandates, &mut findings);
                } else if any_vocabulary {
                    // Neither the requirer nor any unit that could host the component declares vocabulary. Whether a
                    // host enforces at all is its node's `[authority]` table, which no unit file shows — one with it
                    // refuses the call, one without runs it — so this is the warning it always was, not an error;
                    // and it says so, and names a rule for the edge that sits where no host reads it.
                    let elsewhere = admitting_elsewhere(e, requirer, &hosts, units, &mandates);
                    findings.push(Finding {
                        severity: Severity::Warning,
                        kind:     "ungoverned edge".into(),
                        unit:     Some(e.requirer.clone()),
                        message:  format!(
                            "requirement {}/{}: {} and {} — the unit{} that could host it — declare no mandate and no rule, while other units do: this edge is outside the authority vocabulary. A host built from `[authority]` reads its own unit's rules only, and with none for this call it refuses it{}",
                            e.ns,
                            e.name,
                            e.requirer,
                            hosts.iter().copied().collect::<Vec<_>>().join("|"),
                            if hosts.len() == 1 { "" } else { "s" },
                            if elsewhere.is_empty() { String::new() } else { format!("; the rule declared in {} would admit it there, and the hosts do not read it", elsewhere.join(", ")) }
                        ),
                    });
                }
                continue;
            }
            if !governed(requirer) {
                if any_vocabulary {
                    findings.push(Finding {
                        severity: Severity::Warning,
                        kind:     "ungoverned edge".into(),
                        unit:     Some(e.requirer.clone()),
                        message:  format!(
                            "requirement {}/{}: {} declares no mandate and no rule, while other units do — this edge is outside the authority vocabulary",
                            e.ns, e.name, e.requirer
                        ),
                    });
                }
                continue;
            }
            let Some(principal) = requirer.config.principal.as_deref() else {
                findings.push(Finding {
                    severity: Severity::Error,
                    kind:     "unauthorisable edge".into(),
                    unit:     Some(e.requirer.clone()),
                    message:  format!("requirement {}/{}: {} declares authority vocabulary but no principal, so no rule can name it", e.ns, e.name, e.requirer),
                });
                continue;
            };
            // A deployed provider — a unit's own capability, a group — is read as a skill or a tool, for the requirer's
            // unit principal, from any unit's rules: the reading as it stood, which is not the provider's
            // (`what-is-proven.md#wire-check-reads-a-deployed-edge-for-the-unit`, open).
            let mut admitted = false;
            let mut nearest: Option<String> = None;
            for (operation, resource) in call_shapes(&e.ns, &e.name) {
                let enumerated = mandate_operation_form(&operation, &resource);
                for r in &rules {
                    let actor_ok = r.actor == "*" || r.actor == principal;
                    let op_ok = r.operation == "*" || r.operation == operation;
                    let res_ok = r.resource == "*" || r.resource == resource;
                    if !(actor_ok && op_ok && res_ok) {
                        continue;
                    }
                    match &r.requires_mandate {
                        // A rule requiring a grant admits the edge under one: the grant is issued at run time, not
                        // declared (adopter-surface X3); whether anyone may issue it is the pass below.
                        None => {
                            admitted = true;
                        }
                        Some(scope) => {
                            let held = mandates.iter().any(|m| m.holder == principal && &m.scope == scope && m.permits(&enumerated));
                            if held {
                                admitted = true;
                            } else if nearest.is_none() {
                                nearest = Some(format!(
                                    "a rule admits {operation} on {resource} but requires mandate scope {scope:?}, and no declared mandate held by {principal:?} in that scope enumerates {enumerated:?}"
                                ));
                            }
                        }
                    }
                    if admitted {
                        break;
                    }
                }
                if admitted {
                    break;
                }
            }
            if !admitted {
                findings.push(Finding {
                    severity: Severity::Error,
                    kind:     "unauthorisable edge".into(),
                    unit:     Some(e.requirer.clone()),
                    message:  format!(
                        "requirement {}/{}: {}",
                        e.ns,
                        e.name,
                        nearest.unwrap_or_else(|| format!("no declared rule admits {principal:?} calling it as a skill or a tool"))
                    ),
                });
            }
        }
        // A rule that requires a grant needs someone the rules admit to issue one (`grant.issue` on its resource,
        // or on `*`); without that the call waits for a grant nobody may give and is refused on silence — the
        // declaration says default-deny and nothing else (adopter-surface X3, fourth PR).
        for (u, r) in units.iter().flat_map(|u| u.config.rules.iter().map(move |r| (u, r))).filter(|(_, r)| r.requires_grant) {
            let issuable = rules.iter().any(|i| i.admits_grant_issue_for(&r.resource));
            if !issuable {
                findings.push(Finding {
                    severity: Severity::Warning,
                    kind:     "unissuable grant".into(),
                    unit:     Some(u.name.clone()),
                    message:  format!(
                        "a rule admits {:?} {} on {} only under a grant, and no declared rule admits anyone to grant.issue on it: the call waits for a grant nobody may give and is refused on silence",
                        r.actor, r.operation, r.resource
                    ),
                });
            }
        }
    }

    // ── lanes ──────────────────────────────────────────────────────────────────────────
    let mut lanes: BTreeMap<String, Lane> = BTreeMap::new();
    for u in units {
        for l in &u.config.lanes {
            let e = lanes
                .entry(l.name.clone())
                .or_insert_with(|| Lane { name: l.name.clone(), producers: vec![], consumers: vec![] });
            match l.role {
                LaneRole::Produces => e.producers.push(u.name.clone()),
                LaneRole::Consumes => e.consumers.push(u.name.clone()),
            }
        }
    }
    for l in lanes.values() {
        if l.producers.is_empty() || l.consumers.is_empty() {
            findings.push(Finding {
                severity: Severity::Error,
                kind:     "orphan lane".into(),
                unit:     None,
                message:  if l.producers.is_empty() {
                    format!("lane {:?} is consumed by {} and produced by nobody", l.name, l.consumers.join("|"))
                } else {
                    format!("lane {:?} is produced by {} and consumed by nobody", l.name, l.producers.join("|"))
                },
            });
        }
    }

    let unit_summaries = units
        .iter()
        .map(|u| {
            // X8: the adapter contract's row this unit's runtime names; none for an unknown runtime.
            let adapter = match &u.config.runtime {
                None => Some(crate::adapter_contract::native()),
                Some(r) => crate::adapter_contract::for_runtime(r),
            };
            UnitSummary {
            name:        u.name.clone(),
            principal:   u.config.principal.clone(),
            runtime:     u.config.runtime.clone(),
            adapter:     adapter.map(|a| a.name.to_string()),
            carries:     adapter.map(|a| a.carried().iter().map(|p| p.name().to_string()).collect()).unwrap_or_default(),
            not_carried: adapter.map(|a| a.not_carried().iter().map(|p| p.name().to_string()).collect()).unwrap_or_default(),
            offers:      u.config.capabilities.iter().map(|c| format!("{}/{}", c.ns, c.name))
                .chain(u.config.serves.iter().map(|s| format!("{}/{}", s.ns, s.name)))
                .collect(),
            requires:    u.config.requirements.iter().map(|r| format!("{}/{}", r.ns, r.name)).collect(),
            groups:      u.config.groups.iter().map(|g| g.name.clone()).collect(),
            hosts_kinds: u.config.hosts.as_ref().map(|h| h.kinds.clone()).unwrap_or_default(),
            presence:    u
                .config
                .presence
                .iter()
                .map(|p| {
                    format!(
                        "{}/{} {}..{}",
                        p.filter.ns,
                        p.filter.name,
                        p.min_providers,
                        p.max_providers.map(|m| m.to_string()).unwrap_or_else(|| "∞".into())
                    )
                })
                .collect(),
        }})
        .collect();

    // X8: a runtime the adapter contract does not name is a finding by name — the declaration says an SDK will run
    // the unit, and the table cannot say what that SDK carries.
    for u in units {
        // A unit an SDK loads goes through `units/declare`, which refuses the four stem sections (422); saying so
        // here keeps the declaration from describing something no runtime can run (#676's review, finding 8).
        if u.config.runtime.is_some() {
            let stem: Vec<&str> = [
                (u.config.hosts.is_some(), "[hosts]"),
                (!u.config.presence.is_empty(), "[[presence]]"),
                (!u.config.activations.is_empty(), "[[activation]]"),
                (!u.config.serves.is_empty(), "[[serve]]"),
            ].into_iter().filter(|(present, _)| *present).map(|(_, name)| name).collect();
            if !stem.is_empty() {
                findings.push(Finding {
                    severity: Severity::Error,
                    kind:     "runtime with a stem section".into(),
                    unit:     Some(u.name.clone()),
                    message:  format!(
                        "a unit with `runtime` is loaded by an SDK agent through POST /gateway/units/declare, which refuses {} (422): the hosting sections are a stem's",
                        stem.join(", ")
                    ),
                });
            }
        }
        if u.config.runtime.is_some() && !u.config.schedules.is_empty() {
            findings.push(Finding {
                severity: Severity::Error,
                kind:     "runtime with a schedule".into(),
                unit:     Some(u.name.clone()),
                message:  "a unit with `runtime` is loaded by an SDK agent through POST /gateway/units/declare, which refuses [[schedule]] (422): an SDK agent runs a schedule on GET /gateway/schedules/run, with the section, not in its unit".into(),
            });
        }
        if let Some(r) = &u.config.runtime
            && crate::adapter_contract::for_runtime(r).is_none()
        {
            findings.push(Finding {
                severity: Severity::Error,
                kind:     "unknown runtime".into(),
                unit:     Some(u.name.clone()),
                message:  format!(
                    "runtime {r:?} is not a row of the adapter contract (docs/reference/adapter-contract.md); the runtimes are {}",
                    crate::adapter_contract::runtimes().iter().map(|r| format!("`{r}`")).collect::<Vec<_>>().join(", ")
                ),
            });
        }
    }

    Report {
        schema:    DECLARATION_SCHEMA.into(),
        revision:  opts.revision.clone(),
        units:     unit_summaries,
        artifacts: artifacts.iter().map(|(n, _)| n.clone()).collect(),
        edges,
        lanes:     lanes.into_values().collect(),
        findings,
    }
}

/// A presence filter's identity, as the runtime reads it (`mycelium_wasm_host::provisioner::shed_band`): ranking and
/// max age cleared.
fn presence_identity(f: CapFilter) -> CapFilter {
    CapFilter { ranking: None, max_age: None, ..f }
}

/// `ns/name{attrs}@schema min..max` — how a presence band is named in a finding.
fn band_label(p: &crate::capability_config::PresenceDecl) -> String {
    let attrs: Vec<String> = p.filter.attrs.iter()
        .map(|(k, v)| format!("{k}={}", v.to_constraint().map_or_else(|e| e, |c| format!("{c:?}"))))
        .collect();
    let attrs = if attrs.is_empty() { String::new() } else { format!("{{{}}}", attrs.join(",")) };
    let schema = p.filter.schema_id.as_ref().map_or(String::new(), |s| format!("@{s}"));
    format!("{}/{}{attrs}{schema} {}..{}", p.filter.ns, p.filter.name, p.min_providers,
            p.max_providers.map_or("∞".into(), |m| m.to_string()))
}

/// Whether no capability can match both bands: different `schema_id`s (both set), or some attribute each constrains
/// to **equal** a different value. Anything else (`ne`, ranges, an attribute only one names) may overlap.
fn presence_disjoint(a: &crate::capability_config::PresenceDecl, b: &crate::capability_config::PresenceDecl) -> bool {
    use crate::capability::CapConstraint;
    if let (Some(x), Some(y)) = (&a.filter.schema_id, &b.filter.schema_id)
        && x != y
    {
        return true;
    }
    a.filter.attrs.iter().any(|(k, ca)| {
        b.filter.attrs.get(k).is_some_and(|cb| match (ca.to_constraint(), cb.to_constraint()) {
            (Ok(CapConstraint::Eq(x)), Ok(CapConstraint::Eq(y))) => x != y,
            _ => false,
        })
    })
}

#[cfg(test)]
mod tests {

    /// X8: a directory mixing a native unit and a Strands unit says, per unit, what each runtime carries — from the
    /// adapter contract's rows — and an unknown runtime is a finding by name.
    #[test]
    fn a_units_runtime_names_what_it_carries_and_an_unknown_one_is_a_finding() {
        let native = Unit { name: "worker".into(), config: NodeCapabilityConfig::from_toml_str("principal = \"worker\"\n[[capability]]\nns = \"svc\"\nname = \"worker\"\nttl_secs = 5\n").unwrap() };
        let strands = Unit { name: "planner".into(), config: NodeCapabilityConfig::from_toml_str("principal = \"planner\"\nruntime = \"strands\"\n").unwrap() };
        let r = check(&[native.clone(), strands.clone()], &[], &CheckOptions::default());
        assert_eq!(r.exit_code(), 0, "{}", r.render_text());
        let worker = r.units.iter().find(|u| u.name == "worker").unwrap();
        assert_eq!(worker.adapter.as_deref(), Some("native"));
        assert!(worker.carries.iter().any(|p| p == "tracing") && worker.not_carried.iter().any(|p| p == "cancellation"), "{worker:?}");
        let planner = r.units.iter().find(|u| u.name == "planner").unwrap();
        assert_eq!((planner.runtime.as_deref(), planner.adapter.as_deref()), (Some("strands"), Some("strands-session-mycelium")));
        assert!(planner.carries.iter().any(|p| p == "session persistence") && planner.not_carried.iter().any(|p| p == "tracing"), "{planner:?}");
        let text = r.render_text();
        assert!(text.contains("unit worker (runtime native, native): carries ") && text.contains("unit planner (runtime strands, strands-session-mycelium): carries session persistence"), "{text}");

        let cobol = Unit { name: "legacy".into(), config: NodeCapabilityConfig::from_toml_str("principal = \"legacy\"\nruntime = \"cobol\"\n").unwrap() };
        let r = check(&[native, strands, cobol], &[], &CheckOptions::default());
        assert_eq!(r.exit_code(), 1);
        let f = r.findings.iter().find(|f| f.kind == "unknown runtime").expect("the finding by name");
        assert_eq!(f.unit.as_deref(), Some("legacy"));
        assert!(f.message.contains("\"cobol\"") && f.message.contains("`strands`"), "{}", f.message);
        assert!(r.render_text().contains("unit legacy (runtime cobol): not a row of the adapter contract"));
        assert!(NodeCapabilityConfig::from_toml_str("runtime = \"\"\n").is_err(), "an empty runtime is refused by the loader, as an empty principal is");

        // A runtime unit with a stem section describes what no SDK can declare: a finding by name (#676's review).
        let hosting = Unit { name: "hoster".into(), config: NodeCapabilityConfig::from_toml_str("principal = \"hoster\"\nruntime = \"python\"\n[hosts]\nkinds = [\"blob\"]\ntrusted_publishers = [\"ed25519:2152f8d19b791d24453242e15f2eab6cb7cffa7b6a5ed30097960e069881db12\"]\nplacement_root = \"/var/lib/mycelium/artifacts\"\n").unwrap() };
        let r = check(std::slice::from_ref(&hosting), &[], &CheckOptions::default());
        let f = r.findings.iter().find(|f| f.kind == "runtime with a stem section").expect("the finding");
        assert!(f.message.contains("[hosts]") && f.message.contains("422"), "{}", f.message);
    }
    use super::*;

    fn unit(name: &str, toml: &str) -> Unit {
        Unit {
            name:   name.into(),
            config: NodeCapabilityConfig::from_toml_str(toml).unwrap_or_else(|e| panic!("{name}: {e}")),
        }
    }
    fn kinds(r: &Report) -> Vec<(&str, Severity)> {
        r.findings.iter().map(|f| (f.kind.as_str(), f.severity)).collect()
    }

    #[test]
    fn a_wired_deployment_has_no_errors_and_names_its_bindings() {
        let units = vec![
            unit("llm-a", "[[capability]]\nns=\"llm\"\nname=\"inference\"\n[capability.attrs]\nmodel=\"llama3.2\"\ncontext=8192\n"),
            unit("llm-b", "[[capability]]\nns=\"llm\"\nname=\"inference\"\n[capability.attrs]\nmodel=\"llama3.2\"\ncontext=16384\n"),
            unit("planner", "principal=\"planner\"\n[[requirement]]\nns=\"llm\"\nname=\"inference\"\n[requirement.attrs]\nmodel=\"llama3.2\"\ncontext={ gte = 8192 }\n[requirement.ranking]\nattribute=\"context\"\norder=\"descending\"\n[[lane]]\nname=\"plans\"\nrole=\"produces\"\n"),
            unit("executor", "[[lane]]\nname=\"plans\"\nrole=\"consumes\"\n"),
        ];
        let r = check(&units, &[], &CheckOptions::default());
        assert_eq!(r.exit_code(), 0, "{}", r.render_text());
        assert!(r.findings.is_empty(), "{:?}", r.findings);
        assert_eq!(r.edges.len(), 1);
        assert_eq!(r.edges[0].providers.len(), 2);
        assert_eq!(r.edges[0].principal.as_deref(), Some("planner"));
        assert!(r.render_text().contains("would bind via llm-a, llm-b"));
        assert!(r.render_dot().contains("\"planner\" -> \"llm-a\""));
        let json: serde_json::Value = serde_json::from_str(&r.render_json()).unwrap();
        assert_eq!(json["schema"], DECLARATION_SCHEMA);
    }

    /// Y2's exit: a `[[schedule]]` is refused by `wire-check` without a declared holder band — no unit offers what
    /// its `holders` names — and accepted once one does.
    #[test]
    fn a_schedule_without_a_declared_holder_band_is_refused() {
        let sched = "[[schedule]]\nid=\"close\"\nperiod_secs=3600\n[schedule.holders]\nns=\"ledger\"\nname=\"closer\"\n";
        let closer = "[[capability]]\nns=\"ledger\"\nname=\"closer\"\n";
        let has = |r: &Report, k: &str| kinds(r).contains(&(k, Severity::Error));
        let r = check(&[unit("books", sched)], &[], &CheckOptions::default());
        assert!(has(&r, "schedule without holders"), "{:?}", r.findings);
        // A unit that offers the capability but does not run the schedule is not in the band — the runtime's rule.
        let r = check(&[unit("books", sched), unit("closer", closer)], &[], &CheckOptions::default());
        assert!(has(&r, "schedule without holders"), "a provider that does not run it cannot hold it: {:?}", r.findings);
        let r = check(&[unit("books", &format!("{sched}{closer}"))], &[], &CheckOptions::default());
        assert_eq!(r.exit_code(), 0, "{}", r.render_text());
        // A stem unit and an SDK unit cannot run one.
        let stem = format!("{sched}{closer}[hosts]\nkinds=[\"blob\"]\naccept_unsigned=true\n");
        let r = check(&[unit("books", &stem)], &[], &CheckOptions::default());
        assert!(has(&r, "schedule without holders") && has(&r, "schedule in a stem unit"), "{:?}", r.findings);
        let serving = format!("{sched}[[serve]]\nns=\"ledger\"\nname=\"closer\"\nendpoint=\"http://127.0.0.1:9/v1\"\nmodel=\"m\"\n");
        let r = check(&[unit("books", &serving)], &[], &CheckOptions::default());
        assert!(has(&r, "schedule without holders") && has(&r, "schedule in a stem unit"), "a served skill is not a holder: {:?}", r.findings);
        let sdk = format!("runtime=\"python\"\n{sched}{closer}");
        let r = check(&[unit("books", &sdk)], &[], &CheckOptions::default());
        assert!(has(&r, "runtime with a schedule") && has(&r, "schedule without holders"), "{:?}", r.findings);
        // One id, two declarations.
        let other = sched.replace("period_secs=3600", "period_secs=60");
        let r = check(&[unit("books", &format!("{sched}{closer}")), unit("ledger", &format!("{other}{closer}"))], &[], &CheckOptions::default());
        assert!(has(&r, "schedule declared two ways"), "{:?}", r.findings);
        let r = check(&[unit("books", &format!("{sched}{closer}")), unit("ledger", &format!("{sched}{closer}"))], &[], &CheckOptions::default());
        assert_eq!(r.exit_code(), 0, "the same declaration twice is one schedule: {}", r.render_text());
    }

    #[test]
    fn every_error_class_is_reported_by_name() {
        let units = vec![
            unit("offers", "[[capability]]\nns=\"llm\"\nname=\"inference\"\n[capability.attrs]\ncontext=8192\n[[capability]]\nns=\"plan\"\nname=\"route\"\n"),
            unit("wants", concat!(
                "[[requirement]]\nns=\"llm\"\nname=\"inference\"\nschema_id=\"llm.v2\"\n",
                "[[requirement]]\nns=\"llm\"\nname=\"inference\"\n[requirement.attrs]\ncontext=\"big\"\n",
                "[[requirement]]\nns=\"nobody\"\nname=\"has-this\"\n",
                "[[group]]\nname=\"ghosts\"\n[group.filter]\nns=\"ghost\"\nname=\"town\"\n",
                "[[group]]\nname=\"routers\"\n[group.filter]\nns=\"plan\"\nname=\"route\"\n[[group.requires]]\nns=\"data\"\nname=\"realtime\"\n",
                "[[lane]]\nname=\"orphan\"\nrole=\"consumes\"\n",
                "[[presence]]\nns=\"route\"\nname=\"optimize\"\nmin_providers=2\n",
            )),
        ];
        let r = check(&units, &[], &CheckOptions::default());
        assert_eq!(r.exit_code(), 1);
        let k: Vec<&str> = kinds(&r).into_iter().map(|(k, _)| k).collect();
        for want in [
            "schema-only mismatch",
            "type-cross constraint",
            "unwired requirement",
            "empty group",
            "group requires unmet",
            "orphan lane",
            "presence unhostable",
        ] {
            assert!(k.contains(&want), "missing {want:?} in {k:?}\n{}", r.render_text());
        }
        let text = r.render_text();
        assert!(text.contains("wants schema \"llm.v2\""), "{text}");
        assert!(text.contains("constrained as text but every offer carries it as integer"), "{text}");
        assert!(text.contains("0 of 2 could exist"), "{text}");
    }

    #[test]
    fn provisioning_binds_only_where_a_unit_could_host_it() {
        let art = (
            "route-optimizer".to_string(),
            ArtifactDescription::from_toml_str(
                "kind=\"wasm-component\"\n[provides]\nns=\"route\"\nname=\"optimize\"\n[requires]\ndisk_bytes=0\nmem_bytes=67108864\n",
            )
            .unwrap(),
        );
        let worker = unit("worker", "[[requirement]]\nns=\"route\"\nname=\"optimize\"\n[[presence]]\nns=\"route\"\nname=\"optimize\"\nmin_providers=2\n");

        // nobody hosts → unhostable, and presence cannot be met
        let r = check(std::slice::from_ref(&worker), std::slice::from_ref(&art), &CheckOptions::default());
        assert!(kinds(&r).contains(&("unhostable entry", Severity::Error)), "{}", r.render_text());
        assert!(kinds(&r).contains(&("presence unhostable", Severity::Error)));

        // one host with budget below the footprint → still unhostable
        let small = unit("depot-small", "[hosts]\nkinds=[\"wasm-component\"]\ninstall_budget_bytes=1024\naccept_unsigned=true\n");
        let r = check(&[worker.clone(), small], std::slice::from_ref(&art), &CheckOptions::default());
        assert!(kinds(&r).contains(&("unhostable entry", Severity::Error)), "{}", r.render_text());

        // two hosts with budget → would bind by provisioning (a warning), presence 2 of 2, exit 0
        let a = unit("depot-a", "[hosts]\nkinds=[\"wasm-component\",\"blob\"]\ninstall_budget_bytes=8589934592\naccept_unsigned=true\n");
        let b = unit("depot-b", "[hosts]\nkinds=[\"wasm-component\"]\naccept_unsigned=true\n");
        let r = check(&[worker.clone(), a.clone(), b.clone()], std::slice::from_ref(&art), &CheckOptions::default());
        assert_eq!(r.exit_code(), 0, "{}", r.render_text());
        assert!(kinds(&r).contains(&("would bind by provisioning", Severity::Warning)));
        assert!(matches!(&r.edges[0].providers[0], Provider::Artifact { hosts, .. } if hosts == &vec!["depot-a".to_string(), "depot-b".to_string()]));

        // strict: the same deployment is an error
        let r = check(&[worker, a, b], std::slice::from_ref(&art), &CheckOptions { strict_deployed: true, revision: Some("abc".into()), ..Default::default() });
        assert_eq!(r.exit_code(), 1);
        assert_eq!(r.revision.as_deref(), Some("abc"));
    }

    #[test]
    fn warnings_do_not_fail_the_check() {
        let units = vec![
            unit("only", "[[capability]]\nns=\"a\"\nname=\"b\"\n"),
            unit("needs", "[[requirement]]\nns=\"a\"\nname=\"b\"\n[requirement.ranking]\nattribute=\"speed\"\norder=\"ascending\"\n"),
        ];
        let r = check(&units, &[], &CheckOptions::default());
        assert_eq!(r.exit_code(), 0);
        assert_eq!(kinds(&r), vec![("unranked ranking", Severity::Warning), ("single provider", Severity::Warning)]);
    }

    /// W3: with the schema directory's ids known, an id nobody defined is an error wherever it is
    /// declared, and a provider on `v1` against a requirement on `v2` is the rollout-window case,
    /// named with both ids — a capability now carries its schema in the file too.
    #[test]
    fn schemas_are_checked_against_the_directory_and_the_window_is_named() {
        let units = vec![
            unit("llm-v1", "[[capability]]\nns=\"llm\"\nname=\"inference\"\nschema_id=\"llm/inference/v1\"\n[capability.attrs]\ncontext=8192\n"),
            unit("planner", "[[requirement]]\nns=\"llm\"\nname=\"inference\"\nschema_id=\"llm/inference/v2\"\n[[requirement]]\nns=\"plan\"\nname=\"route\"\nschema_id=\"plan/route/v9\"\n"),
        ];
        let known: BTreeSet<String> = ["llm/inference/v1", "llm/inference/v2"].into_iter().map(String::from).collect();
        let r = check(&units, &[], &CheckOptions { known_schemas: Some(known), ..Default::default() });
        let text = r.render_text();
        assert!(kinds(&r).contains(&("unknown schema", Severity::Error)), "{text}");
        assert!(text.contains("requirement plan/route names schema \"plan/route/v9\""), "{text}");
        assert!(kinds(&r).contains(&("schema-only mismatch", Severity::Error)), "{text}");
        assert!(text.contains("wants schema \"llm/inference/v2\"; offers match except for schema (llm/inference/v1)"), "{text}");
        // Without a schema directory the same deployment reports only the window and the unwired route.
        let r = check(&units, &[], &CheckOptions::default());
        assert!(!kinds(&r).iter().any(|(k, _)| *k == "unknown schema"));
    }

    /// W4: the authority overlay. A governed requirer's edge must be admissible by some declared
    /// rule; a rule that requires a mandate scope needs a mandate held by the requirer's principal
    /// that enumerates the operation; an ungoverned unit beside governed ones is a warning; with
    /// `authority` off, none of this runs.
    #[test]
    fn the_authority_overlay_finds_edges_no_declaration_could_authorise() {
        let provider = unit("optimizer", "[[capability]]\nns=\"route\"\nname=\"optimize\"\n[[capability]]\nns=\"depot\"\nname=\"intake\"\n");
        // Admitted: a rule requiring scope "routing", and a mandate in that scope enumerating the skill form.
        let ok = unit("worker", concat!(
            "principal=\"worker\"\n[[requirement]]\nns=\"route\"\nname=\"optimize\"\n",
            "[[mandate]]\nholder=\"worker\"\nscope=\"routing\"\noperations=[\"skill.invoke:skill:route/optimize\"]\n",
            "[[rule]]\nactor=\"worker\"\noperation=\"skill.invoke\"\nresource=\"skill:route/optimize\"\nrequires_mandate=\"routing\"\n",
        ));
        let r = check(&[provider.clone(), ok.clone()], &[], &CheckOptions::default());
        assert_eq!(r.exit_code(), 0, "{}", r.render_text());
        assert!(r.findings.iter().all(|f| f.kind != "unauthorisable edge"), "{}", r.render_text());

        // Refused: the rule requires a scope no declared mandate held by the principal enumerates.
        let no_mandate = unit("worker", concat!(
            "principal=\"worker\"\n[[requirement]]\nns=\"route\"\nname=\"optimize\"\n",
            "[[rule]]\nactor=\"worker\"\noperation=\"skill.invoke\"\nresource=\"skill:route/optimize\"\nrequires_mandate=\"depot\"\n",
        ));
        let r = check(&[provider.clone(), no_mandate], &[], &CheckOptions::default());
        let text = r.render_text();
        assert!(kinds(&r).contains(&("unauthorisable edge", Severity::Error)), "{text}");
        assert!(text.contains("requires mandate scope \"depot\", and no declared mandate held by \"worker\""), "{text}");

        // Refused: a mandate exists but enumerates a different operation; no rule matches at all.
        let wrong_op = unit("worker", concat!(
            "principal=\"worker\"\n[[requirement]]\nns=\"depot\"\nname=\"intake\"\n",
            "[[mandate]]\nholder=\"worker\"\nscope=\"routing\"\noperations=[\"skill.invoke:skill:route/optimize\"]\n",
        ));
        let r = check(&[provider.clone(), wrong_op], &[], &CheckOptions::default());
        let text = r.render_text();
        assert!(text.contains("no declared rule admits \"worker\" calling it as a skill or a tool"), "{text}");

        // A wildcard rule without a mandate requirement admits; an ungoverned unit beside it warns.
        let open = unit("auditor", "[[rule]]\nactor=\"*\"\noperation=\"*\"\nresource=\"*\"\n");
        let plain = unit("dispatcher", "[[requirement]]\nns=\"depot\"\nname=\"intake\"\n");
        let r = check(&[provider.clone(), ok.clone(), open, plain], &[], &CheckOptions::default());
        assert_eq!(r.exit_code(), 0, "{}", r.render_text());
        assert!(kinds(&r).contains(&("ungoverned edge", Severity::Warning)), "{}", r.render_text());

        // Off: nothing of the above.
        let r = check(&[provider, ok], &[], &CheckOptions { authority: false, ..Default::default() });
        assert!(r.findings.iter().all(|f| !f.kind.contains("edge")), "{}", r.render_text());
    }

    /// **An exposure rule is never an allowance** (adopter-surface Y5; #701's review, finding 3): the
    /// `a2a_skill_authority` example with its weather rule turned into an exposure rule still reports the weather edge
    /// `unauthorisable` — the runtime's evaluator never loads such a rule, so nothing admits the call — and a unit whose
    /// only rule is one is not governed. Seen failing first: 0 errors, the exposure rule read as admitting.
    #[test]
    fn an_exposure_rule_admits_no_edge_and_governs_no_unit() {
        let client = unit("client", include_str!("../examples/units/a2a_skill_authority/client.toml"));
        let gateway_text = include_str!("../examples/units/a2a_skill_authority/gateway.toml");
        let weather = "resource  = \"skill:weather/lookup\"";
        assert!(gateway_text.contains(weather), "the example names its weather rule");
        let exposed = gateway_text.replacen(weather, &format!("{weather}\nexposure  = {{ max_unresolved = 1 }}"), 1);
        let gateway = unit("gateway", &exposed);
        let r = check(&[client.clone(), gateway], &[], &CheckOptions::default());
        let text = r.render_text();
        assert!(
            r.findings.iter().any(|f| f.kind == "unauthorisable edge" && f.message.contains("weather/lookup")),
            "the weather edge is admitted by nothing once its rule is an exposure rule: {text}"
        );
        // A requirer whose only rule is an exposure rule is outside the authority vocabulary.
        let provider = unit("optimizer", "[[capability]]\nns=\"route\"\nname=\"optimize\"\n");
        let only_exposure = unit("worker", concat!(
            "principal=\"worker\"\n[[requirement]]\nns=\"route\"\nname=\"optimize\"\n",
            "[[rule]]\nactor=\"worker\"\noperation=\"skill.invoke\"\nresource=\"*\"\nexposure = { max_unresolved = 1 }\n",
        ));
        let governed = unit("auditor", "principal=\"auditor\"\n[[mandate]]\nholder=\"auditor\"\nscope=\"s\"\noperations=[\"x\"]\n");
        let r = check(&[provider, only_exposure, governed], &[], &CheckOptions::default());
        assert!(kinds(&r).contains(&("ungoverned edge", Severity::Warning)), "{}", r.render_text());
    }

    /// A rule requiring a grant is admitted at the edge — the grant is issued at run time — and warns when no rule
    /// admits anyone to issue it (adopter-surface X3, fourth PR).
    #[test]
    fn a_grant_required_rule_admits_the_edge_and_warns_when_nobody_may_issue() {
        let provider = unit("approver", "[[capability]]\nns=\"purchase\"\nname=\"approve\"\n");
        let lone = unit("purchaser", concat!(
            "principal=\"purchaser\"\n[[requirement]]\nns=\"purchase\"\nname=\"approve\"\n",
            "[[rule]]\nactor=\"purchaser\"\noperation=\"tools/call\"\nresource=\"tool:approve\"\nrequires_grant=true\n",
        ));
        let r = check(&[provider.clone(), lone], &[], &CheckOptions::default());
        let text = r.render_text();
        assert_eq!(r.exit_code(), 0, "{text}");
        assert!(r.findings.iter().all(|f| f.kind != "unauthorisable edge"), "{text}");
        assert!(kinds(&r).contains(&("unissuable grant", Severity::Warning)), "{text}");
        assert!(text.contains("no declared rule admits anyone to grant.issue on it"), "{text}");
        let issued = unit("purchaser", concat!(
            "principal=\"purchaser\"\n[[requirement]]\nns=\"purchase\"\nname=\"approve\"\n",
            "[[rule]]\nactor=\"purchaser\"\noperation=\"tools/call\"\nresource=\"tool:approve\"\nrequires_grant=true\n",
            "[[rule]]\nactor=\"treasurer\"\noperation=\"grant.issue\"\nresource=\"tool:approve\"\n",
        ));
        let r = check(&[provider.clone(), issued], &[], &CheckOptions::default());
        assert!(r.findings.iter().all(|f| f.kind != "unissuable grant"), "{}", r.render_text());
        // The evaluator's `*` admits the issue too, and a `@node` suffix on the grant rule is not part of the match
        // (#680's review, findings 2 and 3).
        let star = unit("purchaser", concat!(
            "principal=\"purchaser\"\n[[requirement]]\nns=\"purchase\"\nname=\"approve\"\n",
            "[[rule]]\nactor=\"purchaser\"\noperation=\"tools/call\"\nresource=\"tool:approve@127.0.0.1:1\"\nrequires_grant=true\n",
            "[[rule]]\nactor=\"treasurer\"\noperation=\"*\"\nresource=\"tool:approve\"\n",
        ));
        let r = check(&[provider, star], &[], &CheckOptions::default());
        assert!(r.findings.iter().all(|f| f.kind != "unissuable grant"), "{}", r.render_text());
    }

    /// The overlay's operation string is the gateway's own rule, pinned equal under the build
    /// that has it.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    #[test]
    fn the_overlay_restates_the_gateway_mandate_operation_exactly() {
        for (op, res) in [("tools/call", "tool:ledger@depot-node"), ("skill.invoke", "skill:depot/dispatch@x"), ("tools/call", "tool:bare")] {
            assert_eq!(mandate_operation_form(op, res), crate::mandate_operation(op, res));
        }
    }

    /// The route optimizer as a hosted component, the unit `depot` that can host it and declares `host` (its own
    /// rules — the ones a provider built from `[authority]` loads), and a worker that requires it and declares
    /// `requirer` — the co-op fixture's shape (`tests/fixtures/units/coop` over `artifacts/route-optimizer.toml`).
    fn hosted_in(ns: &str, host: &str, requirer: &str) -> Report {
        let art = (
            "optimizer".to_string(),
            ArtifactDescription::from_toml_str(&format!("kind=\"wasm-component\"\n[provides]\nns=\"{ns}\"\nname=\"optimize\"\n")).unwrap(),
        );
        let depot = unit("depot", &format!("principal=\"depot\"\n[hosts]\nkinds=[\"wasm-component\"]\naccept_unsigned=true\n{host}"));
        let worker = unit("worker", &format!("principal=\"worker\"\n[[requirement]]\nns=\"{ns}\"\nname=\"optimize\"\n{requirer}"));
        check(&[depot, worker], std::slice::from_ref(&art), &CheckOptions::default())
    }

    /// [`hosted_in`] with the authority declared where the provider reads it: in the host.
    fn hosted(ns: &str, authority: &str) -> Report {
        hosted_in(ns, authority, "")
    }

    fn has(r: &Report, kind: &str) -> bool {
        r.findings.iter().any(|f| f.kind == kind)
    }

    /// **The third call shape: a `cap.invoke` rule admits an edge to a hosted component** (the false red of
    /// `docs/operations/what-is-proven.md#wire-check-does-not-know-the-hosted-capability-shape`). An enforcing provider
    /// decides a member's direct call of a hosted component outside the `tool` namespace as `cap.invoke` on
    /// `cap:{ns}/{name}@{node}` for the actor `node:{sender}` (`provider_enforcement::derive_call`), so a rule on
    /// that operation and resource whose actor is `*` or a node admits the edge — written bare, as `[authority]`
    /// qualifies it to its node, or already qualified to a node. The edge's document carries the shape. The requirer
    /// declares nothing here: a hosted edge is decided by its host's rules, so it is read whenever a host
    /// declares authority vocabulary. Seen failing first: on the code before 2026-11-18 (two shapes, the requirer's unit principal as the
    /// actor) the first rule below was reported `unauthorisable edge`, exit 1.
    #[test]
    fn a_cap_invoke_rule_for_any_node_admits_an_edge_to_a_hosted_component() {
        for (actor, resource) in [("*", "cap:route/optimize"), ("node:10.0.0.7:7946", "cap:route/optimize"), ("*", "cap:route/optimize@10.0.0.2:7946"), ("node:[::1]:7946", "*"), ("*", "*")] {
            let r = hosted("route", &format!("[[rule]]\nactor=\"{actor}\"\noperation=\"cap.invoke\"\nresource=\"{resource}\"\n"));
            assert_eq!(r.exit_code(), 0, "{actor} on {resource}: {}", r.render_text());
            assert!(r.findings.iter().all(|f| !f.kind.ends_with(" edge")), "{actor} on {resource}: {}", r.render_text());
            let shapes: Vec<(&str, &str)> = r.edges[0].operations.iter().map(|s| (s.operation.as_str(), s.resource.as_str())).collect();
            assert_eq!(
                shapes,
                vec![("skill.invoke", "skill:route/optimize"), ("tools/call", "tool:optimize"), ("cap.invoke", "cap:route/optimize")],
                "the two shapes every edge carries, then the hosted one"
            );
        }
        // Another capability's rule, or another operation's, admits nothing here.
        for rule in ["actor=\"*\"\noperation=\"cap.invoke\"\nresource=\"cap:route/other\"\n", "actor=\"*\"\noperation=\"llm.invoke\"\nresource=\"cap:route/optimize\"\n"] {
            let r = hosted("route", &format!("[[rule]]\n{rule}"));
            assert!(has(&r, "unauthorisable edge"), "{rule}: {}", r.render_text());
        }
        // A rule under a grant is admitted under one, as on every edge: the grant is issued at run time.
        let granted = hosted("route", "[[rule]]\nactor=\"*\"\noperation=\"cap.invoke\"\nresource=\"cap:route/optimize\"\nrequires_grant=true\n[[rule]]\nactor=\"*\"\noperation=\"grant.issue\"\nresource=\"*\"\n");
        assert!(granted.findings.iter().all(|f| !f.kind.ends_with(" edge")), "{}", granted.render_text());
    }

    /// **A skill or tool rule does not admit an edge to a hosted component** (the false green of the same ledger
    /// row; the co-op fixture's `auditor.toml` as it stood). The provider that hosts `route/optimize` decides
    /// `cap.invoke` and never consults a `skill.invoke` or `tools/call` rule, so an edge only such a rule would
    /// admit is `unauthorisable edge`, and the finding says which rule the provider does not read. Seen failing
    /// first: on the code before 2026-11-18 the declaration below — the fixture's own rule and mandate — was
    /// reported clean, exit 0.
    #[test]
    fn a_skill_rule_does_not_admit_an_edge_to_a_hosted_component() {
        let skill = concat!(
            "[[rule]]\nactor=\"worker\"\noperation=\"skill.invoke\"\nresource=\"skill:route/optimize\"\nrequires_mandate=\"routing\"\n",
            "[[mandate]]\nholder=\"worker\"\nscope=\"routing\"\noperations=[\"skill.invoke:skill:route/optimize\"]\n",
        );
        for r in [hosted("route", skill), hosted_in("route", "", skill)] {
            assert_eq!(r.exit_code(), 1, "FALSE GREEN: {}", r.render_text());
            let finding = r.findings.iter().find(|f| f.kind == "unauthorisable edge").expect("the edge is named");
            assert_eq!(finding.unit.as_deref(), Some("worker"));
            assert!(finding.message.contains("cap.invoke on cap:route/optimize"), "{}", finding.message);
            assert!(finding.message.contains("skill.invoke on skill:route/optimize") && finding.message.contains("does not consult"), "{}", finding.message);
        }
        let tool = hosted("route", "[[rule]]\nactor=\"*\"\noperation=\"tools/call\"\nresource=\"tool:optimize\"\n");
        assert!(has(&tool, "unauthorisable edge"), "FALSE GREEN: {}", tool.render_text());

        // What is unchanged. A deployed provider is read as a skill or a tool, as before: the same rule admits it,
        // and its edge carries the two shapes.
        let deployed = unit("optimizer", "[[capability]]\nns=\"route\"\nname=\"optimize\"\n");
        let worker = unit("worker", &format!("principal=\"worker\"\n[[requirement]]\nns=\"route\"\nname=\"optimize\"\n{skill}"));
        let r = check(&[deployed, worker], &[], &CheckOptions::default());
        assert!(r.exit_code() == 0 && !has(&r, "unauthorisable edge"), "{}", r.render_text());
        assert_eq!(r.edges[0].operations.len(), 2);
    }

    /// **A hosted component in the `tool` namespace is read for the calling node too** (#814's review, F3). It is
    /// decided as the tool it is — `tools/call` on `tool:{name}@{node}`, the shape every edge already carries, so its
    /// edge has two — and a member's direct call of it for `node:{sender}`: the rule for any node passes, a rule
    /// naming the requirer's unit principal is `node-decided edge`, and a `cap.invoke` or skill rule admits nothing.
    /// The provider's half is `a_hosted_tool_is_decided_for_the_calling_node_as_wire_check_reads_it`. Seen failing
    /// first: on the overlay at #814's first head the unit-principal rule passed with no finding, and this test's
    /// predecessor asserted that it did.
    #[test]
    fn a_hosted_tool_is_read_for_the_calling_node() {
        let rule = |actor: &str, operation: &str, resource: &str| format!("[[rule]]\nactor=\"{actor}\"\noperation=\"{operation}\"\nresource=\"{resource}\"\n");
        let any = hosted("tool", &rule("*", "tools/call", "tool:optimize"));
        assert!(any.exit_code() == 0 && any.findings.iter().all(|f| !f.kind.ends_with(" edge")), "{}", any.render_text());
        assert_eq!(any.edges[0].operations.len(), 2, "the tool shape is one of the two every edge carries");
        let as_unit = hosted("tool", &rule("worker", "tools/call", "tool:optimize"));
        assert!(as_unit.exit_code() == 0 && has(&as_unit, "node-decided edge"), "PASSED SILENTLY: {}", as_unit.render_text());
        for other in [rule("*", "cap.invoke", "cap:tool/optimize"), rule("*", "skill.invoke", "skill:tool/optimize")] {
            let r = hosted("tool", &other);
            assert!(has(&r, "unauthorisable edge"), "{other}: {}", r.render_text());
        }
    }

    /// **A rule that names the unit where the provider reads the node is a warning, never a silent pass.** The
    /// provider decides a member's direct call as `node:{sender}` — the calling node, not the unit it runs — so a
    /// `cap.invoke` rule whose actor is the requirer's unit principal, or one whose required mandate that principal
    /// holds, admits the edge only if the provider read the unit: `node-decided edge`, saying that a provider as
    /// built today refuses the call. Its severity is [`NODE_DECIDED`]'s, in one place. Seen failing first: on the code
    /// before 2026-11-18 the first declaration was `unauthorisable edge` with no word about the actor, and with the
    /// actor check planted to accept the unit principal for the hosted shape it passed with no finding.
    #[test]
    fn a_unit_principal_rule_on_a_hosted_edge_warns_that_the_provider_decides_the_node() {
        let as_unit = hosted("route", "[[rule]]\nactor=\"worker\"\noperation=\"cap.invoke\"\nresource=\"cap:route/optimize\"\n");
        assert_eq!(as_unit.exit_code(), 0, "a warning: {}", as_unit.render_text());
        assert!(!has(&as_unit, "unauthorisable edge"), "{}", as_unit.render_text());
        let w = as_unit.findings.iter().find(|f| f.kind == "node-decided edge").unwrap_or_else(|| panic!("PASSED SILENTLY: {}", as_unit.render_text()));
        assert_eq!((w.kind.as_str(), w.severity, w.unit.as_deref()), (NODE_DECIDED.0, NODE_DECIDED.1, Some("worker")));
        assert!(w.message.contains("requirement route/optimize") && w.message.contains("node:{sender}") && w.message.contains("\"worker\""), "{}", w.message);
        assert!(w.message.contains("built from `[authority]` refuses this call"), "the message says which provider it speaks for, and what it does: {}", w.message);

        // The same for a mandate: the rule is for any node, the mandate it requires is held by the unit principal.
        let unit_held = hosted(
            "route",
            concat!(
                "[[rule]]\nactor=\"*\"\noperation=\"cap.invoke\"\nresource=\"cap:route/optimize\"\nrequires_mandate=\"routing\"\n",
                "[[mandate]]\nholder=\"worker\"\nscope=\"routing\"\noperations=[\"cap.invoke:cap:route/optimize\"]\n",
            ),
        );
        assert!(unit_held.exit_code() == 0 && has(&unit_held, "node-decided edge") && !has(&unit_held, "unauthorisable edge"), "{}", unit_held.render_text());

        // A rule that admits any node beside it: the edge passes, and nothing warns.
        let both = hosted(
            "route",
            concat!(
                "[[rule]]\nactor=\"worker\"\noperation=\"cap.invoke\"\nresource=\"cap:route/optimize\"\n",
                "[[rule]]\nactor=\"*\"\noperation=\"cap.invoke\"\nresource=\"cap:route/optimize\"\n",
            ),
        );
        assert!(both.exit_code() == 0 && !has(&both, "node-decided edge"), "{}", both.render_text());
        // Another unit's principal is nobody's reading: an error, as any rule that names someone else.
        let other = hosted("route", "[[rule]]\nactor=\"auditor\"\noperation=\"cap.invoke\"\nresource=\"cap:route/optimize\"\n");
        assert!(has(&other, "unauthorisable edge") && !has(&other, "node-decided edge"), "{}", other.render_text());
    }

    /// **A hosted edge's rule is read where its provider reads it** (#814's review, F1). `[authority]` builds a
    /// node's evaluator from its own unit's rules, and `Provider::Artifact::hosts` names the units that could host the
    /// component: only a rule one of them declares admits. A rule in the requirer, or in a third unit, is
    /// `unauthorisable edge`, naming where the rule is and where its provider reads it. Where the hosts differ — one
    /// declares the rule, one does not — the edge is `host-dependent edge`, a warning: which host installs the
    /// component is decided at run time, and the call is admitted at one and refused at the other. The provider's
    /// half is `a_hosted_edges_rule_admits_only_from_a_unit_that_hosts_it`. Seen failing first: on the overlay at
    /// #814's first head (rules gathered from every unit) each of these passed with no finding.
    #[test]
    fn a_hosted_edges_rule_admits_only_from_a_host_unit() {
        let rule = "[[rule]]\nactor=\"*\"\noperation=\"cap.invoke\"\nresource=\"cap:route/optimize\"\n";
        let in_requirer = hosted_in("route", "", rule);
        let f = in_requirer.findings.iter().find(|f| f.kind == "unauthorisable edge").unwrap_or_else(|| panic!("FALSE GREEN: no host loads the worker's rule: {}", in_requirer.render_text()));
        assert!(f.message.contains("`worker`") && f.message.contains("depot"), "where the rule is, and where its provider reads it: {}", f.message);

        let art = ("optimizer".to_string(), ArtifactDescription::from_toml_str("kind=\"wasm-component\"\n[provides]\nns=\"route\"\nname=\"optimize\"\n").unwrap());
        let host = |name: &str, rules: &str| unit(name, &format!("principal=\"{name}\"\n[hosts]\nkinds=[\"wasm-component\"]\naccept_unsigned=true\n{rules}"));
        let worker = unit("worker", "principal=\"worker\"\n[[requirement]]\nns=\"route\"\nname=\"optimize\"\n");
        let auditor = unit("auditor", &format!("principal=\"auditor\"\n{rule}"));
        // The hosts declare vocabulary of their own — another capability's rule — and not this one: an error.
        let other = "[[rule]]\nactor=\"*\"\noperation=\"skill.invoke\"\nresource=\"skill:depot/intake\"\n";
        let third = check(&[auditor.clone(), host("depot-a", other), host("depot-b", other), worker.clone()], std::slice::from_ref(&art), &CheckOptions::default());
        let f = third.findings.iter().find(|f| f.kind == "unauthorisable edge").unwrap_or_else(|| panic!("FALSE GREEN: no depot loads the auditor's rule: {}", third.render_text()));
        assert!(f.message.contains("`auditor`") && f.message.contains("depot-a|depot-b"), "{}", f.message);
        // The hosts and the requirer declare nothing (#814's confirm round, C2): whether they enforce at all is in
        // their nodes' configuration, not in a unit file, so the edge is `ungoverned edge`, a warning — which says
        // that the rule another unit declares for it is one the hosts do not read.
        let outside = check(&[auditor, host("depot-a", ""), host("depot-b", ""), worker.clone()], std::slice::from_ref(&art), &CheckOptions::default());
        assert!(outside.exit_code() == 0 && !has(&outside, "unauthorisable edge"), "FALSE RED: no host declares any vocabulary: {}", outside.render_text());
        let f = outside.findings.iter().find(|f| f.kind == "ungoverned edge" && f.message.contains("route/optimize")).unwrap_or_else(|| panic!("PASSED SILENTLY: {}", outside.render_text()));
        assert!(f.message.contains("`auditor`") && f.message.contains("depot-a|depot-b") && f.message.contains("[authority]"), "{}", f.message);
        // …and an unrelated rule elsewhere names nothing for this edge.
        let unrelated = unit("ledger", "principal=\"ledger\"\n[[rule]]\nactor=\"token:fleet/treasurer\"\noperation=\"grant.issue\"\nresource=\"tool:ledger\"\n");
        let outside = check(&[unrelated, host("depot-a", ""), host("depot-b", ""), worker.clone()], std::slice::from_ref(&art), &CheckOptions::default());
        assert!(outside.exit_code() == 0 && !has(&outside, "unauthorisable edge"), "FALSE RED: {}", outside.render_text());
        let f = outside.findings.iter().find(|f| f.kind == "ungoverned edge" && f.message.contains("route/optimize")).expect("the edge is named as outside the vocabulary");
        assert!(!f.message.contains("`ledger`"), "{}", f.message);

        let differ = check(&[host("depot-a", ""), host("depot-b", rule), worker.clone()], std::slice::from_ref(&art), &CheckOptions::default());
        assert_eq!(differ.exit_code(), 0, "{}", differ.render_text());
        let f = differ.findings.iter().find(|f| f.kind == "host-dependent edge").unwrap_or_else(|| panic!("PASSED SILENTLY: depot-a would refuse: {}", differ.render_text()));
        assert_eq!((f.kind.as_str(), f.severity), HOST_DEPENDENT);
        assert!(f.message.contains("depot-a") && f.message.contains("depot-b"), "{}", f.message);

        let both = check(&[host("depot-a", rule), host("depot-b", rule), worker], std::slice::from_ref(&art), &CheckOptions::default());
        assert!(both.findings.iter().all(|f| !f.kind.ends_with(" edge")), "{}", both.render_text());
    }

    /// **A rule form no provider admits a member's call under does not pass** (#814's review, F2). What a provider
    /// built from the declaration does with each rule field is fixed by the door and the declaration alone, for a
    /// member's direct call — so the check reads each as the provider does, and the finding names the reason:
    ///
    /// | Field | At the hosted door, for a member's direct call | The check |
    /// |---|---|---|
    /// | `actor` | matched exactly; the caller is `node:{sender}` | `*`, or `node:` and a node id in its canonical spelling; `node:*`, `node:` and the like name no node |
    /// | `operation` | matched exactly, or `*` | the same |
    /// | `resource` | matched exactly at `…@{node}`, or `*`; `[authority]` qualifies a bare one | `*`, bare, or `@` and a node id; `…@` and the like name no node |
    /// | `requires_scopes` | the rule applies only to a caller holding them; a member's call carries none | the rule does not apply |
    /// | `signed_with` | the rule applies only to a signed grant decision | the rule does not apply |
    /// | `cancellable` | refused: the door cannot stop a begun call (`door_cannot_stop`) | the rule refuses |
    /// | `requires_facts` | *indeterminate*: the reference evaluator cannot establish a fact | the rule refuses |
    /// | `requires_mandate` | *indeterminate* without one; `[authority]` entitles no authority to a scope | `mandate-dependent edge` with a node-held declared mandate, `node-decided edge` with a unit-held one, else refuses |
    /// | `requires_grant`, `requires_values` | decided on a grant, or on the call's arguments: run-time facts | not read: the rule is taken to permit |
    /// | `exposure` | never loaded as an allowance | not a rule |
    ///
    /// And the **order**: the evaluator decides by the first allowance that matches, so a rule that matches a
    /// member's call and does not permit it shadows every later rule it covers. The provider's half, form by form, is
    /// `a_rule_form_no_provider_admits_a_members_call_under_is_not_passed`. Seen failing first: on the overlay at
    /// #814's first head every refusing form below passed with no finding.
    #[test]
    fn a_rule_form_no_provider_admits_a_members_call_under_is_unauthorisable() {
        let base = "[[rule]]\nactor=\"*\"\noperation=\"cap.invoke\"\nresource=\"cap:route/optimize\"\n";
        let refused: [(String, &str); 12] = [
            (format!("{base}cancellable=true\n"), "cannot stop"),
            (format!("{base}requires_scopes=[\"routing:write\"]\n"), "carries no scopes"),
            (base.replace("\"*\"", "\"node:*\""), "names no node"),
            (base.replace("\"*\"", "\"node:\""), "names no node"),
            (base.replace("\"*\"", "\"node:depot-a\""), "names no node"),
            (base.replace("\"*\"", "\"node:10.0.0.7\""), "names no node"),
            (base.replace("optimize\"", "optimize@\""), "names no node"),
            (base.replace("optimize\"", "optimize@depot\""), "names no node"),
            (format!("{base}signed_with=[\"service\"]\n"), "signed grant decision"),
            (format!("{base}requires_facts=[\"budget\"]\n"), "cannot establish"),
            (format!("{base}requires_mandate=\"routing\"\n"), "requires mandate scope \"routing\""),
            (format!("{base}cancellable=true\n{base}"), "first"),
        ];
        for (rules, why) in refused {
            let r = hosted("route", &rules);
            let f = r.findings.iter().find(|f| f.kind == "unauthorisable edge").unwrap_or_else(|| panic!("FALSE GREEN: {rules}\n{}", r.render_text()));
            assert!(f.message.contains(why), "{rules}: the reason is named ({why}): {}", f.message);
            assert_eq!(r.exit_code(), 1);
        }
        // The plain rule first is the one that decides; a shadowing rule that names another node shadows only it.
        for rules in [format!("{base}{base}cancellable=true\n"), format!("{}{base}", base.replace("\"*\"", "\"node:10.0.0.7:7946\"") + "cancellable=true\n")] {
            let r = hosted("route", &rules);
            assert!(r.findings.iter().all(|f| !f.kind.ends_with(" edge")), "{rules}: {}", r.render_text());
        }
        // A rule that names a value does not stand in a later rule's way (#814's confirm round, C1): the evaluator
        // skips a rule whose named value the call carries with another value, so whether it matches is a run-time
        // fact, and the plain rule after it can be the first that matches. Seen failing first: `unauthorisable edge`,
        // "never reached", for a call the provider runs.
        for refusing in ["cancellable=true\n", "requires_facts=[\"budget\"]\n"] {
            let r = hosted("route", &format!("{base}{refusing}[rule.requires_values]\nstops=99\n{base}"));
            assert!(r.exit_code() == 0 && r.findings.iter().all(|f| !f.kind.ends_with(" edge")), "FALSE RED: a rule naming a value is no blocker: {}", r.render_text());
        }
        // A mandate a node holds: not a pass, and not an error — `[authority]` entitles no authority to the scope.
        let node_held = hosted("route", &format!("{base}requires_mandate=\"routing\"\n[[mandate]]\nholder=\"node:10.0.0.7:7946\"\nscope=\"routing\"\noperations=[\"cap.invoke:cap:route/optimize\"]\n"));
        let f = node_held.findings.iter().find(|f| f.kind == "mandate-dependent edge").unwrap_or_else(|| panic!("PASSED SILENTLY: {}", node_held.render_text()));
        assert_eq!((f.kind.as_str(), f.severity), MANDATE_DEPENDENT);
        assert!(node_held.exit_code() == 0 && !has(&node_held, "unauthorisable edge"), "{}", node_held.render_text());
    }

    /// **A `blob` beside the component does not bring the skill reading back** (#814's review, F10). Two artifacts
    /// could provide the capability, a component and a blob; nothing says how a blob-provided capability is called,
    /// and the component's provider decides `cap.invoke`. A skill rule is `unauthorisable edge`. Seen failing first:
    /// on the overlay at #814's first head the blob made the edge "deployed" and the skill rule passed it.
    #[test]
    fn a_blob_beside_a_component_does_not_admit_by_a_skill_rule() {
        let component = ("optimizer".to_string(), ArtifactDescription::from_toml_str("kind=\"wasm-component\"\n[provides]\nns=\"route\"\nname=\"optimize\"\n").unwrap());
        let blob = ("weights".to_string(), ArtifactDescription::from_toml_str("kind=\"blob\"\n[provides]\nns=\"route\"\nname=\"optimize\"\n").unwrap());
        let depot = unit("depot", "principal=\"depot\"\n[hosts]\nkinds=[\"wasm-component\",\"blob\"]\naccept_unsigned=true\n[[rule]]\nactor=\"worker\"\noperation=\"skill.invoke\"\nresource=\"skill:route/optimize\"\n");
        let worker = unit("worker", "principal=\"worker\"\n[[requirement]]\nns=\"route\"\nname=\"optimize\"\n[[mandate]]\nholder=\"worker\"\nscope=\"s\"\noperations=[\"x\"]\n");
        let r = check(&[depot, worker], &[component, blob], &CheckOptions::default());
        assert_eq!(r.edges[0].providers.len(), 2, "both artifacts would provide it: {:?}", r.edges[0].providers);
        assert!(has(&r, "unauthorisable edge"), "FALSE GREEN: {}", r.render_text());
    }

    /// The hosted shape is the provider's own derivation, not a restatement that can drift: for a kind outside the
    /// `tool` namespace `derive_call` answers the shape's operation and its resource at the node; for the `tool`
    /// namespace there is no hosted shape, and `derive_call` answers the tool shape every edge already carries.
    /// Seen to bite: with `hosted_call_shape`'s resource planted to `cap:{name}` the first assertion failed.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    #[test]
    fn the_hosted_shape_is_what_the_provider_derives() {
        let derive = |ns: &str, name: &str| {
            let kind = format!("{}{ns}/{name}", crate::HOSTED_CAPABILITY_KIND_PREFIX);
            let (operation, resource, _) = crate::agent::provider_enforcement::derive_call(&kind, b"{}", None, "10.0.0.2:7946").unwrap_or_else(|_| panic!("{kind} derives"));
            (operation, resource)
        };
        let (operation, resource) = hosted_call_shape("route", "optimize").expect("outside the tool namespace");
        assert_eq!(derive("route", "optimize"), (operation.clone(), format!("{resource}@10.0.0.2:7946")));
        assert_eq!(operation, crate::HOSTED_CAPABILITY_OPERATION);
        assert_eq!(hosted_call_shape("tool", "optimize"), None);
        let [_, (tool_op, tool_res)] = call_shapes("tool", "optimize");
        assert_eq!(derive("tool", "optimize"), (tool_op, format!("{tool_res}@10.0.0.2:7946")));
    }

    #[test]
    fn an_artifact_description_with_an_unknown_kind_is_refused() {
        let e = ArtifactDescription::from_toml_str("kind=\"native\"\n[provides]\nns=\"a\"\nname=\"b\"\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown artifact kind") && e.contains("native"), "{e}");
    }

    /// Two `[[presence]]` bands over providers both can match, at least one with a ceiling, trade a provider back and
    /// forth — the capped one sheds against its ceiling while the other's floor (or its own ceiling) brings one back.
    /// Named at design time; not named: bands provably disjoint (an `eq` on one attribute to different values, or
    /// different `schema_id`s), one band however it is spelled (the runtime's identity: ranking ignored, `x = "v"` and
    /// `x = { eq = "v" }` alike), a floor and a ceiling over the same population with the floor at or below it, and two
    /// floors (#560's review).
    #[test]
    fn presence_bands_that_overlap_with_ceilings_are_named() {
        let host = unit("depot", "[[capability]]\nns=\"route\"\nname=\"optimize\"\n[capability.attrs]\nregion=\"eu\"\n");
        let p = |name: &str, body: &str| unit(name, &format!("[[presence]]\nns=\"route\"\nname=\"optimize\"\n{body}"));
        let overlap = |units: &[Unit]| {
            let r = check(units, &[], &CheckOptions::default());
            kinds(&r).iter().filter(|k| **k == ("presence bands overlap", Severity::Warning)).count()
        };
        let any3 = p("a", "min_providers=1\nmax_providers=3\n");
        let eu2 = p("b", "min_providers=1\nmax_providers=2\n[presence.attrs]\nregion=\"eu\"\n");
        let us2 = p("c", "min_providers=1\nmax_providers=2\n[presence.attrs]\nregion=\"us\"\n");
        let eu_floor3 = p("d", "min_providers=3\n[presence.attrs]\nregion=\"eu\"\n");
        assert_eq!(overlap(&[host.clone(), any3.clone(), eu2.clone()]), 1, "an unattributed band and an `eu` band, both capped");
        assert_eq!(overlap(&[host.clone(), eu2.clone(), us2.clone()]), 0, "`eu` and `us` cannot share a provider");
        assert_eq!(overlap(&[host.clone(), eu2.clone(), p("e", "min_providers=1\nmax_providers=2\n[presence.attrs]\nregion={ eq = \"eu\" }\n[presence.ranking]\nattribute=\"load\"\norder=\"ascending\"\n")]), 0,
                   "one band, spelled and ranked differently");
        assert_eq!(overlap(&[host.clone(), any3.clone(), eu_floor3.clone()]), 1, "an `eu` floor against an unattributed ceiling");
        assert_eq!(overlap(&[host.clone(), eu2.clone(), eu_floor3]), 1, "a floor above a ceiling over one population");
        assert_eq!(overlap(&[host.clone(), eu2.clone(), p("f", "min_providers=2\n[presence.attrs]\nregion=\"eu\"\n")]), 0,
                   "a floor at the ceiling over one population");
        assert_eq!(overlap(&[host.clone(), p("g", "min_providers=1\n"), p("h", "min_providers=2\n[presence.attrs]\nregion=\"eu\"\n")]), 0, "two floors");
        assert_eq!(overlap(&[host.clone(), p("i", "min_providers=1\nmax_providers=2\nschema_id=\"v1\"\n"),
                             p("j", "min_providers=1\nmax_providers=2\nschema_id=\"v2\"\n")]), 0, "different schemas");
    }
}
