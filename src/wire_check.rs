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
//! | `group requires unmet` | a group's `requires` filter binds to nothing | error |
//! | `orphan lane` | a lane consumed and never produced, or produced and never consumed | error |
//! | `unhostable entry` | an artifact would satisfy the filter but no unit's `[hosts]` names its kind with budget for its footprint | error |
//! | `presence unhostable` | a `[[presence]]` floor cannot be met by deployed providers plus distinct hosting units | error |
//! | `would bind by provisioning` | no deployed provider, but a hostable artifact matches | warning (error with `strict_deployed`) |
//! | `unranked ranking` | a ranking on an attribute no matching capability carries | warning |
//! | `single provider` | a requirement with exactly one possible provider | warning |
//!
//! The JSON output is a versioned document, [`DECLARATION_SCHEMA`], with the `revision` the caller
//! supplies (the git commit of the units directory) so a consumer can join it to runtime records
//! by `(principal, ns, name, schema_id)` — the plan's §9. A change to its shape is a schema version.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::capability::{CapConstraint, CapFilter, CapValue, Capability};
use crate::capability_config::{CapDecl, LaneRole, NodeCapabilityConfig};
use crate::error::GossipError;

/// The JSON document's schema id. Bump it when the shape changes; a consumer pins it.
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
#[derive(Debug, Clone, Default)]
pub struct CheckOptions {
    /// Treat *would bind by provisioning* as an error: the deployment must be wired by what is
    /// deployed, not by what could be installed.
    pub strict_deployed: bool,
    /// The revision to stamp on the document (the git commit of the units directory), if known.
    pub revision: Option<String>,
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
    Artifact { artifact: String, hosts: Vec<String> },
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
                        Provider::Artifact { artifact, hosts } => format!("artifact {artifact} on {}", hosts.join("|")),
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
                    providers.push(Provider::Artifact { artifact: aname.clone(), hosts });
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

    let resolver = Resolver { units, artifacts, offers: &offers, opts };

    // ── requirements ───────────────────────────────────────────────────────────────────
    let mut edges = Vec::new();
    for u in units {
        for r in &u.config.requirements {
            let Ok(filter) = r.to_filter() else { continue };
            let providers = resolver.resolve(&u.name, &filter, "requirement", &mut findings);
            edges.push(Edge {
                requirer:  u.name.clone(),
                principal: u.config.principal.clone(),
                ns:        r.ns.clone(),
                name:      r.name.clone(),
                schema_id: r.schema_id.clone(),
                providers,
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
        .map(|u| UnitSummary {
            name:        u.name.clone(),
            principal:   u.config.principal.clone(),
            offers:      u.config.capabilities.iter().map(|c| format!("{}/{}", c.ns, c.name)).collect(),
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
        })
        .collect();

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

#[cfg(test)]
mod tests {
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
        let small = unit("depot-small", "[hosts]\nkinds=[\"wasm-component\"]\ninstall_budget_bytes=1024\n");
        let r = check(&[worker.clone(), small], std::slice::from_ref(&art), &CheckOptions::default());
        assert!(kinds(&r).contains(&("unhostable entry", Severity::Error)), "{}", r.render_text());

        // two hosts with budget → would bind by provisioning (a warning), presence 2 of 2, exit 0
        let a = unit("depot-a", "[hosts]\nkinds=[\"wasm-component\",\"blob\"]\ninstall_budget_bytes=8589934592\n");
        let b = unit("depot-b", "[hosts]\nkinds=[\"wasm-component\"]\n");
        let r = check(&[worker.clone(), a.clone(), b.clone()], std::slice::from_ref(&art), &CheckOptions::default());
        assert_eq!(r.exit_code(), 0, "{}", r.render_text());
        assert!(kinds(&r).contains(&("would bind by provisioning", Severity::Warning)));
        assert!(matches!(&r.edges[0].providers[0], Provider::Artifact { hosts, .. } if hosts == &vec!["depot-a".to_string(), "depot-b".to_string()]));

        // strict: the same deployment is an error
        let r = check(&[worker, a, b], std::slice::from_ref(&art), &CheckOptions { strict_deployed: true, revision: Some("abc".into()) });
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

    #[test]
    fn an_artifact_description_with_an_unknown_kind_is_refused() {
        let e = ArtifactDescription::from_toml_str("kind=\"native\"\n[provides]\nns=\"a\"\nname=\"b\"\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown artifact kind") && e.contains("native"), "{e}");
    }
}
