//! Namespace confinement — the **enforcement point** of the host ⇄ component boundary.
//!
//! A WASM guest is untrusted foreign code running in the node's own process. Unlike the
//! substrate (which is detection-not-prevention, because Layer I must not police higher
//! *layers*), the host *mediates every import a guest makes* and can legitimately **prevent**:
//! a component addresses KV keys **relative to its own capability namespace**, and the host
//! prefixes them into a private, per-component subtree it can never escape.
//!
//! Component KV lives under [`COMPONENT_KV_PREFIX`]`{node}/{namespace}/…` — a dedicated prefix,
//! deliberately **not** `cap/` (which the capability resolver/`demand` scan): component working
//! state must not pollute the capability registry. (This refines the §E.2 sketch, which named
//! `cap/{me}/{ns}/*`, for that reason.)
//!
//! A component's **signals** are confined the same way ([`confine_kind`]): it may emit only kinds
//! under [`COMPONENT_SIGNAL_PREFIX`]`{namespace}/…` or kinds the host listed for it, and never a
//! protected RPC kind (`mcp.invoke`, `skill.invoke`, `llm.invoke`, the node's
//! `protected_rpc_kinds`) — those have doors of their own where authority is checked, and an
//! emit from inside the node's process would reach their handlers unframed, past every check.

use mycelium::NodeId;

/// KV prefix owned by component working-state. One private subtree per `(node, namespace)`.
pub const COMPONENT_KV_PREFIX: &str = "comp/";

/// Signal-kind prefix a component may emit under: `comp/{namespace}/…`. Fleet-wide (no node
/// segment — a signal is addressed by kind, not by the emitter), one family per namespace.
pub const COMPONENT_SIGNAL_PREFIX: &str = "comp/";

/// Why a component-relative key was refused. The guest cannot turn any of these into a
/// host-side write — confinement is enforced, not merely logged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfinementError {
    /// Empty key.
    Empty,
    /// Leading `/` — an attempt to address an absolute key outside the subtree.
    Absolute,
    /// A `..` path segment — a traversal attempt out of the subtree.
    Traversal,
    /// A signal kind that is protected work on this node (`mcp.invoke`, `skill.invoke`,
    /// `llm.invoke`, a configured `protected_rpc_kinds` entry): it has a door of its own where
    /// authority is checked, and a component is not that door.
    ProtectedKind,
    /// A signal kind outside the component's namespace (`comp/{namespace}/…`) and not listed for
    /// it by the host.
    ForeignKind,
    /// The component's namespace is empty or contains `/`, so its `comp/{namespace}/…` family
    /// would overlap another namespace's; such a component emits nothing.
    MalformedNamespace,
}

impl std::fmt::Display for ConfinementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "empty component key"),
            Self::Absolute => write!(f, "absolute key escapes the component subtree"),
            Self::Traversal => write!(f, "`..` traversal escapes the component subtree"),
            Self::ProtectedKind => write!(f, "protected RPC kind: a component may not emit work that has its own door"),
            Self::ForeignKind => write!(f, "signal kind outside the component's namespace and not listed for it"),
            Self::MalformedNamespace => write!(f, "component namespace is empty or contains `/`: its signal family would overlap another's"),
        }
    }
}

impl std::error::Error for ConfinementError {}

/// Map a component-relative `rel_key` to its confined absolute KV key
/// `comp/{node}/{namespace}/{rel_key}`, or refuse it.
///
/// `namespace` is host-set (from the component's manifest, trusted); only `rel_key` is
/// guest-controlled, so only it is validated. The guest can never read or write outside its
/// own `(node, namespace)` subtree — every escape shape (`/…`, `../…`, `a/../../b`) is rejected.
pub fn confine_key(node: &NodeId, namespace: &str, rel_key: &str) -> Result<String, ConfinementError> {
    if rel_key.is_empty() {
        return Err(ConfinementError::Empty);
    }
    if rel_key.starts_with('/') {
        return Err(ConfinementError::Absolute);
    }
    if rel_key.split('/').any(|seg| seg == "..") {
        return Err(ConfinementError::Traversal);
    }
    Ok(format!("{COMPONENT_KV_PREFIX}{node}/{namespace}/{rel_key}"))
}

/// May a component in `namespace` emit signal `kind`? A namespace that is empty or contains `/`
/// emits nothing (its family would overlap another namespace's). Allowed: a kind under
/// `comp/{namespace}/…`, or one of `listed` (kinds the host granted this component — never a
/// protected one, so a grant cannot open the door). Refused first and always: a kind in
/// [`mycelium::BUILTIN_PROTECTED_RPC_KINDS`] or in `protected` (the node's
/// `protected_rpc_kinds`). Everything else is foreign and refused.
///
/// `namespace`, `listed` and `protected` are host-set (trusted); only `kind` is guest-controlled.
pub fn confine_kind(namespace: &str, kind: &str, listed: &[String], protected: &[String]) -> Result<(), ConfinementError> {
    if kind.is_empty() {
        return Err(ConfinementError::Empty);
    }
    if namespace.is_empty() || namespace.contains('/') {
        return Err(ConfinementError::MalformedNamespace);
    }
    if mycelium::BUILTIN_PROTECTED_RPC_KINDS.contains(&kind) || protected.iter().any(|k| k == kind) {
        return Err(ConfinementError::ProtectedKind);
    }
    let own = format!("{COMPONENT_SIGNAL_PREFIX}{namespace}/");
    if kind.strip_prefix(own.as_str()).is_some_and(|rest| !rest.is_empty()) || listed.iter().any(|k| k == kind) {
        return Ok(());
    }
    Err(ConfinementError::ForeignKind)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node() -> NodeId {
        NodeId::new("127.0.0.1", 9000).unwrap()
    }

    #[test]
    fn confines_a_normal_key_under_the_component_subtree() {
        let k = confine_key(&node(), "nlp", "state/cursor").unwrap();
        assert_eq!(k, format!("comp/{}/nlp/state/cursor", node()));
        assert!(k.starts_with(COMPONENT_KV_PREFIX));
    }

    #[test]
    fn rejects_empty_key() {
        assert_eq!(confine_key(&node(), "nlp", ""), Err(ConfinementError::Empty));
    }

    #[test]
    fn rejects_absolute_key() {
        assert_eq!(confine_key(&node(), "nlp", "/etc/secret"), Err(ConfinementError::Absolute));
    }

    #[test]
    fn rejects_traversal_in_any_segment() {
        assert_eq!(confine_key(&node(), "nlp", ".."), Err(ConfinementError::Traversal));
        assert_eq!(confine_key(&node(), "nlp", "../other"), Err(ConfinementError::Traversal));
        assert_eq!(confine_key(&node(), "nlp", "a/../../cap/evil"), Err(ConfinementError::Traversal));
    }

    #[test]
    fn a_component_emits_only_under_its_own_signal_namespace() {
        let none: &[String] = &[];
        assert_eq!(confine_kind("nlp", "comp/nlp/tick", none, none), Ok(()));
        assert_eq!(confine_kind("nlp", "comp/nlp/", none, none), Err(ConfinementError::ForeignKind));
        assert_eq!(confine_kind("nlp", "comp/nlpx/tick", none, none), Err(ConfinementError::ForeignKind));
        assert_eq!(confine_kind("nlp", "comp/other/tick", none, none), Err(ConfinementError::ForeignKind));
        assert_eq!(confine_kind("nlp", "agent.state", none, none), Err(ConfinementError::ForeignKind));
        assert_eq!(confine_kind("nlp", "", none, none), Err(ConfinementError::Empty));
        // A host-listed kind is allowed...
        let listed = vec!["agent.state".to_string()];
        assert_eq!(confine_kind("nlp", "agent.state", &listed, none), Ok(()));
    }

    /// A namespace with a `/` (or none at all) would share another namespace's `comp/` family —
    /// `a` emitting `comp/a/b/x` is `a/b`'s kind — so such a namespace emits nothing. Seen failing
    /// first: `a/b` emitted `comp/a/b/x`.
    #[test]
    fn a_namespace_with_a_slash_or_none_emits_nothing() {
        let none: &[String] = &[];
        assert_eq!(confine_kind("a/b", "comp/a/b/x", none, none), Err(ConfinementError::MalformedNamespace));
        assert_eq!(confine_kind("", "comp//x", none, none), Err(ConfinementError::MalformedNamespace));
        assert_eq!(confine_kind("a/b", "listed.kind", &["listed.kind".to_string()], none), Err(ConfinementError::MalformedNamespace));
        assert_eq!(confine_kind("a", "comp/a/x", none, none), Ok(()));
    }

    #[test]
    fn a_protected_kind_is_refused_even_when_listed() {
        let none: &[String] = &[];
        for k in ["mcp.invoke", "skill.invoke", "llm.invoke"] {
            assert_eq!(confine_kind("nlp", k, none, none), Err(ConfinementError::ProtectedKind), "{k}");
            // ...and a listing cannot open the door.
            assert_eq!(confine_kind("nlp", k, &[k.to_string()], none), Err(ConfinementError::ProtectedKind), "{k} listed");
        }
        // The node's own `protected_rpc_kinds` are refused the same way.
        let protected = vec!["depot.dispatch".to_string()];
        assert_eq!(confine_kind("nlp", "depot.dispatch", none, &protected), Err(ConfinementError::ProtectedKind));
        assert_eq!(confine_kind("nlp", "depot.dispatch", &protected, &protected), Err(ConfinementError::ProtectedKind));
    }

    #[test]
    fn a_component_can_never_reach_another_namespace_or_the_cap_registry() {
        // Even a key that *names* cap/ stays inside the component's own subtree.
        let k = confine_key(&node(), "nlp", "cap/whatever").unwrap();
        assert!(k.starts_with(&format!("comp/{}/nlp/", node())));
        assert!(!k.starts_with("cap/"), "must not land in the capability registry");
    }
}
