//! **Where a routed action would go, and what it carries** (post-360 row R — inference routing as
//! policy).
//!
//! A router — `mycelium-reason`'s `InferenceRouter` is the first — picks a destination for a call
//! among several that could serve it. Before row R the action evaluator saw the chosen resource
//! (`prompt:llm/{model}@{provider}`) and nothing about it: not whether the provider's inference runs
//! inside this domain or at an endpoint off the mesh, and not what kind of data the request carries.
//! So nothing could decide that a *confidential* prompt may only go to an in-domain model.
//!
//! These types are the facts a routing enforcement point establishes and hands to the gateway's
//! preflight ([`GossipAgent::gateway_preflight_routed`](crate::GossipAgent::gateway_preflight_routed)):
//! the [`Destination`] it is about to dispatch to with its [`Locality`], the request's [`DataClass`]
//! and where that class came from, and the destinations it has already been refused for this call.
//! The envelope carries the first two to the evaluator; the evidence record carries all three.
//!
//! **What is and is not verified.** The locality is the *provider's declaration* (for
//! `mycelium-reason`, the `locality` attribute of its `llm-meta/{model}` ad) — not observed: nothing
//! here watches where a provider's backend actually sends a prompt. Nor is it bound to its writer:
//! a capability ad's key names the node, and nothing ties the key to the member that wrote it, so a
//! member of the fleet can write another node's ad. A gateway *client* cannot — the two gateway
//! doors that write ads for clients refuse the `llm-meta` namespace (`capability/advertise`,
//! `units/declare`), and the provisioner never installs an entry providing one. A provider that declares nothing is [`Locality::Unknown`], which a rule about
//! locality reads as *not established*, never as in-domain. The class is **declared, not detected**:
//! no content is inspected. Its [`ClassSource`] says who declared it, so a reader can tell a class
//! the operator bound to the caller's credential from one the client asserted.
//!
//! Available in every gateway build — a companion's door names its route in a build without `tls`,
//! where the preflight is inert and these facts are simply not consulted.

use serde::{Deserialize, Serialize};

/// Where a destination's work happens, relative to this domain.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Locality {
    /// Inside the domain: the work runs on a node of this fleet, with an engine of its own.
    InDomain,
    /// Outside the domain: the work leaves the mesh — e.g. a provider whose backend is an
    /// OpenAI-compatible endpoint off the mesh.
    External,
    /// Not established — nothing declared it. A rule about locality reads this as *not established*
    /// (`Indeterminate`), never as either of the other two.
    Unknown,
}

impl Locality {
    /// The wire and label form: `in_domain`, `external`, `unknown`.
    pub fn label(&self) -> &'static str {
        match self {
            Locality::InDomain => "in_domain",
            Locality::External => "external",
            Locality::Unknown => "unknown",
        }
    }

    /// Read a declared locality. Anything but `in_domain` or `external` — a typo, an empty string,
    /// a value from a later vocabulary — is [`Locality::Unknown`]: an unreadable declaration is not
    /// a declaration of either.
    pub fn parse(s: &str) -> Self {
        match s {
            "in_domain" => Locality::InDomain,
            "external" => Locality::External,
            _ => Locality::Unknown,
        }
    }
}

impl std::fmt::Display for Locality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// One destination a router could dispatch to: its name, and where its work happens.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Destination {
    /// What the destination is called in evidence — for a routed model, `llm/{model}@{provider}`.
    pub name: String,
    /// Where its work happens, as established (see the module doc for how).
    pub locality: Locality,
}

impl Destination {
    /// A destination named `name` whose work happens at `locality`.
    pub fn new(name: impl Into<String>, locality: Locality) -> Self {
        Self { name: name.into(), locality }
    }
}

/// Who said what class a request's data is.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassSource {
    /// The operator bound this class (as a floor) to the caller's verified principal. A client
    /// cannot lower it.
    Principal,
    /// The client declared it, at or above any floor its principal carries.
    Declared,
    /// Nothing classified the request, so the configured default for unclassified requests applied.
    Default,
}

impl ClassSource {
    /// The wire and label form: `principal`, `declared`, `default`.
    pub fn label(&self) -> &'static str {
        match self {
            ClassSource::Principal => "principal",
            ClassSource::Declared => "declared",
            ClassSource::Default => "default",
        }
    }
}

/// The class of the data a request carries — declared, never detected — and who declared it.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataClass {
    /// The class's name in the operator's vocabulary (`public`, `confidential`, …).
    pub class: String,
    /// Who said so.
    pub source: ClassSource,
}

impl DataClass {
    /// The class `class`, from `source`.
    pub fn new(class: impl Into<String>, source: ClassSource) -> Self {
        Self { class: class.into(), source }
    }
}

/// A destination this call was refused for before the one it is now deciding on.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefusedDestination {
    /// The destination.
    pub destination: Destination,
    /// The refusal's machine-readable reason (`action_denied`, `authority_not_established`).
    pub reason: String,
}

impl RefusedDestination {
    /// `destination`, refused for `reason`.
    pub fn new(destination: Destination, reason: impl Into<String>) -> Self {
        Self { destination, reason: reason.into() }
    }
}

/// What a routing enforcement point established about one routed decision: the destination it is
/// deciding on, the request's class, and the destinations already refused for this call.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteFacts {
    /// The destination being decided on.
    pub destination: Destination,
    /// The request's class, when the enforcement point classifies; `None` when it does not, which a
    /// rule about class reads as *not established*.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_class: Option<DataClass>,
    /// The destinations this call was refused for before this one, in the order they were tried.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refused: Vec<RefusedDestination>,
}

impl RouteFacts {
    /// The facts for a decision on `destination`, unclassified, with nothing refused before it.
    pub fn new(destination: Destination) -> Self {
        Self { destination, data_class: None, refused: Vec::new() }
    }

    /// The request's class.
    pub fn with_data_class(mut self, class: DataClass) -> Self {
        self.data_class = Some(class);
        self
    }

    /// The destinations already refused for this call.
    pub fn with_refused(mut self, refused: Vec<RefusedDestination>) -> Self {
        self.refused = refused;
        self
    }
}
