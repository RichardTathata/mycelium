//! **Credentials attached at egress** (post-360 plan row L).
//!
//! A secret need not enter a component's or an activation command's environment, a unit file, or a
//! backend's fields: the component holds an opaque **placeholder**
//! ([`credential_placeholder`](crate::config::credential_placeholder) — `mycelium-credential:NAME`),
//! puts it where the secret would go (an `Authorization: Bearer …` value, an `x-api-key` header), and
//! sends the request through this gate. The gate holds each binding's value — read once, at
//! `GossipAgent::start()` ([`EgressCredentials::load`]) — and on the way out:
//!
//! - replaces every header that carries a placeholder with the binding's form (`bearer`, `header`,
//!   `basic`), **only** when the request's URL is one the binding authorises
//!   ([`CredentialBinding::authorises_url`]: the bound host exactly, under its path prefix, over https or
//!   to loopback unless `allow_plaintext`) — anywhere else the request is **refused**, before it is sent;
//! - follows redirects itself, through the node's [`EgressPolicy::redirect_verdict`] on every hop, and on
//!   each hop resolves the placeholders afresh: a hop the binding does not authorise gets **no**
//!   credential (dropped, counted) and no placeholder either; the cross-origin headers reqwest would
//!   strip (`Authorization`, `Cookie`, `Proxy-Authorization`) are stripped as reqwest strips them;
//! - counts each binding's uses, refusals and redirect drops ([`EgressCredentials::uses`]; under
//!   `metrics`, `mycelium_egress_credential_total{binding, event}`) — the label is the binding's
//!   name, never its value.
//!
//! **Where the value is not:** this module's `Debug` impls print names, hosts and sources, never a value;
//! a value is held as a *sensitive* `HeaderValue` (hyper prints it as `Sensitive`); no error, log line
//! or trace field carries one; nothing here writes to the evidence journal. **What this is not:** a
//! placeholder in a URL's query string or a request body is not resolved (it is sent as written — an
//! opaque name); a value is not zeroised when the gate is dropped; the gate compares host names, not
//! the addresses they resolve to (the allow-list's own limit).

use std::sync::atomic::{AtomicU64, Ordering};

use reqwest::header::{HeaderName, HeaderValue};

use crate::config::{credential_placeholders_in, CredentialBinding, CredentialForm, EgressPolicy, CREDENTIAL_PLACEHOLDER_MARKER};
use crate::error::GossipError;

/// One binding with its value, inside the gate.
struct Bound {
    cfg:     CredentialBinding,
    header:  HeaderName,
    /// The value in its attached form (`Bearer …`, `Basic …` or the bare value), marked sensitive.
    value:   HeaderValue,
    used:    AtomicU64,
    refused: AtomicU64,
    dropped: AtomicU64,
}

impl std::fmt::Debug for Bound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bound")
            .field("name", &self.cfg.name)
            .field("host", &self.cfg.host)
            .field("path_prefix", &self.cfg.path_prefix)
            .field("form", &self.cfg.form)
            .field("header", &self.header)
            .field("value", &"<redacted>")
            .field("used", &self.used.load(Ordering::Relaxed))
            .finish()
    }
}

/// What one binding has done, from [`EgressCredentials::uses`]. Counts, never the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialUse {
    /// The binding's name.
    pub name: String,
    /// Requests (or redirect hops) the value was attached to.
    pub attached: u64,
    /// Requests refused because their URL is not one the binding authorises.
    pub refused: u64,
    /// Redirect hops that left the binding, so the value was not attached.
    pub dropped_on_redirect: u64,
}

/// Why the gate did not send a request. No variant carries a credential's value.
#[derive(Debug)]
#[non_exhaustive]
pub enum CredentialError {
    /// A placeholder names a binding this node does not have.
    UnknownBinding { name: String },
    /// The request's URL is not one the binding authorises — sent nowhere.
    NotAuthorised { binding: String, reason: String },
    /// The node's egress allow-list does not permit the request's host.
    EgressDenied { url: String },
    /// A redirect was refused (the hop cap, a downgrade, a denied host, a body that cannot be re-sent).
    Redirect(String),
    /// The HTTP client's own error.
    Http(reqwest::Error),
}

impl std::fmt::Display for CredentialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownBinding { name } => write!(f, "egress credential: no binding named `{name}` on this node"),
            Self::NotAuthorised { reason, .. } => write!(f, "egress credential refused: {reason}"),
            Self::EgressDenied { url } => write!(f, "egress: the allow-list does not permit {url}"),
            Self::Redirect(why) => write!(f, "egress credential: redirect refused: {why}"),
            Self::Http(e) => write!(f, "egress credential: {e}"),
        }
    }
}

impl std::error::Error for CredentialError {}

/// The egress gate's credential half: every binding's value, and the client that sends requests
/// carrying them. Built once, at `start()` ([`GossipAgent::egress_credentials`](crate::GossipAgent::egress_credentials)),
/// or by an embedder with [`load`](Self::load).
pub struct EgressCredentials {
    bound:  Vec<Bound>,
    egress: EgressPolicy,
    /// Follows no redirect itself: [`send`](Self::send) follows them, re-resolving each hop.
    client: reqwest::Client,
}

impl std::fmt::Debug for EgressCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EgressCredentials").field("bindings", &self.bound).finish_non_exhaustive()
    }
}

impl EgressCredentials {
    /// Read each binding's value — `value_env` from the environment, `value_file` from disk (one
    /// trailing newline trimmed) — once, now. A binding that does not validate, whose value is unset,
    /// unreadable, empty, or not a valid header value is refused by name; the error never holds a value.
    pub fn load(bindings: &[CredentialBinding], egress: &EgressPolicy) -> Result<Self, GossipError> {
        // The rules `validate()` applies (names, forms, sources, the allow-list), so an embedder that
        // builds a gate without a config is held to them too.
        let probe = crate::config::GossipConfig {
            egress: egress.clone(),
            egress_credentials: bindings.to_vec(),
            ..Default::default()
        };
        probe.validate()?;
        let refuse = |name: &str, why: String| GossipError::InvalidField {
            field:  "egress_credentials",
            reason: format!("`{name}`: {why}"),
        };
        let mut bound = Vec::with_capacity(bindings.len());
        for b in bindings {
            let raw = match (&b.value_env, &b.value_file) {
                (Some(var), _) => std::env::var(var)
                    .map_err(|_| refuse(&b.name, format!("value_env `{var}` is not set (or not unicode) on this host")))?,
                (None, Some(path)) => {
                    let text = std::fs::read_to_string(path)
                        .map_err(|e| refuse(&b.name, format!("value_file {} cannot be read ({})", path.display(), e.kind())))?;
                    let text = text.strip_suffix('\n').unwrap_or(&text);
                    text.strip_suffix('\r').unwrap_or(text).to_string()
                }
                (None, None) => return Err(refuse(&b.name, "names no value source".into())),
            };
            if raw.is_empty() {
                return Err(refuse(&b.name, "its value is empty".into()));
            }
            let (header, attached) = match b.form {
                CredentialForm::Bearer => (reqwest::header::AUTHORIZATION, format!("Bearer {raw}")),
                CredentialForm::Basic => {
                    use base64::Engine as _;
                    let user = b.username.as_deref().unwrap_or_default();
                    (reqwest::header::AUTHORIZATION, format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{user}:{raw}"))))
                }
                CredentialForm::Header => {
                    let name = b.header.as_deref().unwrap_or_default();
                    let h = HeaderName::from_bytes(name.as_bytes()).map_err(|_| refuse(&b.name, format!("{name:?} is not a header name")))?;
                    (h, raw)
                }
            };
            let mut value = HeaderValue::from_str(&attached)
                .map_err(|_| refuse(&b.name, "its value is not a valid header value (a control character or a non-visible byte)".into()))?;
            value.set_sensitive(true);
            bound.push(Bound { cfg: b.clone(), header, value, used: AtomicU64::new(0), refused: AtomicU64::new(0), dropped: AtomicU64::new(0) });
        }
        let client = crate::agent::egress_client::build_or_none(crate::agent::egress_client::without_redirects());
        Ok(Self { bound, egress: egress.clone(), client })
    }

    /// True if this gate holds a binding called `name`.
    pub fn has(&self, name: &str) -> bool {
        self.bound.iter().any(|b| b.cfg.name == name)
    }

    /// The bindings' names, in configuration order.
    pub fn names(&self) -> Vec<String> {
        self.bound.iter().map(|b| b.cfg.name.clone()).collect()
    }

    /// Every binding's counts.
    pub fn uses(&self) -> Vec<CredentialUse> {
        self.bound
            .iter()
            .map(|b| CredentialUse {
                name: b.cfg.name.clone(),
                attached: b.used.load(Ordering::Relaxed),
                refused: b.refused.load(Ordering::Relaxed),
                dropped_on_redirect: b.dropped.load(Ordering::Relaxed),
            })
            .collect()
    }

    /// Send `req`, resolving its placeholders at the gate and following redirects as the module doc
    /// says. Build `req` with any client (`client.post(url).bearer_auth(placeholder).build()`); a
    /// per-request timeout carries, a client-level one does not (the gate sends on its own client). A
    /// request with no placeholder is sent as it is, under the same redirect rules.
    pub async fn send(&self, req: reqwest::Request) -> Result<reqwest::Response, CredentialError> {
        let mut template = req;
        let mut hop = 0usize;
        loop {
            let url = template.url().clone();
            if !self.egress.permits_url(url.as_str()) {
                return Err(CredentialError::EgressDenied { url: url.as_str().to_string() });
            }
            let (out, keep) = match template.try_clone() {
                Some(copy) => (copy, Some(template)),
                None => (template, None),
            };
            let out = self.resolve(out, hop)?;
            let resp = self.client.execute(out).await.map_err(CredentialError::Http)?;
            let status = resp.status();
            if !status.is_redirection() {
                return Ok(resp);
            }
            let Some(location) = resp.headers().get(reqwest::header::LOCATION).and_then(|l| l.to_str().ok()) else {
                return Ok(resp);
            };
            let next = url.join(location).map_err(|_| CredentialError::Redirect(format!("an unparsable Location {location:?}")))?;
            hop += 1;
            self.egress.redirect_verdict(hop, url.scheme(), next.scheme(), next.host_str()).map_err(CredentialError::Redirect)?;
            let Some(mut t) = keep else {
                return Err(CredentialError::Redirect("the request's body cannot be re-sent".into()));
            };
            let to_get = status == reqwest::StatusCode::SEE_OTHER
                || ((status == reqwest::StatusCode::MOVED_PERMANENTLY || status == reqwest::StatusCode::FOUND) && t.method() == reqwest::Method::POST);
            if to_get {
                *t.method_mut() = reqwest::Method::GET;
                *t.body_mut() = None;
                for h in [reqwest::header::CONTENT_TYPE, reqwest::header::CONTENT_LENGTH, reqwest::header::TRANSFER_ENCODING] {
                    t.headers_mut().remove(h);
                }
            }
            if url.host_str() != next.host_str() || url.port_or_known_default() != next.port_or_known_default() {
                // reqwest's cross-origin rule, kept: these are not re-sent to another origin — unless
                // the header carries a placeholder, which the next hop resolves (or drops) itself.
                for h in [reqwest::header::AUTHORIZATION, reqwest::header::COOKIE, reqwest::header::PROXY_AUTHORIZATION] {
                    let carries = t.headers().get_all(&h).iter().any(carries_placeholder);
                    if !carries {
                        t.headers_mut().remove(h);
                    }
                }
            }
            *t.url_mut() = next;
            template = t;
        }
    }

    /// Replace every placeholder-carrying header of `req` with its binding's form — or, on a
    /// redirect hop the binding does not authorise, with nothing.
    fn resolve(&self, mut req: reqwest::Request, hop: usize) -> Result<reqwest::Request, CredentialError> {
        let mut carriers: Vec<(HeaderName, Vec<String>)> = Vec::new();
        for (name, value) in req.headers() {
            if carries_placeholder(value) {
                let text = String::from_utf8_lossy(value.as_bytes());
                let names = credential_placeholders_in(&text).into_iter().map(str::to_string).collect();
                match carriers.iter_mut().find(|(n, _)| n == name) {
                    Some((_, v)) => v.extend::<Vec<String>>(names),
                    None => carriers.push((name.clone(), names)),
                }
            }
        }
        if carriers.is_empty() {
            return Ok(req);
        }
        let url = req.url().as_str().to_string();
        let mut attach: Vec<&Bound> = Vec::new();
        for (header, names) in carriers {
            req.headers_mut().remove(&header);
            for name in names {
                let Some(b) = self.bound.iter().find(|b| b.cfg.name == name) else {
                    return Err(CredentialError::UnknownBinding { name });
                };
                match b.cfg.authorises_url(&url) {
                    Ok(()) => {
                        if !attach.iter().any(|a| std::ptr::eq(*a, b)) {
                            attach.push(b);
                        }
                    }
                    Err(reason) if hop == 0 => {
                        b.refused.fetch_add(1, Ordering::Relaxed);
                        count(&b.cfg.name, "refused");
                        return Err(CredentialError::NotAuthorised { binding: b.cfg.name.clone(), reason });
                    }
                    Err(_) => {
                        b.dropped.fetch_add(1, Ordering::Relaxed);
                        count(&b.cfg.name, "dropped_on_redirect");
                        tracing::info!(binding = %b.cfg.name, hop, "egress credential: a redirect left the binding — sent without it");
                    }
                }
            }
        }
        for b in attach {
            req.headers_mut().insert(b.header.clone(), b.value.clone());
            b.used.fetch_add(1, Ordering::Relaxed);
            count(&b.cfg.name, "attached");
        }
        Ok(req)
    }
}

fn carries_placeholder(v: &HeaderValue) -> bool {
    let m = CREDENTIAL_PLACEHOLDER_MARKER.as_bytes();
    v.as_bytes().windows(m.len()).any(|w| w == m)
}

#[cfg(feature = "metrics")]
fn count(binding: &str, event: &'static str) {
    metrics::counter!("mycelium_egress_credential_total", "binding" => binding.to_string(), "event" => event).increment(1);
}

#[cfg(not(feature = "metrics"))]
fn count(_binding: &str, _event: &'static str) {}

#[cfg(test)]
#[path = "egress_credentials_tests.rs"]
mod tests;
