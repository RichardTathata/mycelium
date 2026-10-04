//! The one place an outbound HTTP client is built (realignment repairs R3; the review's F03).
//!
//! The egress allow-list gated the first URL of every outbound call, and every client then followed
//! reqwest's default ten redirects wherever they led. So an allowed MCP server, LLM endpoint or
//! OIDC issuer could send the node's request — a handshake, a prompt, a JWKS fetch — to a host the
//! list denies. A redirect is a destination, so the rule is now:
//!
//! - a client that knows the node's [`EgressPolicy`] re-checks **every hop** through
//!   [`EgressPolicy::redirect_verdict`] — target host on the list, at most
//!   [`EgressPolicy::MAX_REDIRECT_HOPS`] hops, never `https` → `http` ([`with_policy`]);
//! - a client built without one follows **no** redirect ([`without_redirects`]);
//! - a client that carries a credential header reqwest does not strip cross-host (the federation
//!   client's `x-mycelium-federation-call`, the bulk peer fetch) follows **no** redirect either,
//!   whatever the policy says — a redirect would hand the credential to a host the caller never
//!   chose.
//!
//! With an empty allow-list a policy-aware client follows redirects as before, re-checked for the
//! hop cap and the downgrade only, so a deployment that never configured egress keeps working.
//!
//! The companions (`mycelium-wasm-host`, `mycelium-reason`) build their own clients against the
//! same verdict; they depend on `mycelium` without `gateway`, so they cannot import this module.

use crate::config::EgressPolicy;

/// The redirect policy for a client that knows the node's egress policy: every hop goes through
/// [`EgressPolicy::redirect_verdict`].
pub fn redirect_policy(egress: EgressPolicy) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        let hop = attempt.previous().len();
        let from = attempt.previous().last().map(|u| u.scheme().to_string()).unwrap_or_default();
        let to = attempt.url();
        match egress.redirect_verdict(hop, &from, to.scheme(), to.host_str()) {
            Ok(()) => attempt.follow(),
            Err(why) => attempt.error(why),
        }
    })
}

/// A client builder whose redirects are re-checked against `egress` on every hop.
pub fn with_policy(egress: &EgressPolicy) -> reqwest::ClientBuilder {
    reqwest::Client::builder().redirect(redirect_policy(egress.clone()))
}

/// A client builder that follows no redirect: a 3xx is returned to the caller as it arrived.
pub fn without_redirects() -> reqwest::ClientBuilder {
    reqwest::Client::builder().redirect(reqwest::redirect::Policy::none())
}

/// Build, falling back to a client that follows **no** redirect if the TLS backend cannot be
/// initialised — never to `reqwest::Client::new()`, which would follow ten unchecked.
pub fn build_or_none(builder: reqwest::ClientBuilder) -> reqwest::Client {
    builder.build().unwrap_or_else(|e| {
        tracing::error!("egress: an HTTP client could not be built ({e}); falling back to one that follows no redirect");
        without_redirects().build().expect("a client with no options and no redirects builds")
    })
}
