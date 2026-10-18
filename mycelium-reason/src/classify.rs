//! **Data classification for routed inference** (post-360 row R — inference routing as policy).
//!
//! A [`DataClassPolicy`] is the operator's vocabulary of data classes, ordered from least to most
//! restrictive, and the rules for deciding which one a request is:
//!
//! 1. **A floor bound to the caller's credential.** [`DataClassPolicy::floor_for`] binds a class to a
//!    verified principal (the gateway auth layer's `token:{issuer}/{name}`, `oidc:{issuer}/{subject}`,
//!    …); [`DataClassPolicy::floor_for_all`] binds one to every caller. A request from that principal
//!    is **at least** that class: a client may raise it, **never lower it**.
//! 2. **The default, as a floor.** A caller with no bound floor has the default class — the **most
//!    restrictive** unless [`DataClassPolicy::unclassified_as`] says otherwise — as its floor, so it
//!    cannot declare itself below it. Only an explicitly bound floor can sit lower (a principal bound to
//!    `public` may send `public`). Until the round-1 review the default applied only to a request that
//!    declared nothing, so any caller, anonymous included, could declare `public` and lower it.
//! 3. **A class the client declares** — the `x-mycelium-data-class` header on the façade's two doors
//!    ([`DATA_CLASS_HEADER`]), given once. Taken when it is at or above the floor; below it, the floor
//!    applies and the record says where the floor came from. A declaration naming a class the policy
//!    does not have is refused, never guessed at.
//!
//! **What is verified and what is not.** The principal is the gateway's, so a floor is as strong as
//! the credential it is bound to. The declared class is the client's assertion and is used only to
//! *raise*. Nothing inspects the content: the class is **declared, never detected**, so a client that
//! sends confidential text marked `public` — with no floor above `public` — routes it as public.

use std::collections::HashMap;
use std::fmt;

use mycelium::{ClassSource, DataClass};

/// The request header a client declares its data class in, on `/gateway/reason/route` and
/// `/gateway/reason/v1/chat/completions`. OpenAI clients set it as a default header.
pub const DATA_CLASS_HEADER: &str = "x-mycelium-data-class";

/// The principal key [`DataClassPolicy::floor_for_all`] binds — a floor for every caller.
const EVERY_CALLER: &str = "*";

/// The operator's data classes, least to most restrictive, and how a request is classified (module doc).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataClassPolicy {
    classes: Vec<String>,
    unclassified: usize,
    floors: HashMap<String, usize>,
}

/// Why a classification policy could not be built, or a request could not be classified.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClassError {
    /// A policy needs at least one class.
    NoClasses,
    /// A class name is empty, or not printable ASCII without spaces.
    BadName(String),
    /// The same class named twice.
    Duplicate(String),
    /// A class the policy does not have — in a declaration, a floor or the default.
    Unknown(String),
    /// A class was declared, but this node has no classification policy, so it cannot be honoured.
    NotConfigured(String),
}

impl fmt::Display for ClassError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClassError::NoClasses => write!(f, "a data classification needs at least one class"),
            ClassError::BadName(c) => write!(f, "{c:?} is not a class name (printable ASCII, no spaces)"),
            ClassError::Duplicate(c) => write!(f, "class {c:?} is named twice"),
            ClassError::Unknown(c) => write!(f, "{c:?} is not a data class this node knows"),
            ClassError::NotConfigured(c) => write!(
                f,
                "class {c:?} was declared, but this node has no data classification configured, so it cannot be honoured"
            ),
        }
    }
}

impl std::error::Error for ClassError {}

impl DataClassPolicy {
    /// Classes ordered **least to most restrictive** — e.g. `["public", "internal", "confidential"]`.
    /// An unclassified request is the last (most restrictive) until
    /// [`unclassified_as`](Self::unclassified_as) says otherwise.
    pub fn new<I, S>(classes: I) -> Result<Self, ClassError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let classes: Vec<String> = classes.into_iter().map(Into::into).collect();
        if classes.is_empty() {
            return Err(ClassError::NoClasses);
        }
        for (i, c) in classes.iter().enumerate() {
            if c.is_empty() || !c.bytes().all(|b| b.is_ascii_graphic()) {
                return Err(ClassError::BadName(c.clone()));
            }
            if classes[..i].contains(c) {
                return Err(ClassError::Duplicate(c.clone()));
            }
        }
        let unclassified = classes.len() - 1;
        Ok(Self { classes, unclassified, floors: HashMap::new() })
    }

    /// The class an unclassified request is. Default: the most restrictive.
    pub fn unclassified_as(mut self, class: &str) -> Result<Self, ClassError> {
        self.unclassified = self.rank(class)?;
        Ok(self)
    }

    /// A request from `principal` (the gateway's verified principal) is **at least** `class`.
    pub fn floor_for(mut self, principal: impl Into<String>, class: &str) -> Result<Self, ClassError> {
        let rank = self.rank(class)?;
        self.floors.insert(principal.into(), rank);
        Ok(self)
    }

    /// Every request is **at least** `class`, whoever sends it — so no declaration goes below it.
    pub fn floor_for_all(self, class: &str) -> Result<Self, ClassError> {
        self.floor_for(EVERY_CALLER, class)
    }

    /// The classes, least to most restrictive.
    pub fn classes(&self) -> &[String] {
        &self.classes
    }

    fn rank(&self, class: &str) -> Result<usize, ClassError> {
        self.classes.iter().position(|c| c == class).ok_or_else(|| ClassError::Unknown(class.to_string()))
    }

    /// Classify one request from `principal` (`None`: anonymous) that declared `declared`.
    pub fn classify(&self, principal: Option<&str>, declared: Option<&str>) -> Result<DataClass, ClassError> {
        let declared = declared.map(|d| self.rank(d)).transpose()?;
        let bound = [principal, Some(EVERY_CALLER)]
            .into_iter()
            .flatten()
            .filter_map(|p| self.floors.get(p).copied())
            .max();
        // No bound floor: the default is the floor (a declaration may raise it, never lower it).
        let (floor, floor_source) = match bound {
            Some(f) => (f, ClassSource::Principal),
            None => (self.unclassified, ClassSource::Default),
        };
        let (rank, source) = match declared {
            Some(d) if d >= floor => (d, ClassSource::Declared),
            _ => (floor, floor_source),
        };
        Ok(DataClass::new(self.classes[rank].clone(), source))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn three() -> DataClassPolicy {
        DataClassPolicy::new(["public", "internal", "confidential"]).unwrap()
    }

    /// An unclassified request is the most restrictive class unless the operator says otherwise.
    #[test]
    fn an_unclassified_request_is_the_most_restrictive_class() {
        let c = three().classify(Some("token:gw/alice"), None).unwrap();
        assert_eq!((c.class.as_str(), c.source), ("confidential", ClassSource::Default));
        let c = three().unclassified_as("internal").unwrap().classify(None, None).unwrap();
        assert_eq!((c.class.as_str(), c.source), ("internal", ClassSource::Default));
    }

    /// The round-1 review's finding 3: with no bound floor the default is the floor, so a caller —
    /// anonymous included — cannot declare itself below it. Seen failing first: `public` was taken.
    #[test]
    fn the_default_is_a_floor_a_declaration_cannot_lower() {
        let c = three().classify(None, Some("public")).unwrap();
        assert_eq!((c.class.as_str(), c.source), ("confidential", ClassSource::Default));
        let c = three().classify(Some("token:gw/anyone"), Some("internal")).unwrap();
        assert_eq!(c.class, "confidential");
        let p = three().unclassified_as("internal").unwrap();
        assert_eq!(p.classify(None, Some("public")).unwrap().class, "internal");
        let c = p.classify(None, Some("confidential")).unwrap();
        assert_eq!((c.class.as_str(), c.source), ("confidential", ClassSource::Declared), "raised");
        // Only an explicitly bound floor sits lower than the default.
        let p = three().floor_for("token:gw/web", "public").unwrap();
        let c = p.classify(Some("token:gw/web"), Some("public")).unwrap();
        assert_eq!((c.class.as_str(), c.source), ("public", ClassSource::Declared));
        let c = p.classify(Some("token:gw/web"), None).unwrap();
        assert_eq!((c.class.as_str(), c.source), ("public", ClassSource::Principal));
        assert_eq!(p.classify(Some("token:gw/other"), Some("public")).unwrap().class, "confidential");
    }

    /// A client may raise the class its credential carries and never lower it.
    #[test]
    fn a_declaration_raises_a_credential_floor_and_never_lowers_it() {
        let p = three().floor_for("token:gw/hr", "confidential").unwrap();
        let c = p.classify(Some("token:gw/hr"), Some("public")).unwrap();
        assert_eq!((c.class.as_str(), c.source), ("confidential", ClassSource::Principal), "not lowered");
        let p = three().floor_for("token:gw/eng", "internal").unwrap();
        let c = p.classify(Some("token:gw/eng"), Some("confidential")).unwrap();
        assert_eq!((c.class.as_str(), c.source), ("confidential", ClassSource::Declared), "raised");
        let c = p.classify(Some("token:gw/eng"), None).unwrap();
        assert_eq!((c.class.as_str(), c.source), ("internal", ClassSource::Principal));
        // Another principal is not bound by eng's floor — the default floor applies to it.
        let c = p.classify(Some("token:gw/web"), Some("public")).unwrap();
        assert_eq!((c.class.as_str(), c.source), ("confidential", ClassSource::Default));
    }

    #[test]
    fn a_floor_for_every_caller_holds_for_anonymous_too() {
        let p = three().floor_for_all("internal").unwrap();
        for who in [None, Some("token:gw/anyone")] {
            let c = p.classify(who, Some("public")).unwrap();
            assert_eq!(c.class, "internal", "{who:?}");
            assert_eq!(p.classify(who, None).unwrap().class, "internal", "a bound floor replaces the default");
        }
    }

    #[test]
    fn an_unknown_class_is_refused_never_guessed() {
        assert_eq!(three().classify(None, Some("secret")), Err(ClassError::Unknown("secret".into())));
        assert_eq!(three().classify(None, Some("Public")), Err(ClassError::Unknown("Public".into())));
        assert!(matches!(three().floor_for("x", "nope"), Err(ClassError::Unknown(_))));
        assert!(matches!(three().unclassified_as("nope"), Err(ClassError::Unknown(_))));
    }

    #[test]
    fn a_policy_is_well_formed() {
        assert_eq!(DataClassPolicy::new(Vec::<String>::new()), Err(ClassError::NoClasses));
        assert!(matches!(DataClassPolicy::new(["a", "a"]), Err(ClassError::Duplicate(_))));
        assert!(matches!(DataClassPolicy::new(["a b"]), Err(ClassError::BadName(_))));
        assert!(matches!(DataClassPolicy::new([""]), Err(ClassError::BadName(_))));
    }
}
