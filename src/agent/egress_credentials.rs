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
    /// What a [`Redactor`] masks for this binding: the bare value, and for `basic` the base64 credential
    /// that carries it. Never printed.
    secrets: Vec<Vec<u8>>,
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
    /// Two different bindings would set the same header on one request — refused, since only one value
    /// could be sent and neither caller asked for the other's.
    ConflictingBindings { header: String },
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
            Self::ConflictingBindings { header } => write!(f, "egress credential: two bindings would set the {header} header on one request"),
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
        Self::load_with(bindings, egress, crate::agent::egress_client::without_redirects())
    }

    /// [`load`](Self::load), with the gate's client also trusting `root_pem` — **for tests only** (a loopback
    /// TLS upstream signed by a test CA, so the https path is exercised without the network). Not built
    /// outside `cfg(test)` and feature `test-util`.
    #[cfg(any(test, feature = "test-util"))]
    #[doc(hidden)]
    pub fn load_trusting_root(bindings: &[CredentialBinding], egress: &EgressPolicy, root_pem: &[u8]) -> Result<Self, GossipError> {
        let root = reqwest::Certificate::from_pem(root_pem)
            .map_err(|e| GossipError::InvalidField { field: "egress_credentials", reason: format!("test root: {e}") })?;
        Self::load_with(bindings, egress, crate::agent::egress_client::without_redirects().add_root_certificate(root))
    }

    fn load_with(bindings: &[CredentialBinding], egress: &EgressPolicy, builder: reqwest::ClientBuilder) -> Result<Self, GossipError> {
        // The bindings' own rules (names, hosts and ports, forms, sources, the allow-list) — the ones
        // `GossipConfig::validate` runs — so an embedder that builds a gate without a config is held to
        // them too (row L's review, finding 8: this validated a whole default config).
        crate::config::validate_credential_bindings(bindings, egress)?;
        // Named by index and binding name; never the variable's name or the file's path, which a value
        // pasted into the wrong field would put in a start-time log line (row L's review, finding 2).
        let refuse = |i: usize, name: &str, why: String| GossipError::InvalidField {
            field:  "egress_credentials",
            reason: format!("[{i}] `{name}`: {why}"),
        };
        let mut bound = Vec::with_capacity(bindings.len());
        for (i, b) in bindings.iter().enumerate() {
            let raw = match (&b.value_env, &b.value_file) {
                (Some(var), _) => std::env::var(var)
                    .map_err(|_| refuse(i, &b.name, "the variable its value_env names is not set (or not unicode) on this host".into()))?,
                (None, Some(path)) => {
                    let text = std::fs::read_to_string(path)
                        .map_err(|e| refuse(i, &b.name, format!("the file its value_file names cannot be read ({})", e.kind())))?;
                    let text = text.strip_suffix('\n').unwrap_or(&text);
                    text.strip_suffix('\r').unwrap_or(text).to_string()
                }
                (None, None) => return Err(refuse(i, &b.name, "names no value source".into())),
            };
            if raw.is_empty() {
                return Err(refuse(i, &b.name, "its value is empty".into()));
            }
            let mut secrets = vec![raw.as_bytes().to_vec()];
            let (header, attached) = match b.form {
                CredentialForm::Bearer => (reqwest::header::AUTHORIZATION, format!("Bearer {raw}")),
                CredentialForm::Basic => {
                    use base64::Engine as _;
                    let user = b.username.as_deref().unwrap_or_default();
                    let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{raw}"));
                    secrets.push(encoded.as_bytes().to_vec());
                    (reqwest::header::AUTHORIZATION, format!("Basic {encoded}"))
                }
                CredentialForm::Header => {
                    let name = b.header.as_deref().unwrap_or_default();
                    let h = HeaderName::from_bytes(name.as_bytes()).map_err(|_| refuse(i, &b.name, "its header is not a header name".into()))?;
                    (h, raw)
                }
            };
            let mut value = HeaderValue::from_str(&attached)
                .map_err(|_| refuse(i, &b.name, "its value is not a valid header value (a control character or a non-visible byte)".into()))?;
            value.set_sensitive(true);
            bound.push(Bound { cfg: b.clone(), header, value, secrets, used: AtomicU64::new(0), refused: AtomicU64::new(0), dropped: AtomicU64::new(0) });
        }
        let client = crate::agent::egress_client::build_or_none(builder);
        Ok(Self { bound, egress: egress.clone(), client })
    }

    /// True if this gate holds a binding called `name`.
    pub fn has(&self, name: &str) -> bool {
        self.bound.iter().any(|b| b.cfg.name == name)
    }

    /// Would the binding `name` attach to a request for `url`? What a caller checks at its own start
    /// (a stem's `[[serve]]`) so a misdirected endpoint is refused there, not at its first call.
    pub fn authorises(&self, name: &str, url: &str) -> Result<(), CredentialError> {
        let b = self.bound.iter().find(|b| b.cfg.name == name).ok_or_else(|| CredentialError::UnknownBinding { name: name.to_string() })?;
        b.cfg.authorises_url(url).map_err(|reason| CredentialError::NotAuthorised { binding: name.to_string(), reason })
    }

    /// Is `candidate` the value of any binding here? Compared in constant time per binding; for a check that
    /// keeps a value out of somewhere it must not be (a confined command's environment), never to print it.
    pub fn holds_value(&self, candidate: &str) -> bool {
        let c = candidate.as_bytes();
        self.bound.iter().any(|b| b.secrets.first().is_some_and(|s| s.len() == c.len() && s.iter().zip(c).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0))
    }

    /// Does any binding's value appear **inside** `text` (#644's round-2 review, N7)? What keeps a value out of a
    /// variable that carries it within a longer string. Never prints either.
    pub fn appears_in(&self, text: &str) -> bool {
        let t = text.as_bytes();
        self.bound.iter().any(|b| b.secrets.first().is_some_and(|s| !s.is_empty() && t.windows(s.len()).any(|w| w == &s[..])))
    }

    /// A [`Redactor`] for the bindings named (unknown names are skipped): what masks their values in a
    /// response before it is relayed to a party that must not hold them (an activation's command, behind its
    /// egress proxy, when an upstream echoes what it was sent).
    pub fn redactor(&self, names: &[String]) -> Redactor {
        let needles = self
            .bound
            .iter()
            .filter(|b| names.contains(&b.cfg.name))
            .flat_map(|b| b.secrets.iter().cloned())
            .filter(|s| !s.is_empty())
            .collect::<Vec<Vec<u8>>>();
        // #644's round-2 review, N4: longest first, so a value that prefixes another cannot mask the longer
        // one's head and leave its tail.
        let mut needles = needles;
        needles.sort_by_key(|n| std::cmp::Reverse(n.len()));
        Redactor { needles }
    }

    /// Is the binding `name` bound to `host` (any port, any scheme)? What an activation's egress proxy asks
    /// before it opens a tunnel it cannot see into to a host that receives a value.
    pub fn binds_host(&self, name: &str, host: &str) -> bool {
        let bare = |h: &str| h.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
        self.bound.iter().any(|b| b.cfg.name == name && bare(&b.cfg.host) == bare(host))
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
            // Only the redirects reqwest follows (row L's review, finding 5): a 300, 304 or 305 is an
            // answer, handed back as it arrived — following a 300 replayed the body to its Location.
            if !matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308) {
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

    /// Send `req` once, resolving its placeholders at the gate as [`send`](Self::send) does on its first
    /// hop — under the node's allow-list, the value attached only where the binding authorises, a request it
    /// does not authorise refused before it is sent — and **follow no redirect**: a 3xx comes back as it
    /// arrived. For a caller that must judge every hop by a rule of its own before it is taken (a stem's
    /// activation egress proxy, whose command follows the redirect through the proxy again, so the hop meets
    /// the activation's allow-list and this gate afresh).
    pub async fn send_once(&self, req: reqwest::Request) -> Result<reqwest::Response, CredentialError> {
        if !self.egress.permits_url(req.url().as_str()) {
            return Err(CredentialError::EgressDenied { url: req.url().as_str().to_string() });
        }
        let out = self.resolve(req, 0)?;
        self.client.execute(out).await.map_err(CredentialError::Http)
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
        // Two bindings that would set one header (two bearers, or a header form naming `authorization`)
        // are refused: only one value could be sent, and neither caller asked for the other's
        // (row L's review, finding 4 — both used to be counted, one sent).
        for (k, a) in attach.iter().enumerate() {
            if attach[k + 1..].iter().any(|b| b.header == a.header) {
                return Err(CredentialError::ConflictingBindings { header: a.header.as_str().to_string() });
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

/// Masks credential values in bytes headed somewhere they must not arrive ([`EgressCredentials::redactor`]):
/// every occurrence of a value becomes the same number of `*`, so a length (`Content-Length`) stays true.
/// Its `Debug` prints how many values it masks, never one.
pub struct Redactor {
    needles: Vec<Vec<u8>>,
}

impl std::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Redactor").field("values", &self.needles.len()).finish()
    }
}

impl Redactor {
    /// Masks nothing.
    pub fn is_empty(&self) -> bool {
        self.needles.is_empty()
    }

    /// Mask every value in `buf` in place; true if any was found.
    pub fn mask(&self, buf: &mut [u8]) -> bool {
        let mut hit = false;
        for n in &self.needles {
            let mut i = 0;
            while i + n.len() <= buf.len() {
                if buf[i..i + n.len()] == n[..] {
                    buf[i..i + n.len()].fill(b'*');
                    hit = true;
                    i += n.len();
                } else {
                    i += 1;
                }
            }
        }
        hit
    }

    /// A header value with every value masked — `None` when it held none.
    pub fn header(&self, v: &HeaderValue) -> Option<HeaderValue> {
        let mut b = v.as_bytes().to_vec();
        self.mask(&mut b).then(|| HeaderValue::from_bytes(&b).unwrap_or_else(|_| HeaderValue::from_static("*")))
    }

    /// A masker for a body that arrives in chunks: a value split across chunks is masked too. It withholds
    /// the last (longest value − 1) bytes of each chunk until the next arrives or [`RedactStream::finish`].
    pub fn stream(&self) -> RedactStream {
        let keep = self.needles.iter().map(Vec::len).max().unwrap_or(1).saturating_sub(1);
        RedactStream { redactor: Redactor { needles: self.needles.clone() }, tail: Vec::new(), keep }
    }
}

/// [`Redactor::stream`]'s state.
pub struct RedactStream {
    redactor: Redactor,
    tail: Vec<u8>,
    keep: usize,
}

impl std::fmt::Debug for RedactStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedactStream").field("withheld", &self.tail.len()).finish()
    }
}

impl RedactStream {
    /// The masked bytes now safe to send. A value that ends in a later chunk starts within the withheld
    /// tail, so it is masked when that chunk arrives.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<u8> {
        let mut buf = std::mem::take(&mut self.tail);
        buf.extend_from_slice(chunk);
        self.redactor.mask(&mut buf);
        let split = buf.len().saturating_sub(self.keep);
        self.tail = buf.split_off(split);
        buf
    }

    /// What is withheld, masked.
    pub fn finish(mut self) -> Vec<u8> {
        let mut t = std::mem::take(&mut self.tail);
        self.redactor.mask(&mut t);
        t
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
