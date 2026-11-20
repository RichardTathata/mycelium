/// Node TLS context — always compiles, only has content with the `tls` feature.
///
/// When `tls` is disabled this is a zero-size struct and every method is
/// unreachable; the struct is used only as a type in `Option<Arc<NodeTls>>`
/// so function signatures stay uniform regardless of the feature flag.
///
/// The handle (`Arc<NodeTls>` in `TaskCtx::tls`) is still set once, but its
/// inner state is **swappable at runtime** (WS5 hot cert rotation): the active
/// signing key and rustls configs live behind lock-free [`arc_swap::ArcSwap`]
/// cells, so [`rotate`](NodeTls::rotate) can replace them atomically while
/// signing/handshake paths keep reading the current value via the accessor
/// methods. Read the key/configs through the methods, never a cached clone, so
/// a rotation is observed.
/// The identity-anchor sink: records a directly-connected peer's CA-validated Ed25519 key
/// (identity-auth Phase 1b). Installed once via [`NodeTls::set_anchor_sink`].
#[cfg(feature = "tls")]
pub type AnchorSink = std::sync::Arc<dyn Fn(&crate::node_id::NodeId, [u8; 32]) + Send + Sync>;

pub struct NodeTls {
    #[cfg(feature = "tls")]
    server_config: arc_swap::ArcSwap<rustls::ServerConfig>,
    #[cfg(feature = "tls")]
    client_config: arc_swap::ArcSwap<rustls::ClientConfig>,
    #[cfg(feature = "tls")]
    signing_key: arc_swap::ArcSwap<ed25519_dalek::SigningKey>,
    /// Server-only rustls config for the HTTP gateway (SOC 2 WS-A): same node
    /// identity cert, but built `.with_no_client_auth()` so ordinary HTTP clients
    /// (no client cert) can connect; used only when `GossipConfig::gateway_tls` reuses the node
    /// cert. Built **once**, over a resolver that reads [`gateway_key`](Self::gateway_key) at every
    /// handshake, so a listener holding this config presents the current identity after a rotation
    /// (external review R7's review, finding 4: the gateway kept a one-time copy and served the old
    /// certificate, which a bulk fetch refuses once the old key is revoked).
    #[cfg(feature = "tls")]
    gateway_server_config: std::sync::Arc<rustls::ServerConfig>,
    /// The gateway's current certificate and key; swapped by [`activate`](NodeTls::activate).
    #[cfg(feature = "tls")]
    gateway_key: std::sync::Arc<arc_swap::ArcSwap<rustls::sign::CertifiedKey>>,
    /// The fleet's server-certificate check — the CA chain, then the removal and revocation checks —
    /// for a client this node builds outside the mesh (the bulk fetch, R7). One verifier for the node's
    /// life, handed out by [`fleet_server_verifier`](NodeTls::fleet_server_verifier), that delegates at
    /// every verification to the check [`activate`](NodeTls::activate) last installed: a client built
    /// once at start (the bulk fetch's) follows a change of roots (identity-and-lifecycle I2).
    #[cfg(feature = "tls")]
    fleet_server_verifier: std::sync::Arc<dyn rustls::client::danger::ServerCertVerifier>,
    /// The check `fleet_server_verifier` delegates to; swapped by `activate` when the roots change.
    #[cfg(feature = "tls")]
    fleet_server_current: std::sync::Arc<arc_swap::ArcSwap<std::sync::Arc<dyn rustls::client::danger::ServerCertVerifier>>>,
    /// Identity-anchor sink (identity-auth Phase 1b): records a directly-connected peer's
    /// CA-validated Ed25519 key. Installed once at agent start with a closure capturing the
    /// `CoreCtx` anchor maps; the outbound writer calls it after each handshake. `OnceLock` so
    /// the hot connect path reads it lock-free and it is set exactly once. Not rotated (the sink
    /// captures long-lived map `Arc`s, independent of the swapped cert/config).
    #[cfg(feature = "tls")]
    peer_anchor_sink: std::sync::OnceLock<AnchorSink>,
    /// This node's current certificate (DER), published as its certificate record
    /// (`sys/identity-cert/{node}/{key}`, post-360 plan row Q). Rotates with the identity.
    #[cfg(feature = "tls")]
    node_cert: arc_swap::ArcSwap<Vec<u8>>,
    /// The fleet CA's chain check, without the removal check — what a certificate record is judged
    /// by (`verify_member_cert`). A same-CA rotation keeps it; a change of roots swaps it
    /// (identity-and-lifecycle I2), **before** `roots_generation` is bumped.
    #[cfg(feature = "tls")]
    cert_verifier: arc_swap::ArcSwap<std::sync::Arc<dyn rustls::server::danger::ClientCertVerifier>>,
    /// The CA certificates this node trusts now (DER), in the order the root store was built from.
    #[cfg(feature = "tls")]
    roots: arc_swap::ArcSwap<Vec<Vec<u8>>>,
    /// The **roots generation** (identity-and-lifecycle I2, R4): bumped by `activate`, with a `Release`
    /// store, after it has swapped the verifiers for material that changes the roots. A reader loads it
    /// with `Acquire` **before** it loads a verifier ([`verify_member_cert_tagged`](NodeTls::verify_member_cert_tagged)),
    /// so a reader that sees the new generation sees the new verifier, and one that sees the old tags
    /// what it judged with the old: a cached verdict of an older generation is a miss.
    #[cfg(feature = "tls")]
    roots_generation: std::sync::atomic::AtomicU64,
    /// The removal and standing sets the mesh verifiers read — kept so a change of roots
    /// ([`reroot_material`](NodeTls::reroot_material)) builds verifiers over the same live sets.
    #[cfg(feature = "tls")]
    removed: std::sync::Arc<crate::removal::RemovedSet>,
    #[cfg(feature = "tls")]
    standing: std::sync::Arc<crate::removal::KeyStanding>,
    /// Held for the whole of [`activate`](NodeTls::activate) and while [`rotation_after`](NodeTls::rotation_after)
    /// reads the roots it builds over: one writer of the identity and the roots at a time (#796's review,
    /// finding 2). Never held across an `.await` or another lock (lock-order table, `NodeTls::activate_lock`).
    #[cfg(feature = "tls")]
    activate_lock: std::sync::Mutex<()>,
    /// Mesh sessions closed because their peer's certificate fails roots this node changed to (I2, #805's review F10):
    /// `[inbound old root, inbound predates the change, outbound old root, outbound predates the change]`.
    #[cfg(feature = "tls")]
    sessions_closed: [std::sync::atomic::AtomicU64; 4],
}

#[cfg(feature = "tls")]
impl NodeTls {
    /// The current rustls server config (accept side). Cloned cheaply (Arc).
    pub fn server_config(&self) -> std::sync::Arc<rustls::ServerConfig> {
        self.server_config.load_full()
    }
    /// The current rustls client config (connect side).
    pub fn client_config(&self) -> std::sync::Arc<rustls::ClientConfig> {
        self.client_config.load_full()
    }
    /// The server-only rustls config for the HTTP gateway (node-cert reuse path). One config for
    /// the node's life: its resolver presents the identity [`activate`](NodeTls::activate) last
    /// installed, at every handshake.
    pub fn gateway_server_config(&self) -> std::sync::Arc<rustls::ServerConfig> {
        std::sync::Arc::clone(&self.gateway_server_config)
    }
    /// The fleet's server-certificate check (CA chain, removal, revocation), for a client built
    /// outside the mesh — the bulk fetch (external review R7).
    ///
    /// The verifier returned is the same for the node's life and delegates, at each verification, to
    /// the check over the roots the node trusts **then**: a client holding it follows a change of roots
    /// (identity-and-lifecycle I2).
    pub fn fleet_server_verifier(&self) -> std::sync::Arc<dyn rustls::client::danger::ServerCertVerifier> {
        std::sync::Arc::clone(&self.fleet_server_verifier)
    }

    /// The roots generation (identity-and-lifecycle I2, R4), loaded with `Acquire`: `0` at start, one
    /// more at each [`activate`](NodeTls::activate) whose material changed the roots.
    pub fn roots_generation(&self) -> u64 {
        self.roots_generation.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Count a mesh session closed at a change of roots (#805's review F10): `inbound` or outbound, and **why** — the
    /// peer's key already has standing under the new roots (the session predates the change: the peer has a successor
    /// certificate it would present on a new handshake) or it has none (the peer is on an old root).
    pub fn note_session_closed(&self, inbound: bool, peer_der: &[u8]) {
        // Only a standing the fold has judged under the current roots can say so: until then, an old-root anchor not yet
        // dropped would read as a successor's, so the close counts as an old-root peer's.
        let current = self.standing.judged_under() == Some(self.roots_generation());
        let predates = current && ed25519_key_from_cert_der(peer_der).is_some_and(|k| self.standing.anchors_key(&k));
        let i = (if inbound { 0 } else { 2 }) + usize::from(predates);
        self.sessions_closed[i].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// [`Self::note_session_closed`]'s counters: inbound old root, inbound predates the change, outbound old root,
    /// outbound predates the change.
    pub fn sessions_closed_by_roots(&self) -> [u64; 4] {
        std::array::from_fn(|i| self.sessions_closed[i].load(std::sync::atomic::Ordering::Relaxed))
    }

    /// The CA certificates (DER) this node trusts now.
    pub fn roots_der(&self) -> Vec<Vec<u8>> {
        self.roots.load().as_ref().clone()
    }

    /// Install the identity-anchor sink (identity-auth Phase 1b) — called once at agent start
    /// with a closure that records a peer's CA-validated key into the `CoreCtx` anchor maps.
    /// Idempotent (first wins).
    pub fn set_anchor_sink(&self, sink: AnchorSink) {
        let _ = self.peer_anchor_sink.set(sink);
    }

    /// Record a directly-connected peer's CA-validated key via the installed sink; no-op if none
    /// is installed. Called by the outbound writer after a completed handshake, with the roots generation
    /// it read **before** it loaded the client config the handshake ran under (identity-and-lifecycle I2,
    /// #796's review must-address 6): a handshake judged under roots that have changed since records
    /// nothing, so an old-roots verdict never lands after the agent cleared the anchors at a change of roots.
    pub fn record_anchor(&self, peer: &crate::node_id::NodeId, key: [u8; 32], judged_at: u64) {
        if judged_at != self.roots_generation() {
            return;
        }
        if let Some(sink) = self.peer_anchor_sink.get() {
            sink(peer, key);
        }
    }
    /// The current Ed25519 signing/identity key.
    pub fn signing_key(&self) -> std::sync::Arc<ed25519_dalek::SigningKey> {
        self.signing_key.load_full()
    }
    /// The current 32-byte verifying key.
    pub fn verifying_key_bytes(&self) -> [u8; 32] {
        self.signing_key().verifying_key().to_bytes()
    }

    /// Atomically swap in previously-generated rotation material — the cutover
    /// step of a hot rotation (WS5). New gossip signatures and new TLS handshakes
    /// (`server_config()` / `client_config()` are read per connection) pick up the
    /// new key/cert immediately; existing connections keep their old (CA-trusted)
    /// session. Call only *after* the new verifying key has been published to
    /// peers, so they already accept it.
    ///
    /// **A change of roots** (material from [`reroot_material`](NodeTls::reroot_material),
    /// identity-and-lifecycle I2) also swaps the chain check behind
    /// [`verify_member_cert`](NodeTls::verify_member_cert) and the fleet's server check, and then — only
    /// then, with a `Release` store — bumps [`roots_generation`](NodeTls::roots_generation). A same-CA
    /// rotation carries no roots and leaves both checks and the generation as they were.
    ///
    /// **One writer at a time** (#796's review, finding 2): `activate` holds `activate_lock` for its whole
    /// body, so two activations never interleave their stores. Material that carries no roots was built
    /// over a root set — the one [`rotation_after`](NodeTls::rotation_after) read, or the configuration's
    /// root for the free [`generate_rotation_after`] — and is **refused** when the roots have changed
    /// since: its mesh configs would trust the old set while the chain check trusted the new one, so a
    /// peer `verify_member_cert` accepts would be refused at the handshake. Rebuild it and activate again.
    pub fn activate(&self, m: RotationMaterial) -> Result<(), crate::error::GossipError> {
        let _one_writer = self.activate_lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if m.roots.is_none() {
            let now = self.roots_generation();
            let built = m.built_over.unwrap_or(0);
            if built != now || (m.built_over.is_none() && now != 0) {
                return Err(crate::error::GossipError::InvalidField {
                    field: "tls",
                    reason: format!(
                        "TLS: rotation material built over roots generation {built} cannot be activated at generation \
                         {now}: the node's roots changed after it was built — build it again (NodeTls::rotation_after)"
                    ),
                });
            }
        }
        self.signing_key.store(m.signing_key);
        self.server_config.store(m.server_config);
        self.client_config.store(m.client_config);
        self.gateway_key.store(m.gateway_key);
        self.node_cert.store(std::sync::Arc::new(m.node_cert_der));
        if let Some(roots) = m.roots {
            // R1: a change that closes legacy closes it **before** the new chain check is visible, so no reader sees
            // the narrowed roots with legacy still open; one that would reopen it (refused at widen, I2's record
            // check) reopens only after.
            if roots.legacy_closed {
                self.standing.set_legacy_closed(true);
            }
            self.cert_verifier.store(std::sync::Arc::new(roots.chain));
            self.fleet_server_current.store(std::sync::Arc::new(roots.fleet));
            self.roots.store(std::sync::Arc::new(roots.ders));
            #[cfg(test)]
            if let Some(plant) = ACTIVATE_PLANT.with(|p| p.borrow_mut().take()) {
                plant(self);
            }
            self.roots_generation.fetch_add(1, std::sync::atomic::Ordering::Release);
            if !roots.legacy_closed {
                self.standing.set_legacy_closed(false);
            }
        }
        Ok(())
    }

    /// **A same-CA rotation over the roots this node trusts now** (#796's review, finding 2): a fresh key and
    /// its certificate, issued with the CA key in `cfg` exactly as [`generate_rotation_after`] issues it, but
    /// with the mesh configs built over this node's **current** roots — after a change of roots those are
    /// not the configuration file's. The material records the roots generation it was built over, and
    /// [`activate`](NodeTls::activate) refuses it if the roots change before it is activated. The agent's
    /// `rotate_identity` rotates through this.
    pub fn rotation_after(&self, cfg: &crate::config::TlsConfig, node_id: &crate::node_id::NodeId, issued_floor_ms: u64) -> Result<RotationMaterial, crate::error::GossipError> {
        let (generation, roots) = {
            let _one_writer = self.activate_lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            (self.roots_generation(), self.roots_der())
        };
        imp::rotation_over(cfg, node_id, std::sync::Arc::clone(&self.removed), std::sync::Arc::clone(&self.standing), issued_floor_ms, Some((generation, roots)))
    }

    /// This node's current certificate, DER (post-360 plan row Q: the node's certificate record).
    pub fn node_cert_der(&self) -> Vec<u8> {
        self.node_cert.load().as_ref().clone()
    }

    /// Whether `der` is a certificate the fleet CA issued and that is valid now (chain and
    /// validity dates, judged at the wall clock through the seam) — the check a certificate record
    /// must pass before its key has any standing (post-360 plan row Q). The removal check is not
    /// part of it: a removed member's keys are filtered where keys are read.
    pub fn verify_member_cert(&self, der: &[u8]) -> bool {
        self.verify_member_cert_tagged(der).1
    }

    /// [`verify_member_cert`](NodeTls::verify_member_cert), with the roots generation it judged under
    /// (identity-and-lifecycle I2, R4): the generation is loaded (`Acquire`) **before** the chain check
    /// is, so a verdict tagged `g` was reached by generation `g`'s check or a later one's, never an
    /// earlier one's. A cache of verdicts stores the tag and treats a verdict whose tag is not the
    /// current [`roots_generation`](NodeTls::roots_generation) as a miss — clearing the cache at
    /// `activate` would not do: a judgment running across `activate` would insert its old-roots verdict
    /// after the clear.
    pub fn verify_member_cert_tagged(&self, der: &[u8]) -> (u64, bool) {
        let generation = self.roots_generation();
        #[cfg(test)]
        if let Some(plant) = READER_PLANT.with(|p| p.borrow_mut().take()) {
            plant(self);
        }
        let verifier = self.cert_verifier.load_full();
        let cert = rustls::pki_types::CertificateDer::from(der);
        let now = rustls::pki_types::UnixTime::since_unix_epoch(std::time::Duration::from_millis(
            crate::sim_seam::wall_now_ms(),
        ));
        (generation, verifier.verify_client_cert(&cert, &[], now).is_ok())
    }

    /// **Rotation material for a change of roots** (identity-and-lifecycle I2's reload path): this
    /// node's **current** key, presenting `node_cert_der` — a certificate for that key, pre-issued on
    /// the issuer's host — over a root store of every certificate in `roots`. No CA key is read or
    /// needed, so a node on the secure posture (`cert_pem`, no `ca-key.pem`) can take it live, with no
    /// restart; [`activate`](NodeTls::activate) installs it.
    ///
    /// Refused, by name: no roots; a root that is not a CA certificate rustls can anchor; a certificate
    /// whose key is not this node's current key; and a certificate the given roots do not verify now
    /// (a node never presents a certificate its own new roots would refuse).
    ///
    /// This is the reload path alone. What decides **when** a node widens, cuts over and narrows — the
    /// successor-signed record, its phases persisted under `succession/` — and the re-judging of the
    /// kept anchors (`anchors_seen`) are I2's later PRs: until they land, nothing in the agent calls
    /// this, and a caller that does is judged by the stated state of row I2.
    pub fn reroot_material(&self, node_cert_der: Vec<u8>, roots: Vec<Vec<u8>>) -> Result<RotationMaterial, crate::error::GossipError> {
        imp::reroot_material(self, node_cert_der, roots)
    }
}

#[cfg(all(test, feature = "tls"))]
thread_local! {
    /// A plant for the roots-generation ordering test (I2, exit 18): run between `activate`'s swap of the
    /// verifiers and its generation bump, on this thread's next change of roots.
    pub(crate) static ACTIVATE_PLANT: std::cell::RefCell<Option<ActivatePlant>> = const { std::cell::RefCell::new(None) };
}

/// What [`ACTIVATE_PLANT`] runs.
#[cfg(all(test, feature = "tls"))]
pub(crate) type ActivatePlant = Box<dyn FnOnce(&NodeTls)>;

#[cfg(all(test, feature = "tls"))]
thread_local! {
    /// A plant for the reader-order test (#796's review, finding 3): run between `verify_member_cert_tagged`'s
    /// load of the roots generation and its load of the chain check, on this thread's next call.
    pub(crate) static READER_PLANT: std::cell::RefCell<Option<ActivatePlant>> = const { std::cell::RefCell::new(None) };
}

/// A freshly-generated identity key + CA-signed cert + rustls configs, not yet
/// activated. Produced by `generate_rotation`; consumed by `NodeTls::activate`.
#[cfg(feature = "tls")]
pub struct RotationMaterial {
    pub verifying_key: [u8; 32],
    server_config: std::sync::Arc<rustls::ServerConfig>,
    client_config: std::sync::Arc<rustls::ClientConfig>,
    signing_key: std::sync::Arc<ed25519_dalek::SigningKey>,
    gateway_key: std::sync::Arc<rustls::sign::CertifiedKey>,
    /// The new key's certificate, DER — published as its certificate record before the cutover.
    pub node_cert_der: Vec<u8>,
    /// New roots and the chain checks over them (a change of roots, I2), or `None` for a same-CA
    /// rotation, which keeps the node's checks.
    roots: Option<RootsMaterial>,
    /// For a same-CA rotation: the roots generation whose roots its mesh configs were built over, or
    /// `None` when they were built over the configuration's root (the free `generate_rotation_after`).
    built_over: Option<u64>,
}

/// The chain checks over a new root set, carried by [`RotationMaterial`] for a change of roots.
#[cfg(feature = "tls")]
struct RootsMaterial {
    /// Whether every root is generation 1 or later (I2, R1): what [`NodeTls::activate`] sets
    /// [`KeyStanding::legacy_closed`](crate::removal::KeyStanding::legacy_closed) to.
    legacy_closed: bool,
    ders: Vec<Vec<u8>>,
    chain: std::sync::Arc<dyn rustls::server::danger::ClientCertVerifier>,
    fleet: std::sync::Arc<dyn rustls::client::danger::ServerCertVerifier>,
}

/// What a node certificate says about whose key it is (post-360 plan row Q): its Ed25519 key, the
/// `mycelium://{node}` URIs among its subject alternative names, and its issuance time — carried in
/// the serial number as milliseconds since the epoch by `generate_node_cert`, because `notBefore` is
/// whole seconds (two certificates one rotation apart would tie) and dating it *now* would make a
/// peer whose clock is behind refuse the handshake. `issued_ms` is `None` for a serial that is not
/// such a time (every certificate issued before row Q: a 20-byte digest).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertIdentity {
    pub key: [u8; 32],
    pub node_uris: Vec<String>,
    pub issued_ms: Option<u64>,
}

/// The issuance time a new certificate carries: the wall clock, but strictly after `floor_ms` (the
/// newest issuance among the node's own certificates), so a clock stepped back never mints a key that
/// ranks below one it succeeds (post-360 plan row Q).
pub fn issuance_after(floor_ms: u64) -> u64 {
    crate::sim_seam::wall_now_ms().max(floor_ms.saturating_add(1))
}

#[cfg(all(feature = "tls", any(test, feature = "test-support")))]
thread_local! {
    static NOT_AFTER_OVERRIDE: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// The `notAfter` a node or client certificate is issued with: 2099-01-01, or — in a test build, on this thread inside
/// [`with_not_after_for_test`] — the given time, so a test can issue a short-lived certificate (#805's review F3).
#[cfg(feature = "tls")]
pub(crate) fn set_not_after_for_issue(params: &mut rcgen::CertificateParams) {
    params.not_after = rcgen::date_time_ymd(2099, 1, 1);
    #[cfg(any(test, feature = "test-support"))]
    if let Some(ms) = NOT_AFTER_OVERRIDE.with(|c| c.get()) {
        params.not_after = rcgen::date_time_ymd(1970, 1, 1) + std::time::Duration::from_millis(ms);
    }
}

/// Run `f` with every node or client certificate issued on this thread valid until `not_after_ms` (test builds only).
#[cfg(all(feature = "tls", any(test, feature = "test-support")))]
#[doc(hidden)]
pub fn with_not_after_for_test<R>(not_after_ms: u64, f: impl FnOnce() -> R) -> R {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            NOT_AFTER_OVERRIDE.with(|c| c.set(None));
        }
    }
    NOT_AFTER_OVERRIDE.with(|c| c.set(Some(not_after_ms)));
    let _reset = Reset;
    f()
}

/// The URI a node certificate names its node by: `mycelium://{node}`.
pub fn node_uri(node: &crate::node_id::NodeId) -> String {
    format!("mycelium://{node}")
}

/// The scheme a **client** certificate names its principal by (identity-and-lifecycle I1, D1):
/// `mycelium-client://{issuer}/{name}` — the gateway principal `token:{issuer}/{name}`. A client
/// certificate is issued by the fleet CA to a service's process (`mycelium tls issue --client`), never
/// to a node: both mesh verifiers refuse a certificate naming this scheme, so a client key cannot join
/// the mesh, and `NodeTls::verify_member_cert` (the chain alone) still accepts it, so the client-cert
/// fold can judge its issuance.
pub const CLIENT_URI_SCHEME: &str = "mycelium-client://";

/// The URI a client certificate names its principal by: `mycelium-client://{issuer}/{name}`.
pub fn client_uri(issuer: &str, name: &str) -> String {
    format!("{CLIENT_URI_SCHEME}{issuer}/{name}")
}

/// Whether `s` may be one segment of a client principal (its issuer or its name): 1–128 bytes of ASCII
/// letters, digits, `-`, `_` and `.`, and not `.` or `..` — so a principal is exactly two segments and
/// prints the same in a URI, a file name and a KV key.
pub fn valid_client_segment(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s != "."
        && s != ".."
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// What a client certificate says about whose key it is (identity-and-lifecycle I1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientIdentity {
    /// The gateway identity issuer the principal is qualified by (`gateway_identity_issuer`).
    pub issuer: String,
    /// The service's name under that issuer.
    pub name: String,
    /// The certificate's Ed25519 key — the key the service signs with.
    pub key: [u8; 32],
    /// The issuance time carried in the serial (ms since the epoch), as for a node certificate.
    pub issued_ms: Option<u64>,
}

impl ClientIdentity {
    /// The gateway principal this certificate's key speaks for: `token:{issuer}/{name}`.
    pub fn principal(&self) -> String {
        format!("token:{}/{}", self.issuer, self.name)
    }
}

/// Client names no certificate may carry, because the gateway already gives `token:{issuer}/{name}` to another
/// credential: `legacy` is the legacy `gateway_auth_token`'s principal (#741's review, item 7). A positional token
/// (`#{i}`) cannot collide: `#` is not a segment character.
pub const RESERVED_CLIENT_NAMES: &[&str] = &["legacy"];

/// Read a client certificate's [`ClientIdentity`]: exactly one `mycelium-client://{issuer}/{name}` URI
/// with two valid segments, a name not in [`RESERVED_CLIENT_NAMES`], and no `mycelium://` (node) or
/// `mycelium-consent://` (I7) URI — a certificate naming both is no client's, no node's and no consent's. `None` otherwise. Says nothing about who issued it: judge
/// the chain with [`NodeTls::verify_member_cert`] first. Never panics.
#[cfg(feature = "tls")]
pub fn client_identity(der: &[u8]) -> Option<ClientIdentity> {
    let id = cert_identity(der)?;
    // A node's name, or a consent principal's (identity-and-lifecycle I7, H1), beside the client's: no one's.
    if id.node_uris.iter().any(|u| u.starts_with("mycelium://") || u.starts_with(CONSENT_URI_SCHEME)) {
        return None;
    }
    let mut clients = id.node_uris.iter().filter_map(|u| u.strip_prefix(CLIENT_URI_SCHEME));
    let only = clients.next()?;
    if clients.next().is_some() {
        return None;
    }
    let (issuer, name) = only.split_once('/')?;
    if !valid_client_segment(issuer) || !valid_client_segment(name) || RESERVED_CLIENT_NAMES.contains(&name) {
        return None;
    }
    Some(ClientIdentity { issuer: issuer.to_string(), name: name.to_string(), key: id.key, issued_ms: id.issued_ms })
}

/// Read a certificate's [`CertIdentity`]. The key is read by the X.509 parser rustls uses
/// (`ed25519_key_from_cert_der`); the serial and the subject alternative names by a minimal,
/// bounds-checked DER walk of the `TBSCertificate` (the parser exposes neither). `None` for anything
/// that is not a certificate carrying an Ed25519 key. Says nothing about who issued it: judge the
/// chain with [`NodeTls::verify_member_cert`] first. Never panics.
#[cfg(feature = "tls")]
pub fn cert_identity(der: &[u8]) -> Option<CertIdentity> {
    let key = imp::ed25519_key_from_cert_der(der)?;
    let walk = tbs_walk(der)?;
    Some(CertIdentity { key, node_uris: walk.uris, issued_ms: walk.issued_ms })
}

/// What the hand walk of a `TBSCertificate` reads: the serial as an issuance time, the URIs among the subject
/// alternative names, and the value of the consent grade extension ([`CONSENT_GRADE_OID`]), if present.
#[cfg(feature = "tls")]
struct TbsWalk {
    issued_ms: Option<u64>,
    uris: Vec<String>,
    /// `Some(value)` for exactly one grade extension (the `extnValue` OCTET STRING's contents); `None` for none.
    grade_ext: Option<Vec<u8>>,
}

/// The bounds-checked DER walk [`cert_identity`] and [`consent_identity`] share. `None` for anything that is not a
/// certificate this walk can read, and for one carrying the grade extension twice (RFC 5280 §4.2: an extension
/// appears at most once — a reader that took the first or the last would let the issuer's encoder decide the grade).
/// Never panics.
#[cfg(feature = "tls")]
fn tbs_walk(der: &[u8]) -> Option<TbsWalk> {
    let grade_oid = consent_grade_oid_der();
    let (tag, cert, _) = der_tlv(der)?;
    if tag != 0x30 { return None; }
    let (tag, tbs, _) = der_tlv(cert)?;
    if tag != 0x30 { return None; }
    let mut rest = tbs;
    let (mut tag, mut body, mut next) = der_tlv(rest)?;
    if tag == 0xA0 {
        // [0] EXPLICIT version
        rest = next;
        (tag, body, next) = der_tlv(rest)?;
    }
    if tag != 0x02 { return None; }
    let issued_ms = {
        let mut b = body;
        while b.len() > 1 && b[0] == 0 { b = &b[1..]; }
        if b.is_empty() || b.len() > 8 || b[0] & 0x80 != 0 {
            None
        } else {
            let mut v = 0u64;
            for &x in b { v = (v << 8) | x as u64; }
            (v > 0).then_some(v)
        }
    };
    rest = next;
    // signature, issuer, validity, subject, subjectPublicKeyInfo
    for _ in 0..5 {
        let (_, _, n) = der_tlv(rest)?;
        rest = n;
    }
    let mut uris = Vec::new();
    let mut grade_ext: Option<Vec<u8>> = None;
    while !rest.is_empty() {
        let (tag, body, n) = der_tlv(rest)?;
        rest = n;
        if tag != 0xA3 { continue; }
        // [3] EXPLICIT Extensions ::= SEQUENCE OF Extension
        let (tag, mut exts, _) = der_tlv(body)?;
        if tag != 0x30 { return None; }
        while !exts.is_empty() {
            let (tag, ext, n) = der_tlv(exts)?;
            exts = n;
            if tag != 0x30 { return None; }
            let (tag, oid, mut e) = der_tlv(ext)?;
            if tag != 0x06 { return None; }
            let san = oid == [0x55, 0x1D, 0x11]; // 2.5.29.17 subjectAltName
            let grade = oid == grade_oid.as_slice();
            if !san && !grade { continue; }
            let (mut tag, mut value, mut n2) = der_tlv(e)?;
            if tag == 0x01 {
                // critical BOOLEAN
                e = n2;
                (tag, value, n2) = der_tlv(e)?;
            }
            let _ = n2;
            if tag != 0x04 { return None; }
            if grade {
                if grade_ext.is_some() { return None; }
                grade_ext = Some(value.to_vec());
                continue;
            }
            let (tag, mut names, _) = der_tlv(value)?;
            if tag != 0x30 { return None; }
            while !names.is_empty() {
                let (tag, name, n3) = der_tlv(names)?;
                names = n3;
                if tag == 0x86 {
                    // [6] IMPLICIT IA5String: uniformResourceIdentifier
                    if let Ok(uri) = std::str::from_utf8(name) { uris.push(uri.to_string()); }
                }
            }
        }
    }
    Some(TbsWalk { issued_ms, uris, grade_ext })
}

/// The scheme a **consent** certificate names its principal by (identity-and-lifecycle I7, H1):
/// `mycelium-consent://{issuer}/{name}` — the principal `consent:{issuer}/{name}`, beside (never instead of) the
/// service's `token:{issuer}/{name}`. A consent certificate is the fleet CA's statement over a developer's **P-256**
/// public key alone (`mycelium tls issue --client <name> --consent`): the key decides grants, and never calls, never
/// holds a grant and never joins the mesh. The scheme is bound to the key kind — a `mycelium-consent://` certificate
/// over an Ed25519 key, or one also naming `mycelium-client://` or `mycelium://`, is no consent certificate.
pub const CONSENT_URI_SCHEME: &str = "mycelium-consent://";

/// The URI a consent certificate names its principal by: `mycelium-consent://{issuer}/{name}`.
pub fn consent_uri(issuer: &str, name: &str) -> String {
    format!("{CONSENT_URI_SCHEME}{issuer}/{name}")
}

/// **PROVISIONAL — no certificate carrying it may ship.** The arcs of the consent grade extension's OID
/// (identity-and-lifecycle I7, H2): a non-critical X.509 extension whose value is a DER `UTF8String` holding the
/// grade's exact text ([`ConsentGrade::as_str`]).
///
/// The row planned a `2.25` UUID OID; rcgen's `CustomExtension::from_oid_content` takes `u64` arcs and cannot write a
/// 128-bit UUID arc. The maintainer decided (2026-11-15) that such identifiers go under a **registered IANA Private
/// Enterprise Number** arc, which is requested and not yet held. Until it is, this constant sits under PEN **32473**,
/// the number IANA reserves for documentation (RFC 5612) — so it can collide with no one's real extension — and the
/// issuer refuses to write it outside a test build ([`CONSENT_GRADE_OID_IS_PROVISIONAL`]). A release is blocked on
/// replacing it: the OID is permanent once a certificate carries it.
pub const CONSENT_GRADE_OID: &[u64] = &[1, 3, 6, 1, 4, 1, 32473, 7, 1];

/// Whether [`CONSENT_GRADE_OID`] is still the provisional documentation-PEN arc. While it is, a build that is not a
/// test build ([`consent_grade_oid_open`]) neither issues nor reads a consent certificate: the issuer refuses, and
/// [`consent_identity`] reads none (#800's review, finding 1 — anyone holding the CA key can mint one with openssl, so
/// an ungated reader would give a placeholder OID standing in a shipped node). `scripts/check-release-blockers.sh
/// --release`, run by the release workflow on a tag push, refuses a release while this is `true`.
pub const CONSENT_GRADE_OID_IS_PROVISIONAL: bool = true;

/// Whether this build may issue and read a consent certificate under [`CONSENT_GRADE_OID`]: always once the OID is
/// registered; while it is provisional, only in a test build — `cfg(test)` (mycelium-core's own tests) or the
/// non-default feature `provisional-consent-grade`, which only the test lines that exercise consent certificates turn
/// on (never `test-support`, an example, a Docker image or a shipped binary: #800's review, finding 2).
pub fn consent_grade_oid_open() -> bool {
    consent_grade_oid_open_when(CONSENT_GRADE_OID_IS_PROVISIONAL, provisional_build_open())
}

/// The rule [`consent_grade_oid_open`] applies: a registered OID is open to every build; a provisional one only to a
/// test build.
pub fn consent_grade_oid_open_when(provisional: bool, test_build: bool) -> bool {
    !provisional || test_build
}

#[cfg(any(test, feature = "provisional-consent-grade"))]
thread_local! {
    static PROVISIONAL_GATE_CLOSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn provisional_build_open() -> bool {
    #[cfg(any(test, feature = "provisional-consent-grade"))]
    {
        !PROVISIONAL_GATE_CLOSED.with(|c| c.get())
    }
    #[cfg(not(any(test, feature = "provisional-consent-grade")))]
    {
        false
    }
}

/// Run `f` on this thread as a shipped build would: the provisional gate closed (#800's review, finding 1's tests).
/// Only in a build that has the gate open to close.
#[cfg(any(test, feature = "provisional-consent-grade"))]
#[doc(hidden)]
pub fn with_provisional_consent_grade_closed<R>(f: impl FnOnce() -> R) -> R {
    struct Reopen;
    impl Drop for Reopen {
        fn drop(&mut self) {
            PROVISIONAL_GATE_CLOSED.with(|c| c.set(false));
        }
    }
    PROVISIONAL_GATE_CLOSED.with(|c| c.set(true));
    let _reopen = Reopen;
    f()
}

/// Whether `point` is a point of P-256 other than the identity, as SEC 1 decodes an uncompressed point (`04 ‖ x ‖
/// y`, `y² = x³ − 3x + b mod p`): #800's review, finding 4 — the fleet CA must never vouch for a key no device can
/// hold, which as a consent principal's newest certificate would leave the person's real key `Superseded`.
#[cfg(feature = "tls")]
pub fn p256_point_on_curve(point: &[u8; 65]) -> bool {
    point[0] == 0x04 && p256::PublicKey::from_sec1_bytes(point).is_ok()
}

/// **The one P-256 verifier** (identity-and-lifecycle I7, P2): whether `sig` — an ASN.1 DER ECDSA signature, as
/// CryptoKit's `derRepresentation` and `ring`'s `ECDSA_P256_SHA256_ASN1_SIGNING` write it — is `point`'s over the
/// SHA-256 of `msg`. It sits beside [`verify_bytes`], which stays Ed25519-only, and is called where a consent record
/// is checked (a signed grant decision) and nowhere else.
///
/// **`false` in every build where [`consent_grade_oid_open`] is false**: a shipped build neither issues nor reads a
/// consent certificate while the grade OID is provisional, so it verifies no consent signature either — there is no
/// ungated P-256 verification in this tree. `false` too for a point off the curve. The encoding is not canonical in
/// the ECDSA sense (`(r, n − s)` verifies where `(r, s)` does); nothing keys on a decision's signature bytes.
#[cfg(feature = "tls")]
pub fn verify_consent_p256(point: &[u8; 65], msg: &[u8], sig_der: &[u8]) -> bool {
    consent_grade_oid_open()
        && p256_point_on_curve(point)
        && ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_ASN1, point).verify(msg, sig_der).is_ok()
}

/// Sign `msg` with the P-256 key in `pkcs8` (DER), as a consent key signs: ASN.1 DER ECDSA over SHA-256. **A test
/// build's fixture only** (`cfg(test)` or `provisional-consent-grade`): `dev`'s signer is I7's PR 3, and no shipped
/// build holds this. `None` for a key that does not parse.
#[cfg(all(feature = "tls", any(test, feature = "provisional-consent-grade")))]
#[doc(hidden)]
pub fn sign_consent_p256_in_test_build(pkcs8: &[u8], msg: &[u8]) -> Option<Vec<u8>> {
    let rng = ring::rand::SystemRandom::new();
    let key = ring::signature::EcdsaKeyPair::from_pkcs8(&ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8, &rng).ok()?;
    key.sign(&rng, msg).ok().map(|s| s.as_ref().to_vec())
}

/// The DER content octets of [`CONSENT_GRADE_OID`] (base-128 arcs, the first two folded): what the walk compares an
/// extension's `extnID` against. Pinned in a golden (`consent_tests`).
pub fn consent_grade_oid_der() -> Vec<u8> {
    oid_der(CONSENT_GRADE_OID)
}

/// The DER content octets of an OID given by its arcs (base-128, the first two folded).
fn oid_der(arcs: &[u64]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut push = |mut v: u64| {
        let mut tmp = [0u8; 10];
        let mut i = tmp.len();
        i -= 1;
        tmp[i] = (v & 0x7F) as u8;
        v >>= 7;
        while v > 0 {
            i -= 1;
            tmp[i] = 0x80 | (v & 0x7F) as u8;
            v >>= 7;
        }
        out.extend_from_slice(&tmp[i..]);
    };
    push(arcs[0] * 40 + arcs[1]);
    for &a in &arcs[2..] {
        push(a);
    }
    out
}

// ── The CA generation (identity-and-lifecycle I2, R1) ─────────────────────────────────────────────

/// **PROVISIONAL — no certificate carrying it may ship.** The arcs of the `caGeneration` extension's OID
/// (identity-and-lifecycle I2, R1): a non-critical extension on a **CA** certificate whose value is a DER `INTEGER`,
/// the CA's generation — `tls ca` mints generation 0 (and writes no extension: a CA certificate without it, which
/// covers every CA before I2, is generation 0), and a successor is its predecessor's generation plus one.
///
/// The row planned a `2.25` UUID OID, which rcgen cannot write (its arcs are `u64`). The maintainer decided
/// (2026-11-15, decision (e)) that such identifiers go under a **registered IANA Private Enterprise Number** arc, which
/// is requested and not yet held; I7's grade took the first sub-arc of the documentation PEN **32473** (RFC 5612),
/// `32473.7`, and this takes its own, `32473.2`. Until the registered arc arrives, a build that is not a test build
/// neither writes nor reads it ([`CA_GENERATION_OID_IS_PROVISIONAL`]) and a release is blocked on replacing it
/// (`scripts/check-release-blockers.sh`): the OID is permanent once a certificate carries it.
pub const CA_GENERATION_OID: &[u64] = &[1, 3, 6, 1, 4, 1, 32473, 2, 1];

/// Whether [`CA_GENERATION_OID`] is still the provisional documentation-PEN arc. While it is, a build that is not a
/// test build ([`ca_generation_oid_open`]) mints no CA carrying it and reads none: a root carrying it is refused by name
/// at start, at a change of roots and by the issuer ([`ca_generation`] answers [`CaGenerationError::Provisional`]),
/// never read as generation 0 — which would leave legacy open on a node that trusts only a successor (R1). Anyone
/// holding a CA key can write the extension with openssl, so the gate is on the reader as well as the writer (I7's
/// #800 review, finding 1, applied here).
pub const CA_GENERATION_OID_IS_PROVISIONAL: bool = true;

/// Whether this build may write and read [`CA_GENERATION_OID`]: always once it is registered; while it is
/// provisional, only `cfg(test)` (mycelium-core's own tests) or the non-default feature `provisional-ca-generation`,
/// which only the test lines that exercise a succession turn on (never a shipped binary, an example or an image).
pub fn ca_generation_oid_open() -> bool {
    consent_grade_oid_open_when(CA_GENERATION_OID_IS_PROVISIONAL, provisional_ca_generation_build_open())
}

#[cfg(any(test, feature = "provisional-ca-generation"))]
thread_local! {
    static PROVISIONAL_CA_GENERATION_CLOSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn provisional_ca_generation_build_open() -> bool {
    #[cfg(any(test, feature = "provisional-ca-generation"))]
    {
        !PROVISIONAL_CA_GENERATION_CLOSED.with(|c| c.get())
    }
    #[cfg(not(any(test, feature = "provisional-ca-generation")))]
    {
        false
    }
}

/// Run `f` on this thread as a shipped build would: the `caGeneration` gate closed (the I2 tests of the shipped
/// refusal). Only in a build that has the gate open to close.
#[cfg(any(test, feature = "provisional-ca-generation"))]
#[doc(hidden)]
pub fn with_provisional_ca_generation_closed<R>(f: impl FnOnce() -> R) -> R {
    struct Reopen;
    impl Drop for Reopen {
        fn drop(&mut self) {
            PROVISIONAL_CA_GENERATION_CLOSED.with(|c| c.set(false));
        }
    }
    PROVISIONAL_CA_GENERATION_CLOSED.with(|c| c.set(true));
    let _reopen = Reopen;
    f()
}

/// The DER content octets of [`CA_GENERATION_OID`]. Pinned in a golden (`ca_generation_tests`).
pub fn ca_generation_oid_der() -> Vec<u8> {
    oid_der(CA_GENERATION_OID)
}

/// The `extnValue` contents [`CA_GENERATION_OID`] carries for `generation`: a DER `INTEGER`, minimal, non-negative.
pub fn ca_generation_extension_der(generation: u64) -> Vec<u8> {
    let bytes = generation.to_be_bytes();
    let first = bytes.iter().position(|&b| b != 0).unwrap_or(7);
    let mut content: Vec<u8> = Vec::with_capacity(9);
    if bytes[first] & 0x80 != 0 {
        content.push(0);
    }
    content.extend_from_slice(&bytes[first..]);
    let mut out = vec![0x02, content.len() as u8];
    out.extend_from_slice(&content);
    out
}

/// Why a CA certificate's generation cannot be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaGenerationError {
    /// The certificate cannot be walked, carries the extension twice, or its value is not one minimal non-negative
    /// DER `INTEGER` that fits a `u64`. Never read as 0 (R1: a successor read as generation 0 would leave legacy open).
    Malformed,
    /// The extension is present under the provisional OID and this is no test build ([`ca_generation_oid_open`]).
    Provisional,
}

impl std::fmt::Display for CaGenerationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaGenerationError::Malformed => f.write_str(
                "its caGeneration extension is malformed (present twice, or not one minimal non-negative DER INTEGER)",
            ),
            CaGenerationError::Provisional => f.write_str(
                "it carries the caGeneration extension under the provisional OID 1.3.6.1.4.1.32473.2.1, which this build \
                 does not read until the registered IANA PEN arc replaces it (identity-and-lifecycle I2)",
            ),
        }
    }
}

/// A CA certificate's **generation** (identity-and-lifecycle I2, R1): `Ok(0)` when it carries no `caGeneration`
/// extension (every CA before I2, and every CA `tls ca` mints), `Ok(n)` for one well-formed extension, and an error —
/// never 0 — for a malformed one or, outside a test build, one under the provisional OID. Never panics.
#[cfg(feature = "tls")]
pub fn ca_generation(der: &[u8]) -> Result<u64, CaGenerationError> {
    let oid = ca_generation_oid_der();
    let values = tbs_extension_values(der, &oid).ok_or(CaGenerationError::Malformed)?;
    let value = match values.as_slice() {
        [] => return Ok(0),
        [one] => *one,
        _ => return Err(CaGenerationError::Malformed),
    };
    if !ca_generation_oid_open() {
        return Err(CaGenerationError::Provisional);
    }
    let (tag, body, rest) = der_tlv(value).ok_or(CaGenerationError::Malformed)?;
    if tag != 0x02 || !rest.is_empty() || body.is_empty() || body[0] & 0x80 != 0 {
        return Err(CaGenerationError::Malformed);
    }
    // Minimal: a leading zero only before a byte whose high bit is set.
    if body.len() > 1 && body[0] == 0 && body[1] & 0x80 == 0 {
        return Err(CaGenerationError::Malformed);
    }
    let digits = if body[0] == 0 { &body[1..] } else { body };
    if digits.len() > 8 {
        return Err(CaGenerationError::Malformed);
    }
    Ok(digits.iter().fold(0u64, |v, &b| (v << 8) | b as u64))
}

/// Whether a node trusting exactly `roots` has **no legacy class** (identity-and-lifecycle I2, R1, the maintainer's
/// decision of 2026-11-12): every root is generation 1 or later. Derived from the roots alone, never from
/// `succession/` (#768's H2): a node recovered by the operator's procedure, and one first started on a successor's
/// files, have no `succession/` and are closed all the same. A root whose generation cannot be read is refused before
/// this is asked (at load, [`NodeTls::reroot_material`]); here it counts as generation 0, so an unreadable root never
/// closes anything it should not.
#[cfg(feature = "tls")]
pub fn roots_close_legacy(roots: &[Vec<u8>]) -> bool {
    !roots.is_empty() && roots.iter().all(|r| ca_generation(r).is_ok_and(|g| g >= 1))
}

/// Every `extnValue` content (the OCTET STRING's contents) under `oid_der` in a certificate's extensions, in order.
/// `None` for anything that is not a certificate the bounded walk can read. Never panics.
#[cfg(feature = "tls")]
fn tbs_extension_values<'a>(der: &'a [u8], oid_der: &[u8]) -> Option<Vec<&'a [u8]>> {
    let (tag, cert, _) = der_tlv(der)?;
    if tag != 0x30 { return None; }
    let (tag, tbs, _) = der_tlv(cert)?;
    if tag != 0x30 { return None; }
    let mut rest = tbs;
    let (tag, _, next) = der_tlv(rest)?;
    if tag == 0xA0 {
        rest = next; // [0] EXPLICIT version
    }
    // serial, signature, issuer, validity, subject, subjectPublicKeyInfo
    for _ in 0..6 {
        let (_, _, n) = der_tlv(rest)?;
        rest = n;
    }
    let mut out = Vec::new();
    while !rest.is_empty() {
        let (tag, body, n) = der_tlv(rest)?;
        rest = n;
        if tag != 0xA3 { continue; }
        let (tag, mut exts, _) = der_tlv(body)?;
        if tag != 0x30 { return None; }
        while !exts.is_empty() {
            let (tag, ext, n) = der_tlv(exts)?;
            exts = n;
            if tag != 0x30 { return None; }
            let (tag, oid, mut e) = der_tlv(ext)?;
            if tag != 0x06 { return None; }
            if oid != oid_der { continue; }
            let (mut tag, mut value, mut n2) = der_tlv(e)?;
            if tag == 0x01 {
                e = n2;
                (tag, value, n2) = der_tlv(e)?;
            }
            let _ = n2;
            if tag != 0x04 { return None; }
            out.push(value);
        }
    }
    Some(out)
}

/// The grade a consent certificate carries (identity-and-lifecycle I7, P1, P4 revised by H2, P7): **the issuer's
/// statement of what the enrolment told it** about the key, signed by the fleet CA. It is not a property any verifier
/// can check — a verifier holding the public key cannot tell an Enclave key with a presence flag from a software key
/// (S-presence) — and nothing in this tree reads it as one: a grade names where the enrolling device said the key
/// lives, and the operator who ran `tls issue --grade` vouches for having been told it.
///
/// `service` is not a consent grade (it is the grade a `token:` decider's record carries, I7's later PR), and the
/// grades are not ordered: a policy accepts a **set** of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ConsentGrade {
    /// `os_presence(macos, unbound)`: the enrolment said the key is a Secure Enclave key created with
    /// `biometryCurrentSet` (P1) — an enrolment claim, unverifiable, and *unbound*: the prompt's text is the signing
    /// process's to choose (P5).
    OsPresenceMacosUnbound,
    /// `tty`: the enrolment said the key is a software P-256 key in a same-user file (P7) — any process running as the
    /// user can sign with it, so a `tty` decision may have been made by the agent whose request it approves (B).
    Tty,
    /// `recording`: the enrolment said the key is a `sim` test build's (E6's walk), with no person involved — the
    /// operator's statement, as for every grade.
    Recording,
}

impl ConsentGrade {
    /// Every consent grade, in a fixed order.
    pub const ALL: [ConsentGrade; 3] = [ConsentGrade::OsPresenceMacosUnbound, ConsentGrade::Tty, ConsentGrade::Recording];

    /// The grade's exact text, as the extension carries it and a record names it.
    pub fn as_str(self) -> &'static str {
        match self {
            ConsentGrade::OsPresenceMacosUnbound => "os_presence(macos, unbound)",
            ConsentGrade::Tty => "tty",
            ConsentGrade::Recording => "recording",
        }
    }

    /// The grade named by exactly `s`; `None` for anything else, `service` included.
    pub fn parse(s: &str) -> Option<ConsentGrade> {
        ConsentGrade::ALL.into_iter().find(|g| g.as_str() == s)
    }

    /// The extension's value: a DER `UTF8String` holding [`as_str`](Self::as_str).
    pub fn extension_der(self) -> Vec<u8> {
        let text = self.as_str().as_bytes();
        let mut out = vec![0x0C, text.len() as u8];
        out.extend_from_slice(text);
        out
    }

    /// The grade an extension value names: exactly one `UTF8String`, nothing after it, holding a grade's text.
    pub fn from_extension_der(value: &[u8]) -> Option<ConsentGrade> {
        #[cfg(feature = "tls")]
        {
            let (tag, text, rest) = der_tlv(value)?;
            if tag != 0x0C || !rest.is_empty() { return None; }
            ConsentGrade::parse(std::str::from_utf8(text).ok()?)
        }
        #[cfg(not(feature = "tls"))]
        {
            let _ = value;
            None
        }
    }
}

impl std::fmt::Display for ConsentGrade {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a consent certificate says about whose key it is (identity-and-lifecycle I7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsentIdentity {
    /// The gateway identity issuer the principal is qualified by (`gateway_identity_issuer`).
    pub issuer: String,
    /// The person's name under that issuer.
    pub name: String,
    /// The certificate's P-256 key: the uncompressed X9.63 point (`04 ‖ x ‖ y`), 65 bytes.
    pub point: [u8; 65],
    /// The grade the fleet CA wrote at enrolment.
    pub grade: ConsentGrade,
    /// The issuance time carried in the serial (ms since the epoch), as for a client certificate.
    pub issued_ms: Option<u64>,
}

impl ConsentIdentity {
    /// The principal this certificate's key decides for: `consent:{issuer}/{name}`.
    pub fn principal(&self) -> String {
        format!("consent:{}/{}", self.issuer, self.name)
    }
}

/// Read a consent certificate's [`ConsentIdentity`]: a **P-256** key (`id-ecPublicKey` over `prime256v1`, an
/// uncompressed point), exactly one `mycelium-consent://{issuer}/{name}` URI with two valid segments and a name not in
/// [`RESERVED_CLIENT_NAMES`], no `mycelium://` or `mycelium-client://` URI beside it, and exactly one grade extension
/// naming a [`ConsentGrade`], over a point on the curve. `None` otherwise — **no grade, no consent** — and `None` in
/// every build where [`consent_grade_oid_open`] is false, so a shipped node fails closed on a certificate carrying the
/// provisional OID ([`consent_identity_closed_by_provisional_oid`] tells that case apart, for a counter and a named
/// refusal). Says nothing about who issued it: judge the chain with [`NodeTls::verify_member_cert`] first. Never
/// panics.
#[cfg(feature = "tls")]
pub fn consent_identity(der: &[u8]) -> Option<ConsentIdentity> {
    if !consent_grade_oid_open() {
        return None;
    }
    consent_identity_ungated(der)
}

/// Whether `der` is a consent certificate this build does not read **only** because the grade OID is provisional and
/// this is not a test build (#800's review, finding 1): what the fold counts and the door refuses by name.
#[cfg(feature = "tls")]
pub fn consent_identity_closed_by_provisional_oid(der: &[u8]) -> bool {
    !consent_grade_oid_open() && consent_identity_ungated(der).is_some()
}

#[cfg(feature = "tls")]
fn consent_identity_ungated(der: &[u8]) -> Option<ConsentIdentity> {
    let point = imp::p256_point_from_cert_der(der)?;
    let walk = tbs_walk(der)?;
    if walk.uris.iter().any(|u| u.starts_with("mycelium://") || u.starts_with(CLIENT_URI_SCHEME)) {
        return None;
    }
    let mut named = walk.uris.iter().filter_map(|u| u.strip_prefix(CONSENT_URI_SCHEME));
    let only = named.next()?;
    if named.next().is_some() {
        return None;
    }
    let (issuer, name) = only.split_once('/')?;
    if !valid_client_segment(issuer) || !valid_client_segment(name) || RESERVED_CLIENT_NAMES.contains(&name) {
        return None;
    }
    let grade = ConsentGrade::from_extension_der(walk.grade_ext.as_deref()?)?;
    Some(ConsentIdentity { issuer: issuer.to_string(), name: name.to_string(), point, grade, issued_ms: walk.issued_ms })
}

/// The fixed DER header of a P-256 `SubjectPublicKeyInfo` over an uncompressed point (RFC 5480):
/// `SEQUENCE { SEQUENCE { id-ecPublicKey, prime256v1 }, BIT STRING (0 unused bits) }`, followed by the 65-byte point.
pub const P256_SPKI_HEADER: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x02, 0x01, 0x06, 0x08, 0x2A, 0x86, 0x48, 0xCE,
    0x3D, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];

/// The uncompressed point of a P-256 SPKI PEM (`-----BEGIN PUBLIC KEY-----`), as the developer's side exports it
/// (identity-and-lifecycle I7's issue flow: CryptoKit's `pemRepresentation`, or [`p256_spki_pem`]). Refused, by name:
/// no `PUBLIC KEY` block, or more than one; an SPKI for another algorithm or curve (an Ed25519 key among them); a
/// compressed point; a point not on the curve (#800's review, finding 4). **Raw X9.63 is not accepted**, so the input
/// names its own algorithm.
#[cfg(feature = "tls")]
pub fn p256_point_from_spki_pem(pem: &str) -> Result<[u8; 65], String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    const BEGIN: &str = "-----BEGIN PUBLIC KEY-----";
    const END: &str = "-----END PUBLIC KEY-----";
    if pem.matches(BEGIN).count() != 1 || pem.matches(END).count() != 1 {
        return Err("not_spki_pem: expected exactly one -----BEGIN PUBLIC KEY----- block (raw X9.63 is not accepted)".into());
    }
    if pem.contains("-----BEGIN") && pem.matches("-----BEGIN").count() != 1 {
        return Err("not_spki_pem: expected exactly one PEM block".into());
    }
    let start = pem.find(BEGIN).expect("counted") + BEGIN.len();
    let end = pem.find(END).expect("counted");
    if end < start {
        return Err("not_spki_pem: the END line precedes the BEGIN line".into());
    }
    let b64: String = pem[start..end].chars().filter(|c| !c.is_whitespace()).collect();
    let der = STANDARD.decode(b64).map_err(|e| format!("not_spki_pem: the block is not base64: {e}"))?;
    let (header, point) = der
        .split_at_checked(P256_SPKI_HEADER.len())
        .ok_or_else(|| "not_p256: the SPKI is shorter than a P-256 key's".to_string())?;
    if header != P256_SPKI_HEADER || point.len() != 65 || point[0] != 0x04 {
        return Err("not_p256: the SPKI is not a P-256 (prime256v1) key over an uncompressed point".into());
    }
    let point: [u8; 65] = point.try_into().expect("checked length");
    if !p256_point_on_curve(&point) {
        return Err("not_p256: the point is not on the curve".into());
    }
    Ok(point)
}

/// The SPKI PEM of a P-256 uncompressed point — the fixed header, then the point — as `dev` exports a software
/// consent key's public half (`ring`'s `EcdsaKeyPair::public_key` is this point).
pub fn p256_spki_pem(point: &[u8; 65]) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let mut der = P256_SPKI_HEADER.to_vec();
    der.extend_from_slice(point);
    let b64 = STANDARD.encode(der);
    let mut pem = String::from("-----BEGIN PUBLIC KEY-----\n");
    for chunk in b64.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(chunk).expect("base64 is ascii"));
        pem.push('\n');
    }
    pem.push_str("-----END PUBLIC KEY-----\n");
    pem
}

/// One DER TLV: `(tag, content, rest)`. Single-byte tags, definite lengths of at most four length
/// bytes; `None` on anything else or on a length past the input.
#[cfg(feature = "tls")]
fn der_tlv(input: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = input.split_first()?;
    if tag & 0x1F == 0x1F { return None; }
    let (&first, mut rest) = rest.split_first()?;
    let len = if first < 0x80 {
        first as usize
    } else {
        let n = (first & 0x7F) as usize;
        if n == 0 || n > 4 || rest.len() < n { return None; }
        let mut l = 0usize;
        for &b in &rest[..n] { l = (l << 8) | b as usize; }
        rest = &rest[n..];
        l
    };
    if rest.len() < len { return None; }
    Some((tag, &rest[..len], &rest[len..]))
}

/// Which name of a `TBSCertificate` [`tbs_name`] reads.
#[cfg(feature = "tls")]
#[derive(Clone, Copy)]
pub(crate) enum TbsName {
    Issuer,
    Subject,
}

/// The DER of a certificate's issuer or subject `Name` (the whole TLV), by the bounds-checked walk
/// [`cert_identity`] uses. `None` for anything that is not a certificate. Never panics.
#[cfg(feature = "tls")]
pub(crate) fn tbs_name(der: &[u8], which: TbsName) -> Option<&[u8]> {
    let (tag, cert, _) = der_tlv(der)?;
    if tag != 0x30 { return None; }
    let (tag, tbs, _) = der_tlv(cert)?;
    if tag != 0x30 { return None; }
    let mut rest = tbs;
    let (tag, _, next) = der_tlv(rest)?;
    if tag == 0xA0 {
        rest = next; // [0] EXPLICIT version
    }
    // serial, signature, then issuer; validity, then subject
    let skip = match which { TbsName::Issuer => 2, TbsName::Subject => 4 };
    for _ in 0..skip {
        let (_, _, n) = der_tlv(rest)?;
        rest = n;
    }
    let (tag, _, after) = der_tlv(rest)?;
    if tag != 0x30 { return None; }
    Some(&rest[..rest.len() - after.len()])
}

/// A certificate's issuer name (the DER `Name`), read by the bounded walk; `None` for anything that is not a
/// certificate. What a kept anchor tells its issuers apart by (identity-and-lifecycle I2, R4). Never panics.
#[cfg(feature = "tls")]
pub fn cert_issuer_der(der: &[u8]) -> Option<Vec<u8>> {
    tbs_name(der, TbsName::Issuer).map(<[u8]>::to_vec)
}

/// A certificate's `notAfter`, in ms since the epoch, read by the bounded walk; `None` for anything that is not a
/// certificate or a time this walk cannot read. What a cached verdict carries so that a certificate's expiry is judged
/// in-process as a restart judges it (identity-and-lifecycle I2, #805's review F3). Never panics.
#[cfg(feature = "tls")]
pub fn cert_not_after_ms(der: &[u8]) -> Option<u64> {
    let (tag, cert, _) = der_tlv(der)?;
    if tag != 0x30 { return None; }
    let (tag, tbs, _) = der_tlv(cert)?;
    if tag != 0x30 { return None; }
    let mut rest = tbs;
    let (tag, _, next) = der_tlv(rest)?;
    if tag == 0xA0 {
        rest = next;
    }
    // serial, signature, issuer; then validity
    for _ in 0..3 {
        let (_, _, n) = der_tlv(rest)?;
        rest = n;
    }
    let (tag, validity, _) = der_tlv(rest)?;
    if tag != 0x30 { return None; }
    let (_, _, after_nb) = der_tlv(validity)?;
    let (tag, t, _) = der_tlv(after_nb)?;
    let t = std::str::from_utf8(t).ok()?;
    let (year, rest) = match tag {
        0x17 => {
            let yy: i64 = t.get(0..2)?.parse().ok()?;
            (if yy >= 50 { 1900 + yy } else { 2000 + yy }, t.get(2..)?)
        }
        0x18 => (t.get(0..4)?.parse().ok()?, t.get(4..)?),
        _ => return None,
    };
    if rest.len() != 11 || !rest.ends_with('Z') { return None; }
    let n = |i: usize| -> Option<i64> { rest.get(i..i + 2)?.parse().ok() };
    let (mo, d, h, mi, se) = (n(0)?, n(2)?, n(4)?, n(6)?, n(8)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 { return None; }
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if mo <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3_600 + mi * 60 + se;
    u64::try_from(secs).ok()?.checked_mul(1_000)
}

/// The commonName of a CA certificate's subject, when the subject is **exactly** what this tree writes:
/// one RDN holding one `commonName` (2.5.4.3) `UTF8String` — rcgen's default subject, which every CA
/// before identity-and-lifecycle I2 has, and the subject `mycelium tls ca` writes since. `None` for any
/// other subject, which this issuer cannot reproduce byte for byte (see `signing_ca`). Never panics.
#[cfg(feature = "tls")]
pub(crate) fn ca_subject_cn(ca_cert_der: &[u8]) -> Option<String> {
    let name = tbs_name(ca_cert_der, TbsName::Subject)?;
    let (_, rdns, rest) = der_tlv(name)?;
    if !rest.is_empty() { return None; }
    let (tag, set, rest) = der_tlv(rdns)?;
    if tag != 0x31 || !rest.is_empty() { return None; }
    let (tag, atv, rest) = der_tlv(set)?;
    if tag != 0x30 || !rest.is_empty() { return None; }
    let (tag, oid, value) = der_tlv(atv)?;
    if tag != 0x06 || oid != [0x55, 0x04, 0x03] { return None; }
    let (tag, cn, rest) = der_tlv(value)?;
    if tag != 0x0C || !rest.is_empty() { return None; }
    std::str::from_utf8(cn).ok().map(str::to_string)
}

/// The subject `mycelium tls ca` gives a fleet CA (identity-and-lifecycle I2): its commonName names the
/// fleet — a 64-bit id drawn when the CA is minted — and the CA's generation, so two fleets' CAs, and a
/// CA and its successor, never share a name. A node's own CA (no `tls ca`) keeps rcgen's default subject.
#[cfg(feature = "tls")]
pub fn fleet_ca_subject(fleet: u64, generation: u64) -> String {
    format!("Mycelium fleet {fleet:016x} CA generation {generation}")
}

// ── The CA's commitment to its successor (`ca-commit.pem`; identity-and-lifecycle I2, PR 2b) ─────────────────────

/// The file an operator places in a node's certificate directory (`{auto_cert_dir}/ca-commit.pem`): the old CA's
/// commitment to its successor, written by `mycelium tls commit`.
pub const CA_COMMIT_FILE: &str = "ca-commit.pem";
/// The file a node persists one pin in, under its own `{auto_cert_dir}/succession/`: **one file per committing CA**,
/// `pin-{predecessor}.pem`, `{predecessor}` the 64 lower-case hex digits of the SHA-256 of the committing CA's
/// certificate — the commitment's `predecessor` (the maintainer's decision of 2026-11-18: a pin is per predecessor
/// CA). To remove the pin a node holds for one CA, an operator removes that one file; the others are untouched.
pub fn ca_commit_pin_file(predecessor: &[u8; 32]) -> String {
    format!("{CA_COMMIT_PIN_PREFIX}{}{CA_COMMIT_PIN_SUFFIX}", predecessor.iter().map(|b| format!("{b:02x}")).collect::<String>())
}
/// The start of a pin file's name ([`ca_commit_pin_file`]).
pub const CA_COMMIT_PIN_PREFIX: &str = "pin-";
/// The end of a pin file's name ([`ca_commit_pin_file`]).
pub const CA_COMMIT_PIN_SUFFIX: &str = ".pem";
/// The node's own succession state directory, under `auto_cert_dir`.
pub const SUCCESSION_DIR: &str = "succession";
/// The PEM label of the commitment's signed section.
pub const CA_COMMIT_PEM_LABEL: &str = "MYCELIUM CA COMMITMENT";
/// What the old CA signs: this tag, the SHA-256 of its own certificate, the SHA-256 of the successor's.
#[cfg(feature = "tls")]
const CA_COMMIT_DOMAIN: &[u8] = b"mycelium/ca-commit/v1\0";
/// The signed section's one version.
#[cfg(feature = "tls")]
const CA_COMMIT_VERSION: u8 = 1;

/// **A CA's commitment to its successor** (identity-and-lifecycle I2; the maintainer's decision (a), 2026-11-15): the
/// **old CA's own promise**, signed with its key, that the CA whose certificate is carried here succeeds it. It names
/// the successor and nothing else — no window, no node, no instruction: what a node does about it (widen, cut over,
/// narrow) is each node's own act, decided by later records the successor signs and each node assesses against this.
///
/// A node assesses it against a root it already trusts ([`CaCommitment::verify`]) and pins it only from the file its
/// operator placed (the decision of 2026-11-16); a copy seen anywhere else is compared with the pin, never adopted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaCommitment {
    /// The SHA-256 of the committing (old) CA's certificate DER.
    pub predecessor: [u8; 32],
    /// The successor CA's certificate, DER.
    pub successor_der: Vec<u8>,
    /// The old CA's Ed25519 signature over the domain tag, `predecessor` and the SHA-256 of `successor_der`.
    pub sig: [u8; 64],
}

/// Why a commitment is not one this node can assess. Each is a refusal by name; none is ever read as "no commitment".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaCommitError {
    /// The document is not exactly one certificate and one commitment section of the one version, or carries a key.
    Malformed(String),
    /// No root this node trusts is the CA the commitment names as its signer.
    UnknownPredecessor,
    /// The named predecessor did not sign it (or is not an Ed25519 CA this tree mints).
    BadSignature,
    /// The committing CA's own generation cannot be read from the root this node trusts.
    PredecessorGeneration(CaGenerationError),
    /// The successor's generation cannot be read (malformed, or under the provisional OID in a shipped build).
    SuccessorGeneration(CaGenerationError),
    /// The successor is not its predecessor's generation plus one.
    WrongGeneration { predecessor: u64, successor: u64 },
    /// The successor carries the predecessor's subject or key: it is not a distinct CA.
    NotDistinct,
}

impl std::fmt::Display for CaCommitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaCommitError::Malformed(why) => write!(f, "the commitment is malformed: {why}"),
            CaCommitError::UnknownPredecessor => f.write_str("no root this node trusts is the CA the commitment names as its signer"),
            CaCommitError::BadSignature => f.write_str("the CA the commitment names did not sign it"),
            CaCommitError::PredecessorGeneration(why) => write!(f, "the committing CA's certificate is refused: {why}"),
            CaCommitError::SuccessorGeneration(why) => write!(f, "the successor's CA certificate is refused: {why}"),
            CaCommitError::WrongGeneration { predecessor, successor } => write!(
                f,
                "the successor is generation {successor}, and its predecessor is generation {predecessor}: a successor is its predecessor's generation plus one"
            ),
            CaCommitError::NotDistinct => f.write_str("the successor carries its predecessor's subject or key: a successor is a distinct CA"),
        }
    }
}

/// SHA-256, by the `ring` the TLS stack already carries.
#[cfg(feature = "tls")]
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(ring::digest::digest(&ring::digest::SHA256, data).as_ref());
    out
}

#[cfg(feature = "tls")]
impl CaCommitment {
    fn message(predecessor: &[u8; 32], successor_der: &[u8]) -> Vec<u8> {
        let mut m = Vec::with_capacity(CA_COMMIT_DOMAIN.len() + 64);
        m.extend_from_slice(CA_COMMIT_DOMAIN);
        m.extend_from_slice(predecessor);
        m.extend_from_slice(&sha256(successor_der));
        m
    }

    /// The old CA's act: sign a commitment to `successor_der` with the key of the CA whose certificate is
    /// `predecessor_der`. Refused unless the result verifies against that certificate — a key that is not the
    /// certificate's, a successor of the wrong generation or not distinct.
    pub fn sign(predecessor_der: &[u8], predecessor_key: &ed25519_dalek::SigningKey, successor_der: &[u8]) -> Result<Self, CaCommitError> {
        let predecessor = sha256(predecessor_der);
        let sig = sign_bytes(predecessor_key, &Self::message(&predecessor, successor_der));
        let c = CaCommitment { predecessor, successor_der: successor_der.to_vec(), sig };
        c.verify(&[predecessor_der.to_vec()])?;
        Ok(c)
    }

    /// What identifies the promise: the SHA-256 of the predecessor's digest and the successor certificate's. Two
    /// commitments with one digest are one promise; a node that pinned one refuses any other from the same CA.
    pub fn digest(&self) -> [u8; 32] {
        let mut both = self.predecessor.to_vec();
        both.extend_from_slice(&sha256(&self.successor_der));
        sha256(&both)
    }

    /// **The promisee's assessment**: `roots` are the CA certificates this node trusts. `Ok(generation)` — the
    /// successor's — when one of them is the named predecessor, its key signed this, and the successor is a distinct
    /// CA of the predecessor's generation plus one. Anything else is a named refusal.
    pub fn verify(&self, roots: &[Vec<u8>]) -> Result<u64, CaCommitError> {
        let root = roots
            .iter()
            .find(|r| sha256(r) == self.predecessor)
            .ok_or(CaCommitError::UnknownPredecessor)?;
        let key = ed25519_key_from_cert_der(root).ok_or(CaCommitError::BadSignature)?;
        if !verify_bytes(&key, &Self::message(&self.predecessor, &self.successor_der), &self.sig) {
            return Err(CaCommitError::BadSignature);
        }
        let before = ca_generation(root).map_err(CaCommitError::PredecessorGeneration)?;
        let after = ca_generation(&self.successor_der).map_err(CaCommitError::SuccessorGeneration)?;
        if before.checked_add(1) != Some(after) {
            return Err(CaCommitError::WrongGeneration { predecessor: before, successor: after });
        }
        let successor_key = ed25519_key_from_cert_der(&self.successor_der)
            .ok_or_else(|| CaCommitError::Malformed("the successor is not an Ed25519 CA certificate".into()))?;
        let name = |der: &[u8]| tbs_name(der, TbsName::Subject).map(<[u8]>::to_vec);
        if successor_key == key || name(&self.successor_der).is_none() || name(&self.successor_der) == name(root) {
            return Err(CaCommitError::NotDistinct);
        }
        Ok(after)
    }

    /// The `ca-commit.pem` document: the successor's certificate, then the signed section.
    pub fn to_pem(&self) -> String {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let mut body = Vec::with_capacity(97);
        body.push(CA_COMMIT_VERSION);
        body.extend_from_slice(&self.predecessor);
        body.extend_from_slice(&self.sig);
        let wrap = |label: &str, der: &[u8]| {
            let b64 = STANDARD.encode(der);
            let lines: Vec<&str> = b64.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).unwrap_or_default()).collect();
            format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n", lines.join("\n"))
        };
        format!("{}{}", wrap("CERTIFICATE", &self.successor_der), wrap(CA_COMMIT_PEM_LABEL, &body))
    }

    /// Read a `ca-commit.pem` document: exactly one certificate and exactly one commitment section, and no private
    /// key (what an operator copies to a node must carry none). Parsing proves nothing: [`CaCommitment::verify`] does.
    pub fn from_pem(pem: &str) -> Result<Self, CaCommitError> {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let bad = |why: &str| CaCommitError::Malformed(why.to_string());
        if pem.contains("PRIVATE KEY-----") {
            return Err(bad("it carries a private key; a commitment carries the successor's certificate only"));
        }
        let sections = |label: &str| -> Result<Vec<Vec<u8>>, CaCommitError> {
            let (begin, end) = (format!("-----BEGIN {label}-----"), format!("-----END {label}-----"));
            pem.split(begin.as_str())
                .skip(1)
                .map(|block| {
                    let (body, _) = block.split_once(end.as_str()).ok_or_else(|| bad("a section has no END line"))?;
                    let b64: String = body.split_whitespace().collect();
                    STANDARD.decode(b64).map_err(|_| bad("a section is not base64"))
                })
                .collect()
        };
        let certs = sections("CERTIFICATE")?;
        let commits = sections(CA_COMMIT_PEM_LABEL)?;
        let ([successor_der], [body]) = (certs.as_slice(), commits.as_slice()) else {
            return Err(bad("it must hold exactly one certificate (the successor's) and exactly one commitment section"));
        };
        if body.len() != 97 || body[0] != CA_COMMIT_VERSION {
            return Err(bad("its commitment section is not version 1 (a version byte, a 32-byte digest, a 64-byte signature)"));
        }
        let mut predecessor = [0u8; 32];
        predecessor.copy_from_slice(&body[1..33]);
        let mut sig = [0u8; 64];
        sig.copy_from_slice(&body[33..]);
        Ok(CaCommitment { predecessor, successor_der: successor_der.clone(), sig })
    }
}

/// What a node's commitment check at start found ([`pin_ca_commitment`]): the pins it holds, one per committing CA,
/// and what its operator's file is beside them. No pin and [`OfferedCommitment::Nothing`]: the node holds no commitment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitmentPins {
    /// The pins, in the order of their files' names. **At most one per committing CA.**
    pub pins: Vec<PinnedCommitment>,
    /// What the operator's `ca-commit.pem` holds, when it is not one of the pins.
    pub offered: OfferedCommitment,
}

/// One pin: this node's own promise "I accept no other successor from this CA" (the maintainer's decision of
/// 2026-11-18). It speaks of one committing CA only: the successor's own commitment to *its* successor is another
/// promiser's, held as another pin, and contradicts nothing here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedCommitment {
    pub commitment: CaCommitment,
    /// `true` when this start pinned it (from the operator's file); `false` when it was read back.
    pub first: bool,
    /// `true` while the committing CA is a root this node trusts; `false` once it is not (the pin is spent: it
    /// records a succession this node went through).
    pub active: bool,
}

/// What the operator's `ca-commit.pem` holds beside the pins a node already has ([`pin_ca_commitment`]), when the
/// start goes on. The pins stand in every case. A file that is pinned at this start is not here: it is a
/// [`PinnedCommitment`] with `first` set. A file the node cannot read as a commitment, or one a CA it trusts did not
/// sign, is not here either: it **refuses the start**, with a pin as without one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OfferedCommitment {
    /// No file, or a commitment this node has pinned.
    Nothing,
    /// A well-formed commitment whose committing CA is **no root this node trusts** — the successor's commitment to
    /// its successor, placed before this node trusts the successor (or a CA this node no longer trusts). It pins
    /// nothing; the node reads the file again at its next start. Reported only on a node that holds a pin
    /// ([`EARLY_COMMITMENT_BESIDE_A_PIN_STARTS`]); on a node with none it refuses the start.
    UntrustedCa,
    /// A commitment that names, as its committing CA, **a CA this node holds a pin for and no longer trusts** (the
    /// pin is spent: the node moved past that CA), and that differs from the pin: its digest. The node no longer has
    /// that CA's certificate, so it cannot tell whether the CA's key signed this; it is not called a conflict, it pins
    /// nothing, and the start goes on as for [`OfferedCommitment::UntrustedCa`]. Not "early": the node has been
    /// through that CA's succession.
    DiffersFromSpentPin([u8; 32]),
    /// **A second commitment from a CA this node already holds a pin for**, signed by that CA's key and naming
    /// another successor: its digest. A mistake by the issuer, or a stolen CA key. There is no in-band way to replace
    /// a pin: an operator removes it.
    Conflict([u8; 32]),
}

impl OfferedCommitment {
    /// What the node tells its operator about the file, when it has something to say (`None` for
    /// [`OfferedCommitment::Nothing`]). The pins stand in every case.
    pub fn describe(&self) -> Option<&'static str> {
        match self {
            OfferedCommitment::Nothing => None,
            OfferedCommitment::UntrustedCa => Some(
                "ca-commit.pem holds a commitment from a CA this node does not trust (placed before this node cut over to that CA?); \
                 nothing is pinned from it, this node's pins stand, and the file is read again at the next start",
            ),
            OfferedCommitment::DiffersFromSpentPin(_) => Some(
                "ca-commit.pem holds a commitment naming a CA this node already pinned a commitment from and has since moved past, \
                 and it differs from that pin; this node no longer trusts that CA and cannot tell whether its key signed the file. \
                 Nothing is pinned from it and the pin stands. If it is a second commitment from that CA, compare it with the issuer's",
            ),
            OfferedCommitment::Conflict(_) => Some(
                "ca-commit.pem holds a second commitment from a CA this node already holds a pin for, signed by that CA's key and \
                 naming another successor; it is refused and the pin stands. Two commitments from one CA mean a mistake or a stolen \
                 CA key: compare the file with the issuer's",
            ),
        }
    }
}

/// **The one difference between a node that holds a pin and one that does not**, for the operator's file
/// ([`pin_ca_commitment`]; #812's confirm round C1): a commitment from a CA the node does not trust (yet) lets the
/// start go on beside a pin — reported and counted, nothing pinned — because a fleet-wide copy of the successor's
/// commitment must not stop the nodes that have not cut over from restarting. With no pin the same file refuses the
/// start. **Decided by the maintainer on 2026-11-20: the difference stays as built, and this stays `true`** (`false`
/// would make the early file refuse in both cases; nothing else would change). Every other file a node cannot assess
/// — malformed, empty, carrying a key, a signature a trusted CA did not make — refuses the start in both cases.
pub const EARLY_COMMITMENT_BESIDE_A_PIN_STARTS: bool = true;

#[cfg(feature = "tls")]
mod imp {
    use super::NodeTls;
    use crate::{config::TlsConfig, error::GossipError, node_id::NodeId};

    use ed25519_dalek::{SigningKey, VerifyingKey};
    use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair, SanType, PKCS_ED25519};
    use rustls::{
        pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
        server::WebPkiClientVerifier,
        ClientConfig, RootCertStore, ServerConfig,
    };
    use std::{fs, path::Path, sync::Arc};

    /// How long a node waits for another process that is generating the shared CA: 300 × 100 ms.
    const CA_WAIT_STEPS: u32 = if cfg!(test) { 5 } else { 300 };

    /// **Load the cluster CA, or create it — exactly once, however many nodes start together.**
    ///
    /// Nodes that share `auto_cert_dir` (one volume, several containers) used to race here: each
    /// checked "does a CA exist?", found none, and generated **its own**. Each then trusted a
    /// different root, so mTLS between them never succeeded and they never peered — the federation
    /// suite's intermittent "b1 sees its one peer" failures. The files on disk could even end up as
    /// one node's certificate with another's key.
    ///
    /// Now: an exclusive-create lock (`ca.lock`) gives exactly one process the right to generate.
    /// It re-checks under the lock (another may have finished in between), writes each file to a
    /// temporary name and renames it into place (so no reader sees half a file), then removes the
    /// lock. Every other process waits, bounded, for both files and loads them. A lock that outlives
    /// its holder (a crash mid-generation) is an **error naming the lock**, never a silently
    /// different CA.
    ///
    /// Startup-only filesystem setup, like the rest of this function; the wait is a count-bounded
    /// sleep, not a clock read.
    pub(crate) fn load_or_create_ca(
        cfg: &TlsConfig,
    ) -> Result<(CertificateDer<'static>, KeyPair), GossipError> {
        let err = |reason: String| GossipError::InvalidField { field: "tls", reason };
        let dir = &cfg.auto_cert_dir;
        let auto_ca_cert_path = dir.join("ca-cert.pem");
        let auto_ca_key_path = dir.join("ca-key.pem");
        let ca_cert_path = cfg.ca_cert_pem.clone().unwrap_or(auto_ca_cert_path.clone());
        let lock_path = dir.join("ca.lock");

        let load = || -> Result<(CertificateDer<'static>, KeyPair), GossipError> {
            let pem = fs::read_to_string(&ca_cert_path).map_err(|e| err(format!("TLS: read CA cert: {e}")))?;
            let der = ca_cert_to_der(&pem, &ca_cert_path, root_is_operators(cfg))?;
            let key_pem = fs::read_to_string(&auto_ca_key_path).map_err(|e| err(format!("TLS: read CA key: {e}")))?;
            let key = KeyPair::from_pem(&key_pem).map_err(|e| err(format!("TLS: parse CA key: {e}")))?;
            Ok((der, key))
        };
        let present = || ca_cert_path.exists() && auto_ca_key_path.exists();

        let mut waited = 0u32;
        loop {
            if present() {
                return load();
            }
            match take_ca_lock(&lock_path) {
                Ok(_lock) => {
                    if present() {
                        return load();
                    }
                    // A key without its certificate is the fleet's CA with its certificate lost, or a creation that
                    // stopped part-way — never an empty directory to mint into: a new CA over the fleet's key would
                    // replace the root every node trusts (#699's review, finding 6).
                    if cfg.ca_cert_pem.is_none()
                        && let Some(why) = lone_key_state(dir, true)
                    {
                        return Err(err(why));
                    }
                    return generate_and_publish_ca(dir, &auto_ca_cert_path, &auto_ca_key_path);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if waited >= CA_WAIT_STEPS {
                        return Err(err(format!(
                            "TLS: another process holds {lock_path:?} and no CA appeared within 30s; \
                             if that process crashed while generating the CA, remove the lock and restart"
                        )));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    waited += 1;
                }
                Err(e) => return Err(err(format!("TLS: cannot take CA lock {lock_path:?}: {e}"))),
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn generate_ca_for_test() -> (CertificateDer<'static>, KeyPair) {
        generate_ca(None, 0).expect("a CA")
    }

    /// A fresh CA at `generation` (identity-and-lifecycle I2's tests): its subject names a test fleet and the
    /// generation, and a generation above 0 carries the `caGeneration` extension.
    #[cfg(test)]
    pub(crate) fn generate_ca_at_generation_for_test(fleet: u64, generation: u64) -> (CertificateDer<'static>, KeyPair) {
        generate_ca(Some(super::fleet_ca_subject(fleet, generation)), generation).expect("a CA")
    }

    /// A fresh self-signed Ed25519 CA: its certificate and key pair, written nowhere. `subject` is its
    /// commonName ([`super::fleet_ca_subject`] for `mycelium tls ca`); `None` keeps rcgen's default, which
    /// a node's own CA has. `generation` above 0 writes the `caGeneration` extension (I2, R1), refused by name while its
    /// OID is provisional outside a test build; generation 0 writes none (absent reads as 0).
    fn generate_ca(subject: Option<String>, generation: u64) -> Result<(CertificateDer<'static>, KeyPair), GossipError> {
        let err = |reason: String| GossipError::InvalidField { field: "tls", reason };
        if generation > 0 && !super::ca_generation_oid_open() {
            return Err(err(format!(
                "TLS: a generation-{generation} CA is not minted by this build: the caGeneration extension's OID is \
                 provisional until the registered IANA PEN arc replaces it (identity-and-lifecycle I2)"
            )));
        }
        let ca_key_pair = KeyPair::generate_for(&PKCS_ED25519).map_err(|e| err(format!("TLS: generate CA key: {e}")))?;
        let mut ca_params = CertificateParams::new(vec![]).map_err(|e| err(format!("TLS: CA params: {e}")))?;
        if let Some(cn) = subject {
            let mut dn = rcgen::DistinguishedName::new();
            dn.push(rcgen::DnType::CommonName, rcgen::DnValue::Utf8String(cn));
            ca_params.distinguished_name = dn;
        }
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        ca_params.not_after = rcgen::date_time_ymd(2099, 1, 1);
        if generation > 0 {
            ca_params.custom_extensions =
                vec![rcgen::CustomExtension::from_oid_content(super::CA_GENERATION_OID, super::ca_generation_extension_der(generation))];
        }
        let ca_cert = ca_params.self_signed(&ca_key_pair).map_err(|e| err(format!("TLS: self-sign CA: {e}")))?;
        Ok((CertificateDer::from(ca_cert.der().to_vec()), ca_key_pair))
    }

    /// A generated CA's two files, written in full under temporary names and not yet published.
    pub(crate) struct StagedCa {
        pub(crate) key: std::path::PathBuf,
        pub(crate) cert: std::path::PathBuf,
    }

    /// Write a generated CA's key and certificate under this process's temporary names (`.key.tmp-<pid>`,
    /// `.cert.tmp-<pid>`), **both before either is published**: a crash after the key's rename then always leaves the
    /// certificate's temporary beside it, the marker that reads the lone key as an interrupted creation — never as a
    /// lost certificate whose key must be kept (#699's confirmation round, N1).
    pub(crate) fn stage_ca(dir: &Path, ca_cert_der: &CertificateDer<'_>, ca_key_pair: &KeyPair) -> Result<StagedCa, GossipError> {
        let err = |reason: String| GossipError::InvalidField { field: "tls", reason };
        let pid = std::process::id();
        let staged = StagedCa { key: dir.join(format!(".key.tmp-{pid}")), cert: dir.join(format!(".cert.tmp-{pid}")) };
        write_private(&staged.key, ca_key_pair.serialize_pem().as_bytes()).map_err(|e| err(format!("TLS: write CA key: {e}")))?;
        write_public(&staged.cert, cert_der_to_pem(ca_cert_der.as_ref()).as_bytes()).map_err(|e| err(format!("TLS: write CA cert: {e}")))?;
        Ok(staged)
    }

    /// Generate a CA and publish it: both files written under temporary names, then each renamed into place, the key
    /// first. Called only while holding `ca.lock`.
    fn generate_and_publish_ca(
        dir: &Path,
        cert_path: &Path,
        key_path: &Path,
    ) -> Result<(CertificateDer<'static>, KeyPair), GossipError> {
        let err = |reason: String| GossipError::InvalidField { field: "tls", reason };
        let (ca_cert_der, ca_key_pair) = generate_ca(None, 0)?;

        let staged = stage_ca(dir, &ca_cert_der, &ca_key_pair)?;
        fs::rename(&staged.key, key_path).map_err(|e| err(format!("TLS: publish CA key: {e}")))?;
        fs::rename(&staged.cert, cert_path).map_err(|e| err(format!("TLS: publish CA cert: {e}")))?;

        tracing::info!("TLS: generated new cluster CA in {dir:?} — distribute ca-cert.pem to all nodes");
        Ok((ca_cert_der, ca_key_pair))
    }

    pub fn load_or_generate(
        cfg: &TlsConfig,
        node_id: &NodeId,
        removed: Arc<crate::removal::RemovedSet>,
        standing: Arc<crate::removal::KeyStanding>,
    ) -> Result<NodeTls, GossipError> {
        create_private_dir(&cfg.auto_cert_dir).map_err(|e| {
            GossipError::InvalidField { field: "tls", reason: format!("TLS: cannot create cert dir {:?}: {e}", cfg.auto_cert_dir) }
        })?;
        for said in writable_by_others(&cfg.auto_cert_dir) {
            tracing::warn!("{said}");
        }

        // ── 1. Node signing / identity key ────────────────────────────────
        let sanitized = node_id.as_str().replace([':', '.'], "_");
        let auto_key_path = cfg.auto_cert_dir.join(format!("{sanitized}.key"));
        let signing_key: SigningKey = match &cfg.key_pem {
            Some(p) => load_key_from_pkcs8_pem(p)?,
            None => {
                if auto_key_path.exists() {
                    load_key_raw(&auto_key_path)?
                } else {
                    let key = generate_key()?;
                    save_key_raw(&key, &auto_key_path)?;
                    key
                }
            }
        };

        // ── 2 + 3. The CA, and the node cert ─────────────────────────────
        // A **pre-issued** node certificate (`cert_pem`, issued off-node by `issue_node_cert`) needs
        // the CA *certificate* only: the CA key is neither loaded nor looked for, and nothing is
        // minted — the init that lets `id.ca_key_off_node` hold on a running node (plan §8,
        // 2026-10-03). Without one, the node cert is re-signed at every start with the CA key,
        // which must then be on this node (and is minted here when absent).
        let (ca_cert_der, node_cert_der) = match &cfg.cert_pem {
            Some(cert_path) => {
                if cfg.key_pem.is_none() {
                    return Err(GossipError::InvalidField {
                        field: "tls",
                        reason: "TLS: `cert_pem` names a pre-issued certificate, so `key_pem` must name its key (the node cannot re-sign a certificate it was issued)".into(),
                    });
                }
                let ca_cert_der = load_ca_cert_only(cfg)?;
                let pem = fs::read_to_string(cert_path).map_err(|e| GossipError::InvalidField {
                    field: "tls", reason: format!("TLS: read node cert {cert_path:?}: {e}"),
                })?;
                (ca_cert_der, pem_cert_to_der(&pem)?)
            }
            None => {
                let (ca_cert_der, ca_key_pair) = load_or_create_ca(cfg)?;
                let node_cert_der = node_cert_under(node_id, &public_key_of(&signing_key), &ca_cert_der, &ca_key_pair, super::issuance_after(0))?;
                (ca_cert_der, node_cert_der)
            }
        };

        // ── 4. Build rustls configs ───────────────────────────────────────
        let cert_bytes = node_cert_der.as_ref().to_vec();
        let roots = vec![ca_cert_der.as_ref().to_vec()];
        // R1: legacy closed whenever the root set is built — here, at start, from the root's generation alone.
        standing.set_legacy_closed(super::roots_close_legacy(&roots));
        let (server_config, client_config, gateway_key, cert_verifier, fleet_server_verifier) =
            build_rustls_configs(node_cert_der, &signing_key, std::slice::from_ref(&ca_cert_der), Arc::clone(&removed), Arc::clone(&standing))?;
        let gateway_key = Arc::new(arc_swap::ArcSwap::new(gateway_key));
        let gateway_server_config = Arc::new(live_gateway_server_config(Arc::clone(&gateway_key)));
        let fleet_server_current = Arc::new(arc_swap::ArcSwap::from_pointee(fleet_server_verifier));

        Ok(NodeTls {
            server_config: arc_swap::ArcSwap::from_pointee(server_config),
            client_config: arc_swap::ArcSwap::from_pointee(client_config),
            signing_key: arc_swap::ArcSwap::from_pointee(signing_key),
            gateway_server_config,
            gateway_key,
            fleet_server_verifier: Arc::new(LiveFleetServerVerifier(Arc::clone(&fleet_server_current))),
            fleet_server_current,
            peer_anchor_sink: std::sync::OnceLock::new(),
            node_cert: arc_swap::ArcSwap::from_pointee(cert_bytes),
            cert_verifier: arc_swap::ArcSwap::from_pointee(cert_verifier),
            roots: arc_swap::ArcSwap::from_pointee(roots),
            roots_generation: std::sync::atomic::AtomicU64::new(0),
            removed,
            standing,
            activate_lock: std::sync::Mutex::new(()),
            sessions_closed: Default::default(),
        })
    }

    /// Generate a fresh identity key + CA-signed node cert + rustls configs
    /// WITHOUT activating them, persisting the new key to disk so a restart uses
    /// it. Returns the material (and the new verifying key) so the caller can
    /// publish the new key to peers before the cutover (`NodeTls::activate`).
    /// Reuses the **existing** cluster CA — never regenerates it — and errors if
    /// no CA is present (rotation only makes sense post-bootstrap).
    pub fn generate_rotation(
        cfg: &TlsConfig,
        node_id: &NodeId,
        removed: Arc<crate::removal::RemovedSet>,
        standing: Arc<crate::removal::KeyStanding>,
    ) -> Result<super::RotationMaterial, GossipError> {
        generate_rotation_after(cfg, node_id, removed, standing, 0)
    }

    /// [`generate_rotation`], with the new certificate issued strictly after `issued_floor_ms` — the
    /// newest issuance among this node's own certificates (post-360 plan row Q: a clock stepped back
    /// must not mint a successor that ranks below its predecessor, or the predecessor's revocation
    /// would not count).
    pub fn generate_rotation_after(
        cfg: &TlsConfig,
        node_id: &NodeId,
        removed: Arc<crate::removal::RemovedSet>,
        standing: Arc<crate::removal::KeyStanding>,
        issued_floor_ms: u64,
    ) -> Result<super::RotationMaterial, GossipError> {
        rotation_over(cfg, node_id, removed, standing, issued_floor_ms, None)
    }

    /// The rotation [`generate_rotation_after`] and [`NodeTls::rotation_after`] build: the mesh configs over
    /// `over`'s roots (with the generation they were read at), or over the configuration's root when `None`.
    pub(super) fn rotation_over(
        cfg: &TlsConfig,
        node_id: &NodeId,
        removed: Arc<crate::removal::RemovedSet>,
        standing: Arc<crate::removal::KeyStanding>,
        issued_floor_ms: u64,
        over: Option<(u64, Vec<Vec<u8>>)>,
    ) -> Result<super::RotationMaterial, GossipError> {
        let signing_key = generate_key()?;
        let verifying_key = signing_key.verifying_key().to_bytes();

        // Persist the new key (raw 32 bytes), same layout as load_or_generate.
        let sanitized = node_id.as_str().replace([':', '.'], "_");
        let auto_key_path = cfg.auto_cert_dir.join(format!("{sanitized}.key"));
        save_key_raw(&signing_key, &auto_key_path)?;

        let (ca_cert_der, ca_key_pair) = load_existing_ca(cfg)?;
        let node_cert_der = node_cert_under(node_id, &verifying_key, &ca_cert_der, &ca_key_pair, super::issuance_after(issued_floor_ms))?;
        let cert_bytes = node_cert_der.as_ref().to_vec();
        let (built_over, roots) = match over {
            Some((generation, roots)) => (Some(generation), roots.into_iter().map(CertificateDer::from).collect::<Vec<_>>()),
            None => (None, vec![ca_cert_der]),
        };
        let (server_config, client_config, gateway_key, _, _) =
            build_rustls_configs(node_cert_der, &signing_key, &roots, removed, standing)?;

        Ok(super::RotationMaterial {
            verifying_key,
            server_config: Arc::new(server_config),
            client_config: Arc::new(client_config),
            signing_key: Arc::new(signing_key),
            gateway_key,
            node_cert_der: cert_bytes,
            roots: None,
            built_over,
        })
    }

    /// [`NodeTls::reroot_material`]: the node's current key and `node_cert_der` over a root store of
    /// `roots`, with no CA key (identity-and-lifecycle I2's reload path).
    pub(super) fn reroot_material(tls: &NodeTls, node_cert_der: Vec<u8>, roots: Vec<Vec<u8>>) -> Result<super::RotationMaterial, GossipError> {
        let err = |reason: String| GossipError::InvalidField { field: "tls", reason };
        if roots.is_empty() {
            return Err(err("TLS: a change of roots needs at least one CA certificate".into()));
        }
        let signing_key = tls.signing_key();
        let verifying_key = signing_key.verifying_key().to_bytes();
        if ed25519_key_from_cert_der(&node_cert_der) != Some(verifying_key) {
            return Err(err("TLS: the certificate for a change of roots is not for this node's current key — the \
                 successor re-issues the node's current key, never a new one".into()));
        }
        for (i, r) in roots.iter().enumerate() {
            root_generation_readable(r, &format!("(root {i} of the change of roots)"))?;
        }
        let root_ders: Vec<CertificateDer<'static>> = roots.iter().map(|d| CertificateDer::from(d.clone())).collect();
        let cert = CertificateDer::from(node_cert_der.clone());
        let (server_config, client_config, gateway_key, chain, fleet) =
            build_rustls_configs(cert.clone(), &signing_key, &root_ders, Arc::clone(&tls.removed), Arc::clone(&tls.standing))?;
        let now = rustls::pki_types::UnixTime::since_unix_epoch(std::time::Duration::from_millis(crate::sim_seam::wall_now_ms()));
        chain.verify_client_cert(&cert, &[], now).map_err(|e| err(format!(
            "TLS: the certificate for a change of roots does not verify against the new roots ({e}) — a node never \
             presents a certificate its own roots would refuse"
        )))?;
        Ok(super::RotationMaterial {
            verifying_key,
            server_config: Arc::new(server_config),
            client_config: Arc::new(client_config),
            signing_key,
            gateway_key,
            node_cert_der,
            roots: Some(super::RootsMaterial { legacy_closed: super::roots_close_legacy(&roots), ders: roots, chain, fleet }),
            built_over: None,
        })
    }

    /// **Mint a fleet CA** in `ca_dir` (`ca-cert.pem` + `ca-key.pem`), for the issuer's host that then runs
    /// [`issue_node_cert`] — the off-node path's first step, which until now only a node started over an empty
    /// directory could take (adopter-surface Y8). Refuses, naming the path, when `ca_dir` already holds either
    /// file: a second CA over a fleet's is a new fleet, never a refresh. Returns the certificate's path.
    ///
    /// It holds `ca.lock` — the lock [`load_or_create_ca`] takes — for the whole mint, so it never races a node
    /// generating a CA in the same directory; while another process holds the lock it publishes nothing and is
    /// refused naming the lock. The pair is published by hard link, so `ca_dir` must be on a filesystem that has
    /// them (FAT, exFAT and some SMB mounts do not; the refusal says so). A mint that stops between its two links
    /// leaves `ca-key.pem` beside its lock and its temporaries, and the next mint names that state and how to recover;
    /// a `ca-key.pem` with no such marker is a lost certificate and is named as one (see [`lone_key_state`]). Under the
    /// lock it removes temporaries an earlier crashed creation left.
    pub fn mint_ca(ca_dir: &Path) -> Result<std::path::PathBuf, GossipError> {
        mint_ca_at(ca_dir, None, 0)
    }

    /// [`mint_ca`] at `generation`, naming `fleet` (a fresh fleet id when `None`) — the primitive `mycelium tls commit`
    /// mints a successor with (identity-and-lifecycle I2, PR 2b): generation above 0 writes the `caGeneration`
    /// extension, refused by name while its OID is provisional outside a test build.
    #[doc(hidden)]
    pub fn mint_ca_at(ca_dir: &Path, fleet: Option<u64>, generation: u64) -> Result<std::path::PathBuf, GossipError> {
        let err = |reason: String| GossipError::InvalidField { field: "tls", reason };
        let exists = |p: &Path| err(format!("TLS: {p:?} exists — a CA is minted once; a second one over it is a new fleet"));
        let (cert, key) = (ca_dir.join("ca-cert.pem"), ca_dir.join("ca-key.pem"));
        let lock_path = ca_dir.join("ca.lock");
        let held_elsewhere = || err(format!(
            "TLS: another process holds {lock_path:?} — a node or a mint is creating a CA here; if none is, one crashed \
             while creating it: remove the lock and run `tls ca` again, which names anything else the crash left"
        ));
        // Only while no process holds the lock is what the directory holds settled: a node publishes its key before its
        // certificate, so a lone key read beside someone else's lock may be that node mid-publish (#699's review, 5).
        let refuse_existing = |holding_lock: bool| -> Result<(), GossipError> {
            if let Some(why) = lone_key_state(ca_dir, holding_lock) {
                return Err(err(why));
            }
            for p in [&cert, &key] {
                if p.exists() {
                    return Err(exists(p));
                }
            }
            Ok(())
        };
        if lock_path.exists() {
            return Err(held_elsewhere());
        }
        refuse_existing(false)?;
        create_private_dir(ca_dir).map_err(|e| err(format!("TLS: create {ca_dir:?}: {e}")))?;
        let _lock = match take_ca_lock(&lock_path) {
            Ok(lock) => lock,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Err(held_elsewhere()),
            Err(e) => return Err(err(format!("TLS: cannot take CA lock {lock_path:?}: {e}"))),
        };
        refuse_existing(true)?;
        // No CA here and the lock ours: whatever temporaries remain are an earlier creation's that crashed — a mint's
        // key temporary is a hard link to a dead CA's key — so they go before this one is published (#699's review, 2).
        for stale in creation_temporaries(ca_dir) {
            let _ = fs::remove_file(stale);
        }
        publish_minted_ca(ca_dir, &cert, &key, &exists, fleet, generation)?;
        tracing::info!("TLS: minted a fleet CA in {ca_dir:?} — distribute ca-cert.pem, never ca-key.pem");
        Ok(cert)
    }

    /// `ca_dir` holds `ca-key.pem` without `ca-cert.pem`, and what that means. Two different states look alike, and
    /// the recovery for one destroys the other (#699's review, finding 1):
    /// - **an interrupted creation** — a mint or a node stopped between publishing the key and the certificate. It
    ///   always leaves its markers beside the key: `ca.lock` (unless `holding_lock`, when the lock is this caller's)
    ///   or its temporaries. Nothing can have been issued from that key; the recovery removes the key and the markers.
    /// - **a lost certificate** — no marker. That key may have issued every node's certificate, and every node holds a
    ///   copy of `ca-cert.pem`: the recovery copies one back, and nothing ever tells anyone to delete the key.
    ///
    /// Named by `tls ca`, `tls issue`, rotation and a node's start alike. A certificate without a key is neither —
    /// a node's own directory holds exactly that — and reads as an existing CA.
    fn lone_key_state(ca_dir: &Path, holding_lock: bool) -> Option<String> {
        let (cert, key) = (ca_dir.join("ca-cert.pem"), ca_dir.join("ca-key.pem"));
        if !key.exists() || cert.exists() {
            return None;
        }
        let mut markers: Vec<std::path::PathBuf> = creation_temporaries(ca_dir);
        let lock = ca_dir.join("ca.lock");
        if !holding_lock && lock.exists() {
            markers.insert(0, lock);
        }
        Some(if markers.is_empty() {
            format!(
                "TLS: {ca_dir:?} holds ca-key.pem but its certificate ca-cert.pem is missing, with no lock or temporary \
                 file beside it — this CA may have issued node certificates, and every node holds a copy of \
                 ca-cert.pem: copy one back to {cert:?}. Never delete ca-key.pem: it is the fleet's CA"
            )
        } else {
            let list = markers.iter().map(|p| format!("{p:?}")).collect::<Vec<_>>().join(", ");
            format!(
                "TLS: {ca_dir:?} holds ca-key.pem without ca-cert.pem, beside what an interrupted CA creation leaves \
                 ({list}) — it stopped between publishing the key and the certificate, so nothing was issued from that \
                 key. If no process is creating a CA here now, remove {key:?} and {list}, then create the CA again"
            )
        })
    }

    /// Publish a freshly generated CA in `ca_dir`, while holding `ca.lock`.
    fn publish_minted_ca(ca_dir: &Path, cert: &Path, key: &Path, exists: &dyn Fn(&Path) -> GossipError, fleet: Option<u64>, generation: u64) -> Result<(), GossipError> {
        let err = |reason: String| GossipError::InvalidField { field: "tls", reason };
        let no_links = |what: &str, e: std::io::Error| err(format!(
            "TLS: publish CA {what} by hard link in {ca_dir:?}: {e} — `tls ca` needs a filesystem with hard links (FAT, exFAT \
             and some SMB mounts have none): mint on a local disk and copy the directory"
        ));
        // I2: a minted CA's subject names its fleet and its generation (0: no predecessor), so two
        // fleets' CAs never share a name and a node certificate's issuer is its own CA's.
        let fleet = fleet.unwrap_or_else(|| rand_core::RngCore::next_u64(&mut rand_core::OsRng));
        let (ca_cert_der, ca_key_pair) = generate_ca(Some(super::fleet_ca_subject(fleet, generation)), generation)?;
        // Both files written in full under names of this call's own, then published by hard link, which fails when
        // the name exists — so the check above is a courtesy and the link is the guard: two concurrent mints
        // publish one CA, and the other is refused rather than replacing it (#695's review, finding 4). The key is
        // published first and withdrawn if its certificate's name is taken, so the pair is one CA's or neither.
        let tag = format!("{}-{:x}", std::process::id(), rand_core::RngCore::next_u64(&mut rand_core::OsRng));
        let (tmp_key, tmp_cert) = (ca_dir.join(format!(".ca-key.mint-{tag}")), ca_dir.join(format!(".ca-cert.mint-{tag}")));
        let result = (|| {
            write_private(&tmp_key, ca_key_pair.serialize_pem().as_bytes()).map_err(|e| err(format!("TLS: write CA key: {e}")))?;
            write_public(&tmp_cert, cert_der_to_pem(ca_cert_der.as_ref()).as_bytes()).map_err(|e| err(format!("TLS: write CA cert: {e}")))?;
            fs::hard_link(&tmp_key, key).map_err(|e| if e.kind() == std::io::ErrorKind::AlreadyExists { exists(key) } else { no_links("key", e) })?;
            if let Err(e) = fs::hard_link(&tmp_cert, cert) {
                let _ = fs::remove_file(key);
                return Err(if e.kind() == std::io::ErrorKind::AlreadyExists { exists(cert) } else { no_links("certificate", e) });
            }
            Ok(())
        })();
        let _ = (fs::remove_file(&tmp_key), fs::remove_file(&tmp_cert));
        result
    }

    /// **`mycelium tls commit`: the old CA commits to its successor** (identity-and-lifecycle I2, PR 2b; decision (a)).
    /// Runs on the issuer's host, where the old CA's key is: mints the successor CA into `successor_dir` — the same
    /// fleet, the old CA's generation plus one ([`mint_ca_at`]; refused by name while the generation OID is
    /// provisional outside a test build, before anything is written) — signs the commitment with the **old** CA's key,
    /// and publishes it as `{ca_dir}/ca-commit.pem`, the file an operator copies into each node's certificate
    /// directory. Returns the commitment's path and the successor certificate's.
    ///
    /// **A CA commits once.** A `ca-commit.pem` already in `ca_dir` refuses the run by name, and the file is published
    /// by hard link, which fails when the name exists, so two runs never leave two commitments: a node pins the first
    /// it is given and refuses another. A run that mints the successor and then cannot publish removes nothing; it
    /// names the successor directory to remove before running again (no commitment names that CA, so nothing trusts it).
    pub fn commit_successor(ca_dir: &Path, successor_dir: &Path) -> Result<(std::path::PathBuf, std::path::PathBuf), GossipError> {
        commit_successor_observed(ca_dir, successor_dir, &mut |_, _| {})
    }

    /// [`commit_successor`], reporting each durability step to `observe` right after it returns — `file.sync` with the
    /// file, `dir.sync` with the directory synced (from inside the sync), `link` with the commitment's name — so a test
    /// witnesses the order rather than a label beside the call (the journal's tests' way).
    pub(super) fn commit_successor_observed(
        ca_dir: &Path,
        successor_dir: &Path,
        observe: &mut dyn FnMut(&'static str, &Path),
    ) -> Result<(std::path::PathBuf, std::path::PathBuf), GossipError> {
        let err = |reason: String| GossipError::InvalidField { field: "tls", reason };
        let out = ca_dir.join(super::CA_COMMIT_FILE);
        let committed = || err(format!(
            "TLS: {out:?} exists — this CA has already committed to a successor, and a CA commits once: nodes pin the \
             first commitment they are given and refuse another"
        ));
        match fs::read_to_string(&out) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            // A file that is not a commitment (empty, or cut short): what a crash left before this command synced the
            // file ahead of its name. It names no successor, so no node can have pinned from it; the operator checks
            // that no node was given another copy, and removes it.
            Ok(text) if super::CaCommitment::from_pem(&text).is_err() => {
                return Err(err(format!(
                    "TLS: {out:?} exists and is not a commitment (it is empty or damaged — an interrupted `tls commit`). \
                     It names no successor and no node can pin from it. If no node was given a commitment from this CA, \
                     remove {out:?} and the successor directory that run made, and run `tls commit` again; if one was, \
                     copy that node's `ca-commit.pem` back here instead: a CA commits once"
                )))
            }
            _ => return Err(committed()),
        }
        let ca_cfg = TlsConfig { auto_cert_dir: ca_dir.to_path_buf(), ..TlsConfig::default() };
        let (ca_cert_der, _) = load_existing_ca(&ca_cfg)?;
        let generation = root_generation_readable(ca_cert_der.as_ref(), &format!("{:?}", ca_dir.join("ca-cert.pem")))?;
        let next = generation.checked_add(1).ok_or_else(|| err("TLS: the CA's generation has no successor".into()))?;
        if !super::ca_generation_oid_open() {
            return Err(err(format!(
                "TLS: `tls commit` is refused by this build: a successor is a generation-{next} CA, and the caGeneration \
                 extension's OID 1.3.6.1.4.1.32473.2.1 is provisional until the registered IANA PEN arc replaces it \
                 (identity-and-lifecycle I2). Nothing was written"
            )));
        }
        let ca_key = load_key_from_pkcs8_pem(&ca_dir.join("ca-key.pem"))?;
        // The successor keeps the fleet's name when the old CA carries one (`tls ca`'s subject); a CA with another
        // subject (a node's own, or one minted before I2) gets a fresh fleet id, and is distinct either way.
        let fleet = super::ca_subject_cn(ca_cert_der.as_ref()).and_then(|cn| {
            let hex = cn.strip_prefix("Mycelium fleet ")?.split(' ').next()?;
            u64::from_str_radix(hex, 16).ok()
        });
        let successor_cert = mint_ca_at(successor_dir, fleet, next)?;
        let undo = || format!("remove {successor_dir:?} (no commitment names that CA) and run `tls commit` again");
        let pem = fs::read_to_string(&successor_cert).map_err(|e| err(format!("TLS: read {successor_cert:?}: {e}; {}", undo())))?;
        let successor_der = pem_cert_to_der(&pem)?;
        let commitment = super::CaCommitment::sign(ca_cert_der.as_ref(), &ca_key, successor_der.as_ref())
            .map_err(|e| err(format!("TLS: the commitment could not be made: {e}; {}", undo())))?;
        let tmp = ca_dir.join(format!(".ca-commit.tmp-{}-{:x}", std::process::id(), rand_core::RngCore::next_u64(&mut rand_core::OsRng)));
        // The promise is durable before it is handed to anyone (#812's review F5): the successor's key and certificate
        // and their directory, then the commitment's bytes, then its name (a hard link, which fails when the name
        // exists), then the directory holding the name.
        let synced = |path: &Path, observe: &mut dyn FnMut(&'static str, &Path)| fs::File::open(path).and_then(|f| f.sync_all()).map(|()| observe("file.sync", path));
        let published = (|| {
            let io = |what: &str, e: std::io::Error| err(format!("TLS: {what}: {e}; {}", undo()));
            for f in [successor_dir.join("ca-key.pem"), successor_cert.clone()] {
                synced(&f, observe).map_err(|e| io("sync the successor CA", e))?;
            }
            crate::persistence::sync_parent_dir_observed(&successor_cert, &mut |d| observe("dir.sync", d)).map_err(|e| io("sync the successor's directory", e))?;
            // …and the directory's own entry in the directory above it: a durable commitment must not name a
            // successor whose directory a power loss can take.
            crate::persistence::sync_parent_dir_observed(successor_dir, &mut |d| observe("dir.sync", d)).map_err(|e| io("sync the successor directory's entry", e))?;
            write_public(&tmp, commitment.to_pem().as_bytes()).map_err(|e| io("write the commitment", e))?;
            synced(&tmp, observe).map_err(|e| io("sync the commitment", e))?;
            fs::hard_link(&tmp, &out).map(|()| observe("link", &out)).map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    committed()
                } else {
                    err(format!(
                        "TLS: publish {out:?} by hard link: {e} — `tls commit` needs a filesystem with hard links (FAT, exFAT and \
                         some SMB mounts have none): run it on a local disk; {}",
                        undo()
                    ))
                }
            })?;
            // From here the commitment exists under its name and names the successor: nothing is to be removed.
            crate::persistence::sync_parent_dir_observed(&out, &mut |d| observe("dir.sync", d)).map_err(|e| err(format!(
                "TLS: {out:?} is written and names the successor in {successor_dir:?}, but syncing its directory failed: {e}. \
                 **Do not remove either and do not run `tls commit` again**: the commitment is made. Sync the disk (or \
                 check both are still there after a restart of this host) before copying the file to any node"
            )))
        })();
        let _ = fs::remove_file(&tmp);
        published?;
        tracing::info!("TLS: committed to a successor CA in {successor_dir:?}; copy {out:?} into each node's certificate directory");
        Ok((out, successor_cert))
    }

    /// The commitment file or a pin this node holds, if any exists — what a node with no `[persistence]` is refused
    /// for holding ([`pin_ca_commitment`]'s caller). The operator's file first.
    pub fn held_ca_commitment(cfg: &TlsConfig) -> Option<std::path::PathBuf> {
        let file = cfg.auto_cert_dir.join(super::CA_COMMIT_FILE);
        if fs::metadata(&file).is_ok() {
            return Some(file);
        }
        pin_files(&cfg.auto_cert_dir.join(super::SUCCESSION_DIR)).ok()?.into_iter().next()
    }

    /// The pin files under `dir` (`succession/`), by name. A missing directory holds none.
    fn pin_files(dir: &Path) -> std::io::Result<Vec<std::path::PathBuf>> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let mut out = Vec::new();
        for entry in entries {
            let path = entry?.path();
            let named = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(super::CA_COMMIT_PIN_PREFIX) && n.ends_with(super::CA_COMMIT_PIN_SUFFIX));
            if named {
                out.push(path);
            }
        }
        out.sort();
        Ok(out)
    }

    /// **A node's commitment check at start** (identity-and-lifecycle I2, PR 2b; the maintainer's decisions (a) of
    /// 2026-11-15, of 2026-11-16 and of 2026-11-18). `roots` are the CA certificates this node trusts.
    ///
    /// **A pin is per committing CA.** Each is one file under the node's own `{auto_cert_dir}/succession/`,
    /// `pin-{predecessor}.pem` ([`super::ca_commit_pin_file`]), holding the commitment that CA made. The node:
    /// - reads **every pin**, and refuses to start, by name, on one it cannot read as the pin its name says, or can
    ///   no longer place: a pin **stands** when its committing CA is a root this node trusts (active), or its
    ///   successor is (spent), or its successor is the committing CA of another pin that stands (an earlier
    ///   succession's). The refusal names **every** pin that does not stand and the way out — remove them. Removing a
    ///   pin leaves the pins of later CAs untouched; the pins of earlier ones stand only through it. **Only an active
    ///   pin's signature is checked at a start**: a spent pin does not carry its committing CA's certificate, so it
    ///   is held as this node's own record of a succession it went through and nothing may read a successor from it
    ///   for trust (the phases read only a pin that verified at this start — plan row I2, PR 2b-2);
    /// - reads the **operator-placed** `{auto_cert_dir}/ca-commit.pem`, if there is one:
    ///   - a commitment it already holds: nothing;
    ///   - a commitment from a CA it holds **no pin for**, which verifies against its roots (the committing CA is a
    ///     root it trusts): **pinned** — written under `succession/` (a temporary, fsynced, renamed, the directory
    ///     fsynced) before this returns. So the first commitment is pinned from the old CA, and, once the node trusts
    ///     the successor, one commitment from the successor, the same way;
    ///   - a second commitment from a CA it **holds a pin for**, signed by that CA's key: a conflict — the pin stands;
    ///   - a file it cannot read as a commitment (malformed, empty, carrying a key), or one a CA it trusts did not
    ///     sign: the start is refused by name, with a pin as without one;
    ///   - a commitment from a CA it does not trust (yet): with no pin, the start is refused by name; beside a pin it
    ///     is reported, nothing is pinned, and the start goes on ([`super::EARLY_COMMITMENT_BESIDE_A_PIN_STARTS`], the
    ///     one place the two differ).
    ///
    /// **"Per CA" is per CA certificate**: the pin's name is the digest of the committing CA's certificate, so a CA
    /// re-issued under the same key with another certificate is another committing CA; the old pin can then not be
    /// placed and refuses the start (fails closed).
    ///
    /// The operator's file is never written, and nothing but that file is ever pinned: a commitment seen in the KV is
    /// compared with the pins by its reader and is never passed here.
    pub fn pin_ca_commitment(cfg: &TlsConfig, roots: &[Vec<u8>]) -> Result<super::CommitmentPins, GossipError> {
        use super::{OfferedCommitment as Offered, PinnedCommitment};
        let refuse = |reason: String| GossipError::InvalidField { field: "tls.ca_commit", reason };
        let file = cfg.auto_cert_dir.join(super::CA_COMMIT_FILE);
        let dir = cfg.auto_cert_dir.join(super::SUCCESSION_DIR);
        let way_out = |pin: &Path| format!(
            "The pin is this node's memory of a commitment it accepted from one CA. To drop that one pin, remove {pin:?} \
             and start the node again. The pins of later CAs are untouched; a pin of an **earlier** succession stands only \
             through the pins after it, so removing one from the middle leaves every older one unplaceable too. From the \
             release that adds the succession's phases, {dir:?} also holds this node's phase, so do not remove the directory"
        );
        let mut held: Vec<(std::path::PathBuf, super::CaCommitment)> = Vec::new();
        for path in pin_files(&dir).map_err(|e| refuse(format!("TLS: read {dir:?}: {e}")))? {
            let pem = fs::read_to_string(&path).map_err(|e| refuse(format!("TLS: read {path:?}: {e}")))?;
            let pin = super::CaCommitment::from_pem(&pem).map_err(|e| refuse(format!("TLS: the pin {path:?} is refused: {e}. {}", way_out(&path))))?;
            if path.file_name().and_then(|n| n.to_str()) != Some(super::ca_commit_pin_file(&pin.predecessor).as_str()) {
                return Err(refuse(format!(
                    "TLS: the pin {path:?} holds a commitment by another CA than its name says (a pin's name is its \
                     committing CA's digest). {}",
                    way_out(&path)
                )));
            }
            held.push((path, pin));
        }
        // Which pins stand: active, spent, or an earlier succession's whose successor committed in a pin that stands.
        let active: Vec<bool> = held.iter().map(|(_, p)| p.verify(roots).is_ok()).collect();
        let mut stands: Vec<bool> = held.iter().zip(&active).map(|((_, p), a)| *a || roots.iter().any(|r| r.as_slice() == p.successor_der.as_slice())).collect();
        loop {
            let before = stands.clone();
            for i in 0..held.len() {
                let successor = super::sha256(&held[i].1.successor_der);
                if !stands[i] && held.iter().zip(&before).any(|((_, other), ok)| *ok && other.predecessor == successor) {
                    stands[i] = true;
                }
            }
            if stands == before {
                break;
            }
        }
        // Every pin that does not stand is named in the one refusal, so an operator is not sent through one restart
        // per pin (#812's confirm round C2).
        let fallen: Vec<&std::path::PathBuf> = held.iter().zip(&stands).filter(|(_, ok)| !**ok).map(|((path, _), _)| path).collect();
        if let Some(first) = fallen.first() {
            let all = fallen.iter().map(|p| format!("{p:?}")).collect::<Vec<_>>().join(", ");
            return Err(refuse(format!(
                "TLS: {} pinned CA commitment(s) cannot be placed against the roots this node trusts: {all}. For each, its \
                 committing CA is not a root this node trusts, its successor is not one either, and no pin that stands \
                 continues from it. If they are to go, remove every file named here. {}",
                fallen.len(),
                way_out(first)
            )));
        }
        let mut pins: Vec<PinnedCommitment> =
            held.iter().zip(&active).map(|((_, p), a)| PinnedCommitment { commitment: p.clone(), first: false, active: *a }).collect();
        let offered_text = match fs::read_to_string(&file) {
            Ok(pem) => Some(pem),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(refuse(format!("TLS: read {file:?}: {e}"))),
        };
        // A file this node cannot assess refuses the start, with a pin as without one — it is never called a conflict
        // (#812's review F3), and it is never passed over (the confirm round's C1). The one exception, and the one
        // place a pin makes a difference: a commitment from a CA this node does not trust (yet), beside a pin.
        let cannot_assess = |e: super::CaCommitError| -> Result<Offered, GossipError> {
            if e == super::CaCommitError::UnknownPredecessor && !held.is_empty() && super::EARLY_COMMITMENT_BESIDE_A_PIN_STARTS {
                Ok(Offered::UntrustedCa)
            } else {
                Err(refuse(format!("TLS: the CA commitment {file:?} is refused: {e}")))
            }
        };
        let offered = match offered_text.as_deref().map(super::CaCommitment::from_pem) {
            None => Offered::Nothing,
            Some(Err(e)) => cannot_assess(e)?,
            Some(Ok(o)) => match (held.iter().find(|(_, p)| p.predecessor == o.predecessor), o.verify(roots)) {
                (Some((_, p)), _) if p.digest() == o.digest() => Offered::Nothing,
                // A differing commitment naming a CA this node pinned from and no longer trusts: not early, and not
                // provably that CA's (#812's second confirm round, D3). Under the same switch as the early file.
                (Some(_), Err(super::CaCommitError::UnknownPredecessor)) if super::EARLY_COMMITMENT_BESIDE_A_PIN_STARTS => {
                    Offered::DiffersFromSpentPin(o.digest())
                }
                (_, Err(e)) => cannot_assess(e)?,
                // The CA this node holds a pin for signed a second commitment: the pin stands.
                (Some(_), Ok(_)) => Offered::Conflict(o.digest()),
                // A CA this node trusts and holds no pin for: its one commitment is pinned, from this file only.
                (None, Ok(_)) => {
                    let pin_path = dir.join(super::ca_commit_pin_file(&o.predecessor));
                    // Remembered before it is acted on: the pin is durable — its bytes, then its name — before this returns.
                    let io = |what: &str, e: std::io::Error| refuse(format!("TLS: pin the CA commitment under {dir:?}: {what}: {e}"));
                    crate::persistence::create_dir_all_synced(&dir).map_err(|e| io("create the directory", e))?;
                    let tmp = dir.join(format!(".pin.tmp-{}", std::process::id()));
                    write_public(&tmp, o.to_pem().as_bytes()).map_err(|e| io("write", e))?;
                    fs::File::open(&tmp).and_then(|f| f.sync_all()).map_err(|e| io("sync", e))?;
                    // By hard link, which fails when the name exists: "the first commitment from this CA" is then a
                    // property of the file system, not of this function having read the directory first.
                    let linked = fs::hard_link(&tmp, &pin_path);
                    let _ = fs::remove_file(&tmp);
                    linked.map_err(|e| {
                        if e.kind() == std::io::ErrorKind::AlreadyExists {
                            io("publish the pin (a pin for this CA appeared while this node was starting: start it again)", e)
                        } else {
                            io("publish the pin by hard link (the node's certificate directory must be on a file system with hard links — FAT, exFAT and some SMB mounts have none; starting again will not help)", e)
                        }
                    })?;
                    crate::persistence::sync_parent_dir(&pin_path).map_err(|e| io("sync the directory", e))?;
                    pins.push(PinnedCommitment { commitment: o, first: true, active: true });
                    Offered::Nothing
                }
            },
        };
        Ok(super::CommitmentPins { pins, offered })
    }

    /// **Issue a node certificate off-node** (`id.ca_key_off_node`, plan §8). Runs where the CA key
    /// is — an operator's workstation, a signing host — never on the node: loads the existing CA
    /// from `ca_dir` (never mints one), generates a fresh Ed25519 identity for `node_id`, signs its
    /// certificate exactly as `load_or_generate` would, and writes `<node>.cert.pem` and
    /// `<node>.key.pem` (PKCS#8) under `out_dir`. The node then starts with `[tls] cert_pem` /
    /// `key_pem` pointing at them and `ca-cert.pem` (the certificate only) in its directory, and
    /// `load_or_generate` neither needs nor looks for the CA key. Returns the two paths.
    pub fn issue_node_cert(ca_dir: &Path, node_id: &NodeId, out_dir: &Path) -> Result<(std::path::PathBuf, std::path::PathBuf), GossipError> {
        use base64::{engine::general_purpose::STANDARD, Engine};
        use ed25519_dalek::pkcs8::EncodePrivateKey;
        let err = |reason: String| GossipError::InvalidField { field: "tls", reason };
        let ca_cfg = TlsConfig { auto_cert_dir: ca_dir.to_path_buf(), ..TlsConfig::default() };
        let (ca_cert_der, ca_key_pair) = load_existing_ca(&ca_cfg)?;
        let signing_key = generate_key()?;
        let node_cert_der = node_cert_under(node_id, &public_key_of(&signing_key), &ca_cert_der, &ca_key_pair, super::issuance_after(0))?;
        create_private_dir(out_dir).map_err(|e| err(format!("TLS: create {out_dir:?}: {e}")))?;
        let sanitized = node_id.as_str().replace([':', '.'], "_");
        let cert_path = out_dir.join(format!("{sanitized}.cert.pem"));
        let key_path = out_dir.join(format!("{sanitized}.key.pem"));
        let pkcs8 = signing_key.to_pkcs8_der().map_err(|e| err(format!("TLS: encode node key: {e}")))?;
        let b64 = STANDARD.encode(pkcs8.as_bytes());
        let mut key_pem = String::from("-----BEGIN PRIVATE KEY-----\n");
        for chunk in b64.as_bytes().chunks(64) {
            key_pem.push_str(std::str::from_utf8(chunk).expect("base64 is ascii"));
            key_pem.push('\n');
        }
        key_pem.push_str("-----END PRIVATE KEY-----\n");
        write_public(&cert_path, cert_der_to_pem(node_cert_der.as_ref()).as_bytes()).map_err(|e| err(format!("TLS: write {cert_path:?}: {e}")))?;
        write_private(&key_path, key_pem.as_bytes()).map_err(|e| err(format!("TLS: write {key_path:?}: {e}")))?;
        Ok((cert_path, key_path))
    }

    /// **Re-issue an existing key** under the CA in `ca_dir` (identity-and-lifecycle I2): a node
    /// certificate for `node_id` over `public_key` **alone** — no private key reaches the issuer's host —
    /// issued strictly after `floor_ms` (the node's newest issuance, post-360 plan row Q). Returns the
    /// certificate's DER and writes nothing. The primitive `mycelium tls succeed` is to call (I2's later
    /// PR, which adds `firstIssuedMs` and the inventory); until then only tests do.
    #[doc(hidden)]
    pub fn issue_node_cert_for_key(ca_dir: &Path, node_id: &NodeId, public_key: [u8; 32], floor_ms: u64) -> Result<Vec<u8>, GossipError> {
        let ca_cfg = TlsConfig { auto_cert_dir: ca_dir.to_path_buf(), ..TlsConfig::default() };
        let (ca_cert_der, ca_key_pair) = load_existing_ca(&ca_cfg)?;
        Ok(node_cert_under(node_id, &public_key, &ca_cert_der, &ca_key_pair, super::issuance_after(floor_ms))?.as_ref().to_vec())
    }

    /// The CA **certificate** alone (`ca_cert_pem`, else `{auto_cert_dir}/ca-cert.pem`): load-only,
    /// an error naming the path when absent — never a mint. The path a pre-issued node cert takes.
    fn load_ca_cert_only(cfg: &TlsConfig) -> Result<CertificateDer<'static>, GossipError> {
        let ca_cert_path = cfg.ca_cert_pem.clone().unwrap_or(cfg.auto_cert_dir.join("ca-cert.pem"));
        let pem = fs::read_to_string(&ca_cert_path).map_err(|e| GossipError::InvalidField {
            field: "tls", reason: format!("TLS: a pre-issued node cert needs the CA cert ({ca_cert_path:?}): {e}"),
        })?;
        ca_cert_to_der(&pem, &ca_cert_path, root_is_operators(cfg))
    }

    /// Whether the CA certificate file is the operator's root rather than a node's own CA: `ca_cert_pem` is
    /// set, or the node starts from a pre-issued `cert_pem`, whose `{auto_cert_dir}/ca-cert.pem` the operator
    /// copied there (`mycelium tls issue` says to; #796's review, finding 1).
    fn root_is_operators(cfg: &TlsConfig) -> bool {
        cfg.ca_cert_pem.is_some() || cfg.cert_pem.is_some()
    }

    /// The CA certificate of a PEM document read as the fleet's root. When it is the operator's
    /// root (`configured`: `ca_cert_pem`, or `ca-cert.pem` beside a pre-issued `cert_pem`), it must hold **exactly one** certificate (identity-and-lifecycle I2,
    /// exit 14): `pem_cert_to_der` keeps only the first, so a second root there was silently dropped,
    /// and a succession's two roots must never be written that way. The on-node CA's `ca-cert.pem`
    /// (`configured` false) is read as before; `cert_pem`, a chain, is never read here.
    pub(crate) fn ca_cert_to_der(pem: &str, path: &Path, configured: bool) -> Result<CertificateDer<'static>, GossipError> {
        if configured {
            let n = pem_certs(pem.as_bytes())
                .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: parse CA cert PEM: {e}") })?
                .len();
            if n > 1 {
                return Err(GossipError::InvalidField {
                    field: "tls.ca_cert_pem",
                    reason: format!(
                        "TLS: the fleet's root (`ca_cert_pem`, or `ca-cert.pem` beside a pre-issued `cert_pem`: {path:?}) holds {n} certificates; it must hold exactly one, the fleet CA's. \
                         Before this release only the first was read and the rest were ignored, so remove the others \
                         (docs/guide/deprecations.md §{CA_CERT_PEM_SECTION})"
                    ),
                });
            }
        }
        let der = pem_cert_to_der(pem)?;
        root_generation_readable(der.as_ref(), &format!("{path:?}"))?;
        Ok(der)
    }

    /// Refuse, by name, a root whose generation this build cannot read (identity-and-lifecycle I2, R1): a malformed
    /// `caGeneration`, or one under the provisional OID outside a test build. Every root a node trusts passes here —
    /// the file at start ([`ca_cert_to_der`]), the issuer's CA, and each root of a change of roots
    /// ([`reroot_material`]) — so [`super::roots_close_legacy`] never meets a root it would misread as generation 0.
    pub(super) fn root_generation_readable(der: &[u8], named: &str) -> Result<u64, GossipError> {
        super::ca_generation(der).map_err(|why| GossipError::InvalidField {
            field: "tls.ca_cert_pem",
            reason: format!("TLS: the CA certificate {named} is refused: {why}"),
        })
    }

    /// The section of `docs/guide/deprecations.md` that states [`ca_cert_to_der`]'s refusal.
    pub(crate) const CA_CERT_PEM_SECTION: u32 = 111;

    /// Load the existing cluster CA cert + key (load-only; errors if absent —
    /// unlike `load_or_generate`, rotation must never mint a new CA).
    fn load_existing_ca(cfg: &TlsConfig) -> Result<(CertificateDer<'static>, KeyPair), GossipError> {
        let auto_ca_cert_path = cfg.auto_cert_dir.join("ca-cert.pem");
        let auto_ca_key_path  = cfg.auto_cert_dir.join("ca-key.pem");
        let ca_cert_path = cfg.ca_cert_pem.clone().unwrap_or(auto_ca_cert_path);
        if cfg.ca_cert_pem.is_none()
            && let Some(why) = lone_key_state(&cfg.auto_cert_dir, false)
        {
            return Err(GossipError::InvalidField { field: "tls", reason: why });
        }
        let pem = fs::read_to_string(&ca_cert_path).map_err(|e| GossipError::InvalidField {
            field: "tls", reason: format!("TLS: needs an existing CA cert ({ca_cert_path:?}) — never minted here: {e}"),
        })?;
        let ca_cert_der = ca_cert_to_der(&pem, &ca_cert_path, root_is_operators(cfg))?;
        let key_pem = fs::read_to_string(&auto_ca_key_path).map_err(|e| GossipError::InvalidField {
            field: "tls", reason: format!("TLS: needs the CA key ({auto_ca_key_path:?}) — rotation and issuance sign with it, a node never needs it: {e}"),
        })?;
        let ca_key_pair = KeyPair::from_pem(&key_pem).map_err(|e| GossipError::InvalidField {
            field: "tls", reason: format!("TLS: parse CA key: {e}"),
        })?;
        Ok((ca_cert_der, ca_key_pair))
    }

    fn generate_key() -> Result<SigningKey, GossipError> {
        use rand_core::OsRng;
        Ok(SigningKey::generate(&mut OsRng))
    }

    fn save_key_raw(key: &SigningKey, path: &Path) -> Result<(), GossipError> {
        write_private(path, key.as_bytes())
            .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: write key {:?}: {e}", path) })
    }

    /// Write a private key readable by its owner only: created `0600` on unix whatever the umask, and narrowed to
    /// `0600` before any byte is written when the file already exists (a key an older release left `0644`). Every
    /// private key this module writes goes through here; a certificate goes through [`write_public`]. Elsewhere than
    /// unix it is a plain write: the file takes whatever the platform gives it.
    fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
        write_with_mode(path, contents, 0o600)
    }

    /// Write a certificate: `0644` on unix whatever the umask, narrowed when it rewrites a wider file, so no other
    /// local user can replace a trust anchor or a node's certificate (#695's confirmation round, follow-up 4). Elsewhere
    /// than unix a plain write.
    fn write_public(path: &Path, contents: &[u8]) -> std::io::Result<()> {
        write_with_mode(path, contents, 0o644)
    }

    fn write_with_mode(path: &Path, contents: &[u8], mode: u32) -> std::io::Result<()> {
        use std::io::Write as _;
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            opts.mode(mode);
            let mut f = opts.open(path)?;
            f.set_permissions(fs::Permissions::from_mode(mode))?;
            f.write_all(contents)
        }
        #[cfg(not(unix))]
        {
            let _ = mode;
            opts.open(path)?.write_all(contents)
        }
    }

    /// Create a directory for keys and certificates, `0700` on unix whatever the umask: its only reader is the process
    /// that writes it (a node, or the issuer), and files leave it by being copied, so no other user needs to list or
    /// enter it — and under a permissive umask a wider directory would let one replace the trust anchor. Missing
    /// parents are created `0700` the same way (#699's review, finding 4). A directory that already exists is the
    /// operator's and is left as it is (a node warns when it is writable by others: [`writable_by_others`]).
    /// Elsewhere than unix a plain create.
    fn create_private_dir(dir: &Path) -> std::io::Result<()> {
        if dir.is_dir() {
            return Ok(());
        }
        if let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty() && !p.is_dir()) {
            create_private_dir(parent)?;
        }
        let mut b = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            b.mode(0o700);
            match b.create(dir) {
                Ok(()) => fs::set_permissions(dir, fs::Permissions::from_mode(0o700)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && dir.is_dir() => Ok(()),
                Err(e) => Err(e),
            }
        }
        #[cfg(not(unix))]
        {
            match b.create(dir) {
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && dir.is_dir() => Ok(()),
                r => r,
            }
        }
    }

    /// What, in a certificate directory, another local user could replace: the directory itself or its
    /// `ca-cert.pem` when either is writable by group or others, each named with the `chmod` that closes it. The
    /// module never narrows an existing path itself — a directory that exists is the operator's — so a node says this
    /// at start and `mycelium tls` after `ca` and `issue` (#699's review, finding 3). Empty elsewhere than unix.
    pub fn writable_by_others(dir: &Path) -> Vec<String> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut said = Vec::new();
            let mode = |p: &Path| fs::metadata(p).ok().map(|m| m.permissions().mode() & 0o7777);
            // A sticky directory (`1777`, like `/tmp`) lets no other user rename or delete a file it does not own.
            if let Some(m) = mode(dir).filter(|m| m & 0o022 != 0 && m & 0o1000 == 0) {
                said.push(format!(
                    "TLS: {dir:?} is writable by group or others (mode {m:03o}): another local user could replace the \
                     CA certificate in it — run `chmod 700 {}`", dir.display()
                ));
            }
            let cert = dir.join("ca-cert.pem");
            if let Some(m) = mode(&cert).filter(|m| m & 0o022 != 0) {
                said.push(format!(
                    "TLS: {cert:?} is writable by group or others (mode {m:03o}): another local user could replace the \
                     trust anchor — run `chmod 644 {}`", cert.display()
                ));
            }
            said
        }
        #[cfg(not(unix))]
        {
            let _ = dir;
            Vec::new()
        }
    }

    /// `ca.lock`, held: taken by exclusive create, released when dropped — on success, on an error and when its holder
    /// panics — so only a crash of the process leaves it (#699's review, finding 7).
    pub(crate) struct CaLock(std::path::PathBuf);

    impl Drop for CaLock {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    pub(crate) fn take_ca_lock(path: &Path) -> std::io::Result<CaLock> {
        fs::OpenOptions::new().write(true).create_new(true).open(path)?;
        Ok(CaLock(path.to_path_buf()))
    }

    /// The names a CA creation leaves in `dir` when it stops part-way: a mint's `.ca-key.mint-*` / `.ca-cert.mint-*`
    /// and a node's `.key.tmp-*` / `.cert.tmp-*`. A mint's key temporary is a hard link to the key it was publishing.
    fn creation_temporaries(dir: &Path) -> Vec<std::path::PathBuf> {
        let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
        let mut found: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                n.starts_with(".ca-key.mint-") || n.starts_with(".ca-cert.mint-") || n.starts_with(".key.tmp-") || n.starts_with(".cert.tmp-")
            })
            .collect();
        found.sort();
        found
    }

    fn load_key_raw(path: &Path) -> Result<SigningKey, GossipError> {
        let bytes = fs::read(path)
            .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: read key {:?}: {e}", path) })?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| GossipError::InvalidField { field: "tls", reason: "TLS: key file must be exactly 32 bytes".into() })?;
        Ok(SigningKey::from_bytes(&arr))
    }

    fn load_key_from_pkcs8_pem(path: &Path) -> Result<SigningKey, GossipError> {
        use base64::{engine::general_purpose::STANDARD, Engine};
        use ed25519_dalek::pkcs8::DecodePrivateKey;
        let pem = fs::read_to_string(path)
            .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: read key PEM {:?}: {e}", path) })?;
        // Strip PEM armor and base64-decode to DER, then parse.
        // Avoids requiring the `pem` feature flag on the pkcs8 crate.
        let b64: String = pem.lines()
            .filter(|l| !l.starts_with("-----"))
            .collect();
        let der = STANDARD.decode(b64.trim())
            .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: decode key PEM: {e}") })?;
        SigningKey::from_pkcs8_der(&der)
            .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: parse PKCS8 key: {e}") })
    }

    /// An Ed25519 public key as rcgen signs a certificate over it: the certificate is issued over the
    /// **public key alone** — the private key never reaches the issuer (the re-issue I2 builds on).
    pub(super) struct Ed25519Public(pub(super) [u8; 32]);

    impl rcgen::PublicKeyData for Ed25519Public {
        fn der_bytes(&self) -> &[u8] {
            &self.0
        }
        fn algorithm(&self) -> &rcgen::SignatureAlgorithm {
            &PKCS_ED25519
        }
    }

    pub(super) fn public_key_of(key: &SigningKey) -> [u8; 32] {
        key.verifying_key().to_bytes()
    }

    /// The CA a certificate is signed by, rebuilt for rcgen from the CA's key and **the subject its
    /// certificate carries** (identity-and-lifecycle I2: a distinct successor subject). rcgen 0.13 has no
    /// `from_ca_cert_der` without its `x509-parser` feature, so the CA is rebuilt from parameters; until
    /// I2 the parameters were fixed, and so carried rcgen's default subject whatever the CA's certificate
    /// said. That was harmless only while every CA had that subject.
    ///
    /// The chain is found **by name**: webpki looks the issuer up among the roots by the certificate's
    /// issuer name, and only then checks the signature (the spike S-succeed's finding 3 — across two CAs
    /// with one name a refusal read `BadSignature`, not `UnknownIssuer`). The comment this replaces said
    /// rustls chained by the SubjectKeyIdentifier; it does not. So the issuer name written must be the
    /// CA's subject byte for byte: the subject is read from the CA certificate (`ca_subject_cn`), only
    /// the one shape this tree writes is accepted, and the issued certificate's issuer is compared with it
    /// after signing (`issue_checked`).
    fn signing_ca(ca_cert_der: &[u8], ca_key_pair: &KeyPair) -> Result<rcgen::Certificate, GossipError> {
        let err = |reason: String| GossipError::InvalidField { field: "tls", reason };
        let cn = super::ca_subject_cn(ca_cert_der).ok_or_else(|| err(
            "TLS: the CA certificate's subject is not one this issuer writes (exactly one UTF8String commonName): \
             issue under a CA minted by `mycelium tls ca` or by a node".into(),
        ))?;
        let mut ca_params = CertificateParams::new(vec![]).map_err(|e| err(format!("TLS: CA params for signing: {e}")))?;
        let mut dn = rcgen::DistinguishedName::new();
        dn.push(rcgen::DnType::CommonName, rcgen::DnValue::Utf8String(cn));
        ca_params.distinguished_name = dn;
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        ca_params.not_after  = rcgen::date_time_ymd(2099, 1, 1);
        ca_params.self_signed(ca_key_pair).map_err(|e| err(format!("TLS: reconstruct CA for signing: {e}")))
    }

    /// Sign `params` over `public` under the CA, and refuse the result unless its issuer name is the CA
    /// certificate's subject byte for byte (see [`signing_ca`]).
    fn issue_checked(
        params: CertificateParams,
        public: &[u8; 32],
        ca_cert_der: &[u8],
        ca_key_pair: &KeyPair,
    ) -> Result<CertificateDer<'static>, GossipError> {
        issue_checked_over(params, &Ed25519Public(*public), ca_cert_der, ca_key_pair)
    }

    /// [`issue_checked`] over any public key rcgen can certify — an Ed25519 key, or a consent certificate's P-256
    /// point (identity-and-lifecycle I7).
    fn issue_checked_over(
        params: CertificateParams,
        public: &impl rcgen::PublicKeyData,
        ca_cert_der: &[u8],
        ca_key_pair: &KeyPair,
    ) -> Result<CertificateDer<'static>, GossipError> {
        let err = |reason: String| GossipError::InvalidField { field: "tls", reason };
        let ca = signing_ca(ca_cert_der, ca_key_pair)?;
        let cert = params
            .signed_by(public, &ca, ca_key_pair)
            .map_err(|e| err(format!("TLS: sign certificate: {e}")))?;
        let der = cert.der().to_vec();
        if super::tbs_name(&der, super::TbsName::Issuer).is_none()
            || super::tbs_name(&der, super::TbsName::Issuer) != super::tbs_name(ca_cert_der, super::TbsName::Subject)
        {
            return Err(err("TLS: the certificate's issuer name is not the CA's subject — it would chain to no root".into()));
        }
        Ok(CertificateDer::from(der))
    }

    /// A node certificate for `public` under the CA whose certificate is `ca_cert_der` (post-360 plan row
    /// Q's names and serial; identity-and-lifecycle I2's issuer name).
    pub(super) fn node_cert_under(
        node_id: &NodeId,
        public: &[u8; 32],
        ca_cert_der: &[u8],
        ca_key_pair: &KeyPair,
        issued_ms: u64,
    ) -> Result<CertificateDer<'static>, GossipError> {
        // Add the node's IP address as a Subject Alternative Name
        let ip: std::net::IpAddr = node_id.to_socket_addr().ip();
        let mut params = CertificateParams::new(vec![])
            .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: node cert params: {e}") })?;
        // Post-360 plan row Q: the node's NodeId as a URI, so a certificate names *its node* and not
        // just a host (nodes on one host share the IP), and the issuance time in the serial — the
        // order a CA re-issue outranks earlier keys by. `notBefore` keeps its fixed date: dating it
        // now would make a peer whose clock is behind refuse the handshake.
        let uri = rcgen::Ia5String::try_from(super::node_uri(node_id))
            .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: node URI: {e}") })?;
        params.subject_alt_names = vec![SanType::IpAddress(ip), SanType::URI(uri)];
        params.serial_number = Some(rcgen::SerialNumber::from(issued_ms));
        params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        super::set_not_after_for_issue(&mut params);
        issue_checked(params, public, ca_cert_der, ca_key_pair)
    }

    /// The CA certificate rcgen's fixed parameters make from `ca_key_pair` — the subject every CA had
    /// before I2, and that a node's own CA (`generate_ca`) still has.
    #[cfg(test)]
    pub(super) fn default_subject_ca_der(ca_key_pair: &KeyPair) -> Vec<u8> {
        let mut ca_params = CertificateParams::new(vec![]).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        ca_params.not_after  = rcgen::date_time_ymd(2099, 1, 1);
        ca_params.self_signed(ca_key_pair).unwrap().der().to_vec()
    }

    /// A node certificate under a CA with rcgen's default subject, for tests that hold only a CA key.
    #[cfg(test)]
    pub(super) fn generate_node_cert(
        node_id: &NodeId,
        signing_key: &SigningKey,
        ca_key_pair: &KeyPair,
        issued_ms: u64,
    ) -> Result<CertificateDer<'static>, GossipError> {
        node_cert_under(node_id, &public_key_of(signing_key), &default_subject_ca_der(ca_key_pair), ca_key_pair, issued_ms)
    }

    /// A **client** certificate for the principal `token:{issuer}/{name}` (identity-and-lifecycle I1, D1): the
    /// key's certificate, signed by the fleet CA exactly as a node's is, naming `mycelium-client://{issuer}/{name}`
    /// and nothing else — no IP, no `mycelium://` URI — and dated in its serial like a node's.
    pub(super) fn client_cert_under(
        issuer: &str,
        name: &str,
        public: &[u8; 32],
        ca_cert_der: &[u8],
        ca_key_pair: &KeyPair,
        issued_ms: u64,
    ) -> Result<CertificateDer<'static>, GossipError> {
        let err = |reason: String| GossipError::InvalidField { field: "tls", reason };
        if !super::valid_client_segment(issuer) || !super::valid_client_segment(name) {
            return Err(err(format!(
                "TLS: a client principal's issuer and name are each 1-128 of A-Z a-z 0-9 - _ . (got {issuer:?} / {name:?})"
            )));
        }
        if super::RESERVED_CLIENT_NAMES.contains(&name) {
            return Err(err(format!(
                "TLS: the client name {name:?} is reserved: token:{issuer}/{name} is already another credential's principal"
            )));
        }
        let mut params = CertificateParams::new(vec![]).map_err(|e| err(format!("TLS: client cert params: {e}")))?;
        let uri = rcgen::Ia5String::try_from(super::client_uri(issuer, name)).map_err(|e| err(format!("TLS: client URI: {e}")))?;
        params.subject_alt_names = vec![SanType::URI(uri)];
        params.serial_number = Some(rcgen::SerialNumber::from(issued_ms));
        params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        super::set_not_after_for_issue(&mut params);
        issue_checked(params, public, ca_cert_der, ca_key_pair)
    }

    /// A client certificate under a CA with rcgen's default subject, for tests that hold only a CA key.
    #[cfg(test)]
    pub(super) fn generate_client_cert(
        issuer: &str,
        name: &str,
        signing_key: &SigningKey,
        ca_key_pair: &KeyPair,
        issued_ms: u64,
    ) -> Result<CertificateDer<'static>, GossipError> {
        client_cert_under(issuer, name, &public_key_of(signing_key), &default_subject_ca_der(ca_key_pair), ca_key_pair, issued_ms)
    }

    /// **Issue a client certificate off-node** (identity-and-lifecycle I1): loads the CA from `ca_dir` (never mints
    /// one), generates a fresh Ed25519 key for the principal `token:{issuer}/{name}`, and writes
    /// `client-{issuer}-{name}.cert.pem` and `client-{issuer}-{name}.key.pem` (PKCS#8, `0600`) under `out_dir`.
    /// The key goes to the service's process — an environment variable or a file; an agent's shell inherits either,
    /// so a key an agent can reach is that agent's key. The certificate goes to a gateway's
    /// `POST /gateway/identity/client-cert`, which publishes it. Issuing again for the same principal **supersedes**
    /// the earlier key: its serial is later, and the client-cert fold refuses the older one once the newer is read.
    /// Returns the two paths.
    /// **Re-issue a client's existing key** under the CA in `ca_dir` (identity-and-lifecycle I2): a client certificate
    /// for `token:{issuer}/{name}` over `public_key` alone, issued strictly after `floor_ms` (the principal's mark).
    /// Returns its DER and writes nothing. The primitive `tls succeed`'s client re-issue is to call (I2's PR 3); until
    /// then only tests do.
    #[doc(hidden)]
    pub fn issue_client_cert_for_key(ca_dir: &Path, issuer: &str, name: &str, public_key: [u8; 32], floor_ms: u64) -> Result<Vec<u8>, GossipError> {
        let ca_cfg = TlsConfig { auto_cert_dir: ca_dir.to_path_buf(), ..TlsConfig::default() };
        let (ca_cert_der, ca_key_pair) = load_existing_ca(&ca_cfg)?;
        Ok(client_cert_under(issuer, name, &public_key, &ca_cert_der, &ca_key_pair, super::issuance_after(floor_ms))?.as_ref().to_vec())
    }

    pub fn issue_client_cert(ca_dir: &Path, issuer: &str, name: &str, out_dir: &Path) -> Result<(std::path::PathBuf, std::path::PathBuf), GossipError> {
        use base64::{engine::general_purpose::STANDARD, Engine};
        use ed25519_dalek::pkcs8::EncodePrivateKey;
        let err = |reason: String| GossipError::InvalidField { field: "tls", reason };
        let ca_cfg = TlsConfig { auto_cert_dir: ca_dir.to_path_buf(), ..TlsConfig::default() };
        let (ca_cert_der, ca_key_pair) = load_existing_ca(&ca_cfg)?;
        let signing_key = generate_key()?;
        let cert_der = client_cert_under(issuer, name, &public_key_of(&signing_key), &ca_cert_der, &ca_key_pair, super::issuance_after(0))?;
        create_private_dir(out_dir).map_err(|e| err(format!("TLS: create {out_dir:?}: {e}")))?;
        let cert_path = out_dir.join(format!("client-{issuer}-{name}.cert.pem"));
        let key_path = out_dir.join(format!("client-{issuer}-{name}.key.pem"));
        let pkcs8 = signing_key.to_pkcs8_der().map_err(|e| err(format!("TLS: encode client key: {e}")))?;
        let b64 = STANDARD.encode(pkcs8.as_bytes());
        let mut key_pem = String::from("-----BEGIN PRIVATE KEY-----\n");
        for chunk in b64.as_bytes().chunks(64) {
            key_pem.push_str(std::str::from_utf8(chunk).expect("base64 is ascii"));
            key_pem.push('\n');
        }
        key_pem.push_str("-----END PRIVATE KEY-----\n");
        write_public(&cert_path, cert_der_to_pem(cert_der.as_ref()).as_bytes()).map_err(|e| err(format!("TLS: write {cert_path:?}: {e}")))?;
        write_private(&key_path, key_pem.as_bytes()).map_err(|e| err(format!("TLS: write {key_path:?}: {e}")))?;
        Ok((cert_path, key_path))
    }

    /// A P-256 public point as rcgen signs a certificate over it (identity-and-lifecycle I7, the spike S-presence's
    /// row 6): the fleet's Ed25519 CA certifies a consent key over its **public half alone** — the private half is in
    /// the developer's Secure Enclave or software key file and never reaches the issuer.
    pub(super) struct P256Public(pub(super) [u8; 65]);

    impl rcgen::PublicKeyData for P256Public {
        fn der_bytes(&self) -> &[u8] {
            &self.0
        }
        fn algorithm(&self) -> &rcgen::SignatureAlgorithm {
            &rcgen::PKCS_ECDSA_P256_SHA256
        }
    }

    /// Whether this build may write the consent grade extension: [`consent_grade_oid_open`](super::consent_grade_oid_open),
    /// the same gate the reader takes.
    fn may_write_consent_grade() -> bool {
        super::consent_grade_oid_open()
    }

    /// A **consent** certificate for the principal `consent:{issuer}/{name}` (identity-and-lifecycle I7, H1, H2): the
    /// P-256 `point`'s certificate, signed by the fleet CA, naming `mycelium-consent://{issuer}/{name}` and nothing
    /// else, dated in its serial like a client's, and carrying `grade` in the non-critical grade extension. Refused
    /// while the extension's OID is provisional, outside a test build.
    pub(super) fn consent_cert_under(
        issuer: &str,
        name: &str,
        point: &[u8; 65],
        grade: super::ConsentGrade,
        ca_cert_der: &[u8],
        ca_key_pair: &KeyPair,
        issued_ms: u64,
    ) -> Result<CertificateDer<'static>, GossipError> {
        let err = |reason: String| GossipError::InvalidField { field: "tls", reason };
        if !may_write_consent_grade() {
            return Err(err(
                "TLS: consent certificates are not issued by this build: the grade extension's OID is provisional until \
                 the registered IANA Private Enterprise Number arc arrives (identity-and-lifecycle I7)"
                    .into(),
            ));
        }
        if !super::valid_client_segment(issuer) || !super::valid_client_segment(name) {
            return Err(err(format!(
                "TLS: a consent principal's issuer and name are each 1-128 of A-Z a-z 0-9 - _ . (got {issuer:?} / {name:?})"
            )));
        }
        if super::RESERVED_CLIENT_NAMES.contains(&name) {
            return Err(err(format!("TLS: the name {name:?} is reserved")));
        }
        if !super::p256_point_on_curve(point) {
            return Err(err("TLS: not_p256: a consent key is an uncompressed P-256 point on the curve (04 || x || y)".into()));
        }
        let mut params = CertificateParams::new(vec![]).map_err(|e| err(format!("TLS: consent cert params: {e}")))?;
        let uri = rcgen::Ia5String::try_from(super::consent_uri(issuer, name)).map_err(|e| err(format!("TLS: consent URI: {e}")))?;
        params.subject_alt_names = vec![SanType::URI(uri)];
        params.serial_number = Some(rcgen::SerialNumber::from(issued_ms));
        params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        params.not_after  = rcgen::date_time_ymd(2099, 1, 1);
        params.custom_extensions = vec![rcgen::CustomExtension::from_oid_content(super::CONSENT_GRADE_OID, grade.extension_der())];
        issue_checked_over(params, &P256Public(*point), ca_cert_der, ca_key_pair)
    }

    /// A consent certificate under a CA with rcgen's default subject, for tests that hold only a CA key.
    #[cfg(test)]
    pub(super) fn generate_consent_cert(
        issuer: &str,
        name: &str,
        point: &[u8; 65],
        grade: super::ConsentGrade,
        ca_key_pair: &KeyPair,
        issued_ms: u64,
    ) -> Result<CertificateDer<'static>, GossipError> {
        consent_cert_under(issuer, name, point, grade, &default_subject_ca_der(ca_key_pair), ca_key_pair, issued_ms)
    }

    /// **Issue a consent certificate off-node, over a public key alone** (identity-and-lifecycle I7's issue flow, L7,
    /// M-new-1): loads the CA from `ca_dir` (never mints one), reads the developer's P-256 key from `spki_pem`
    /// ([`p256_point_from_spki_pem`](super::p256_point_from_spki_pem): raw X9.63 and any other algorithm refused),
    /// and writes `consent-{issuer}-{name}.cert.pem` under `out_dir` — **and no key file**: the private half never
    /// reaches this host, and the CA key never reaches the developer's. `grade` is the operator's statement of what
    /// the enrolment told it (H2); the CLI refuses `service`, which is no consent grade. Issuing again for the same
    /// principal supersedes the earlier consent key once published (P6), within `consent:{issuer}/{name}` only (H1).
    /// Returns the certificate's path.
    pub fn issue_consent_cert(
        ca_dir: &Path,
        issuer: &str,
        name: &str,
        spki_pem: &str,
        grade: super::ConsentGrade,
        out_dir: &Path,
    ) -> Result<std::path::PathBuf, GossipError> {
        let err = |reason: String| GossipError::InvalidField { field: "tls", reason };
        let point = super::p256_point_from_spki_pem(spki_pem).map_err(|e| err(format!("TLS: --public-key: {e}")))?;
        let ca_cfg = TlsConfig { auto_cert_dir: ca_dir.to_path_buf(), ..TlsConfig::default() };
        let (ca_cert_der, ca_key_pair) = load_existing_ca(&ca_cfg)?;
        let cert_der = consent_cert_under(issuer, name, &point, grade, &ca_cert_der, &ca_key_pair, super::issuance_after(0))?;
        create_private_dir(out_dir).map_err(|e| err(format!("TLS: create {out_dir:?}: {e}")))?;
        let cert_path = out_dir.join(format!("consent-{issuer}-{name}.cert.pem"));
        write_public(&cert_path, cert_der_to_pem(cert_der.as_ref()).as_bytes()).map_err(|e| err(format!("TLS: write {cert_path:?}: {e}")))?;
        Ok(cert_path)
    }

    /// The uncompressed P-256 point of a certificate's `SubjectPublicKeyInfo` (identity-and-lifecycle I7), read from
    /// its structural position by the parser rustls uses, as [`ed25519_key_from_cert_der`] reads an Ed25519 key: the
    /// parsed SPKI compared whole against [`P256_SPKI_HEADER`](super::P256_SPKI_HEADER) then a 65-byte `04` point.
    /// `None` for anything else. Never panics.
    pub fn p256_point_from_cert_der(der: &[u8]) -> Option<[u8; 65]> {
        let der = CertificateDer::from(der);
        let cert = webpki::EndEntityCert::try_from(&der).ok()?;
        let spki = cert.subject_public_key_info();
        let (header, point) = spki.as_ref().split_at_checked(super::P256_SPKI_HEADER.len())?;
        if header != super::P256_SPKI_HEADER || point.len() != 65 || point[0] != 0x04 {
            return None;
        }
        let point: [u8; 65] = point.try_into().ok()?;
        super::p256_point_on_curve(&point).then_some(point)
    }

    /// Extract the Ed25519 public key from a node cert's `SubjectPublicKeyInfo` (audit 2026-07-15,
    /// identity-authentication Phase 1a — `docs/design/identity-authentication.md`), read from its
    /// **structural position** by the X.509 parser rustls itself uses (post-360 plan row F, 2.33.0).
    ///
    /// Before 2.33.0 this scanned the DER for the fixed Ed25519 SPKI prefix and returned the 32 bytes
    /// after its first occurrence, on the argument that the certificate had already been validated
    /// against the cluster CA. That argument covered the issuer, not the bytes: a certificate's serial
    /// number, names and extensions are the issuer's to choose and precede the real SPKI in the
    /// encoding, so a member holding the CA key — every node, before `secure-single-domain` rev 2 —
    /// could mint a certificate that *reads* as another member's key at the handshake anchor
    /// (`GossipStream::peer_ed25519_key`) and passes a removal of its own (`refuse_if_removed`).
    /// `key_extract_tests::a_planted_key_pattern_in_the_serial_does_not_fool_the_structural_read` is
    /// the plant, seen failing on the scan.
    ///
    /// RFC 8410 admits exactly one encoding for an Ed25519 SPKI —
    /// `30 2A 30 05 06 03 2B 65 70 03 21 00 ‖ key[32]` (OID `1.3.101.112`, no parameters, a 33-byte
    /// BIT STRING with 0 unused bits) — so the parsed SPKI is compared whole against that header.
    /// Returns `None` for anything that is not a certificate carrying exactly that: a malformed
    /// input, a certificate for another algorithm, or an SPKI of another shape. Never panics.
    pub fn ed25519_key_from_cert_der(der: &[u8]) -> Option<[u8; 32]> {
        // SEQUENCE(42) { SEQUENCE(5) { OID 1.3.101.112 }, BIT STRING(33) { 0 unused bits, key } }
        const ED25519_SPKI_HEADER: [u8; 12] = [0x30, 0x2A, 0x30, 0x05, 0x06, 0x03, 0x2B, 0x65, 0x70, 0x03, 0x21, 0x00];
        let der = CertificateDer::from(der);
        let cert = webpki::EndEntityCert::try_from(&der).ok()?;
        let spki = cert.subject_public_key_info();
        let (header, key) = spki.as_ref().split_at_checked(ED25519_SPKI_HEADER.len())?;
        if header != ED25519_SPKI_HEADER {
            return None;
        }
        key.try_into().ok()
    }

    /// Install the process-wide ring crypto provider exactly once (idempotent).
    /// rustls 0.23 resolves a process-level `CryptoProvider` when any config builder
    /// runs and panics if none is set; we pin ring (default-features off) rather than
    /// let aws-lc-rs auto-install. A second call — another agent, or a host that
    /// installed one first — is the desired no-op.
    fn ensure_crypto_provider() {
        static INSTALL_CRYPTO_PROVIDER: std::sync::Once = std::sync::Once::new();
        INSTALL_CRYPTO_PROVIDER.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
    }

    /// Build a **server-only** rustls config (no client-cert demand) from a cert chain
    /// and key — the gateway TLS path. Operators call this indirectly via
    /// `GatewayTlsConfig { cert_pem_path, key_pem_path }`; the node-cert-reuse path uses
    /// the config built inside [`build_rustls_configs`].
    pub fn gateway_server_config_from_pem(
        cert_pem: &str,
        key_pem: &str,
    ) -> Result<ServerConfig, GossipError> {
        ensure_crypto_provider();
        let certs = pem_certs(cert_pem.as_bytes())
            .map_err(|e| GossipError::InvalidField { field: "gateway_tls", reason: format!("gateway TLS: parse cert PEM: {e}") })?;
        if certs.is_empty() {
            return Err(GossipError::InvalidField { field: "gateway_tls", reason: "gateway TLS: cert PEM contained no certificates".into() });
        }
        let key = pem_private_key(key_pem.as_bytes())
            .map_err(|e| GossipError::InvalidField { field: "gateway_tls", reason: format!("gateway TLS: parse key PEM: {e}") })?
            .ok_or_else(|| GossipError::InvalidField { field: "gateway_tls", reason: "gateway TLS: key PEM contained no private key".into() })?;
        ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|e| GossipError::InvalidField { field: "gateway_tls", reason: format!("gateway TLS: build server config: {e}") })
    }

    /// What [`build_rustls_configs`] builds: the mesh's server and client configs, the gateway's
    /// certificate, the fleet CA's chain check on a client certificate, and the fleet's check on a
    /// server certificate (chain, removal, revocation).
    pub(super) type RustlsConfigs = (
        ServerConfig,
        ClientConfig,
        Arc<rustls::sign::CertifiedKey>,
        Arc<dyn rustls::server::danger::ClientCertVerifier>,
        Arc<dyn rustls::client::danger::ServerCertVerifier>,
    );

    pub(super) fn build_rustls_configs(
        node_cert_der: CertificateDer<'static>,
        signing_key: &SigningKey,
        roots: &[CertificateDer<'static>],
        removed: Arc<crate::removal::RemovedSet>,
        standing: Arc<crate::removal::KeyStanding>,
    ) -> Result<RustlsConfigs, GossipError> {
        use ed25519_dalek::pkcs8::EncodePrivateKey;

        ensure_crypto_provider();

        // Convert signing key to rustls PrivateKeyDer
        let pkcs8 = signing_key
            .to_pkcs8_der()
            .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: encode key for rustls: {e}") })?;
        let key_der: PrivateKeyDer<'static> =
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pkcs8.as_bytes().to_vec()));

        // The root store: every CA certificate the node trusts — one, except across a CA succession's
        // window (identity-and-lifecycle I2), when it is the old CA's and the successor's.
        let mut root_store = RootCertStore::empty();
        for ca_cert_der in roots {
            root_store
                .add(ca_cert_der.clone())
                .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: add CA to root store: {e}") })?;
        }
        let root_store = Arc::new(root_store);

        // Server config: require client cert verified against CA
        let verifier = WebPkiClientVerifier::builder(Arc::clone(&root_store))
            .build()
            .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: build client verifier: {e}") })?;

        let chain_verifier = Arc::clone(&verifier);
        // Closure plan C5: a removed member's certificate is refused at the handshake; post-360 plan
        // row Q's follow-up: so is a certificate whose key the node it names has revoked.
        let verifier: Arc<dyn rustls::server::danger::ClientCertVerifier> = Arc::new(RemovalAwareClientVerifier {
            inner: verifier,
            removed: Arc::clone(&removed),
            standing: Arc::clone(&standing),
        });
        let mut server_config = ServerConfig::builder()
            .with_client_cert_verifier(verifier)
            .with_single_cert(vec![node_cert_der.clone()], key_der.clone_key())
            .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: build server config: {e}") })?;
        // No session resumption on the mesh, either side: a resumed TLS 1.3 session re-runs no
        // certificate verifier, so a peer that completed one handshake could reconnect after its
        // removal or its key's revocation and be admitted on the cached session.
        server_config.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});
        server_config.send_tls13_tickets = 0;

        // The gateway's certificate: SAME node cert/key, served with NO client-cert demand so
        // ordinary HTTP clients can connect (the node-cert-reuse gateway-TLS path). A key, not a
        // config: the gateway's one config resolves the current key at every handshake.
        let gateway_signer = rustls::crypto::ring::default_provider()
            .key_provider
            .load_private_key(key_der.clone_key())
            .map_err(|e| GossipError::InvalidField { field: "gateway_tls", reason: format!("gateway TLS: load key: {e}") })?;
        let gateway_key = Arc::new(rustls::sign::CertifiedKey::new(vec![node_cert_der.clone()], gateway_signer));

        // Client config: present node cert, verify server against CA
        let server_verifier = rustls::client::WebPkiServerVerifier::builder(Arc::clone(&root_store))
            .build()
            .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: build server verifier: {e}") })?;
        let fleet_server_verifier: Arc<dyn rustls::client::danger::ServerCertVerifier> =
            Arc::new(RemovalAwareServerVerifier { inner: server_verifier, removed, standing });
        let mut client_config = ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::clone(&fleet_server_verifier))
            .with_client_auth_cert(vec![node_cert_der], key_der)
            .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: build client config: {e}") })?;
        client_config.resumption = rustls::client::Resumption::disabled();

        Ok((server_config, client_config, gateway_key, chain_verifier, fleet_server_verifier))
    }

    /// The gateway's server config on the node-cert path: no client-cert demand, and a certificate
    /// resolved from `key` at every handshake (so a rotation is presented at once).
    pub(super) fn live_gateway_server_config(key: Arc<arc_swap::ArcSwap<rustls::sign::CertifiedKey>>) -> ServerConfig {
        ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(LiveGatewayCert(key)))
    }

    /// Presents the node's current gateway certificate.
    #[derive(Debug)]
    struct LiveGatewayCert(Arc<arc_swap::ArcSwap<rustls::sign::CertifiedKey>>);

    impl rustls::server::ResolvesServerCert for LiveGatewayCert {
        fn resolve(&self, _hello: rustls::server::ClientHello<'_>) -> Option<Arc<rustls::sign::CertifiedKey>> {
            Some(self.0.load_full())
        }
    }

    /// Refuse a certificate whose identity key belongs to a removed member (closure plan C5).
    pub(super) fn refuse_if_removed(
        removed: &crate::removal::RemovedSet,
        end_entity: &CertificateDer<'_>,
    ) -> Result<(), rustls::Error> {
        // Fail closed (#611's review, finding 6): every certificate this codebase issues or loads
        // carries the node's Ed25519 identity key (`generate_node_cert`, `issue_node_cert`; `key_pem`
        // is read as an Ed25519 PKCS8 key), so one without it is not a member's — a CA-key holder
        // could otherwise mint a certificate for another algorithm and pass whatever the ledger says.
        let Some(key) = ed25519_key_from_cert_der(end_entity.as_ref()) else {
            return Err(rustls::Error::General("mycelium: a mesh certificate must carry an Ed25519 identity key".into()));
        };
        if removed.is_key_removed(&key) {
            return Err(rustls::Error::General("mycelium: this identity has been removed from the mesh".into()));
        }
        Ok(())
    }

    /// Refuse a **client** or **consent** certificate at the mesh (identity-and-lifecycle I1, D1; I7): one whose
    /// names include a `mycelium-client://` or `mycelium-consent://` URI. A client certificate is the fleet CA's too, so the chain check alone admits it;
    /// this keeps a service's key — which its agent's shell can reach — off the transport. Fails closed: a
    /// certificate whose names this node cannot read is refused (every certificate this tree issues is readable,
    /// node or client, before row Q or after). Run after the CA chain check, in both verifiers, never in
    /// `verify_member_cert`, which the client-cert fold uses to judge a client certificate's issuance.
    pub(super) fn refuse_if_client(end_entity: &CertificateDer<'_>) -> Result<(), rustls::Error> {
        let Some(id) = super::cert_identity(end_entity.as_ref()) else {
            return Err(rustls::Error::General("mycelium: a mesh certificate whose names this node cannot read".into()));
        };
        if id.node_uris.iter().any(|u| u.starts_with(super::CLIENT_URI_SCHEME)) {
            return Err(rustls::Error::General("mycelium: a client certificate cannot join the mesh".into()));
        }
        // Identity-and-lifecycle I7: a consent certificate is P-256 and `refuse_if_removed` refuses it for its key
        // first; this refuses the scheme too, so the refusal does not rest on the key alone (an Ed25519 certificate a
        // CA-key holder minted with the consent scheme).
        if id.node_uris.iter().any(|u| u.starts_with(super::CONSENT_URI_SCHEME)) {
            return Err(rustls::Error::General("mycelium: a consent certificate cannot join the mesh".into()));
        }
        Ok(())
    }

    /// Refuse a certificate whose key has been revoked by its owner (post-360 plan row Q's follow-up,
    /// `docs/design/revocation-authority.md`): a key its node has revoked by a revocation this node
    /// **counted** — one judged against the key while it was anchored for that node (a CA-issued
    /// certificate record naming it), kept for good after (`NodeStanding::counted`; across a restart, through
    /// the journal under `[persistence]`).
    ///
    /// - A certificate naming its node (`mycelium://{node}`) is judged against that node's standing.
    /// - A certificate naming none (issued before row Q) is judged against whichever node counted a
    ///   revocation of its key: a revocation counts only against a key anchored for the revoking node,
    ///   and a key is anchored only for the node its certificate names, so this reaches the key's owner
    ///   and nobody else.
    ///
    /// Counted revocations only: a node may revoke a key without standing for it (its revocations reach
    /// any key not anchored for it), so reading those would let one member refuse another's handshakes,
    /// and a provisional revocation — of a key whose certificate record has not reached this node —
    /// would cut a freshly rotated node off at the transport until the record arrives. The handshake
    /// checks revocation, not the narrower *distrusted* (`KeyStanding::is_distrusted`): a rotated node
    /// activates its new key before every peer holds its certificate record, and must still connect.
    /// Run after the CA chain check, so the names read are ones the fleet CA issued.
    pub(super) fn refuse_if_revoked(
        standing: &crate::removal::KeyStanding,
        end_entity: &CertificateDer<'_>,
    ) -> Result<(), rustls::Error> {
        if standing.is_empty() {
            return Ok(());
        }
        let Some(id) = super::cert_identity(end_entity.as_ref()) else {
            return Err(rustls::Error::General("mycelium: a mesh certificate whose names this node cannot read".into()));
        };
        let named: Vec<NodeId> = id.node_uris.iter()
            .filter_map(|uri| uri.strip_prefix("mycelium://").and_then(|n| n.parse::<NodeId>().ok()))
            .collect();
        let revoker = if named.is_empty() {
            standing.counted_revoker_of(&id.key)
        } else {
            named.into_iter().find(|node| standing.is_counted_revoked(node, &id.key))
        };
        if let Some(node) = revoker {
            return Err(rustls::Error::General(format!("mycelium: {node} has revoked this certificate's key")));
        }
        Ok(())
    }

    /// The CA check, then the removal and revocation checks, for certificates peers present to this
    /// node's server.
    #[derive(Debug)]
    struct RemovalAwareClientVerifier {
        inner: Arc<dyn rustls::server::danger::ClientCertVerifier>,
        removed: Arc<crate::removal::RemovedSet>,
        standing: Arc<crate::removal::KeyStanding>,
    }

    impl rustls::server::danger::ClientCertVerifier for RemovalAwareClientVerifier {
        fn offer_client_auth(&self) -> bool {
            self.inner.offer_client_auth()
        }
        fn client_auth_mandatory(&self) -> bool {
            self.inner.client_auth_mandatory()
        }
        fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
            self.inner.root_hint_subjects()
        }
        fn verify_client_cert(
            &self,
            end_entity: &CertificateDer<'_>,
            intermediates: &[CertificateDer<'_>],
            now: rustls::pki_types::UnixTime,
        ) -> Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
            let verified = self.inner.verify_client_cert(end_entity, intermediates, now)?;
            refuse_if_removed(&self.removed, end_entity)?;
            refuse_if_client(end_entity)?;
            refuse_if_revoked(&self.standing, end_entity)?;
            Ok(verified)
        }
        fn verify_tls12_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            self.inner.verify_tls12_signature(message, cert, dss)
        }
        fn verify_tls13_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            self.inner.verify_tls13_signature(message, cert, dss)
        }
        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            self.inner.supported_verify_schemes()
        }
    }

    /// The CA check, then the removal and revocation checks, for certificates the servers this node
    /// dials present.
    #[derive(Debug)]
    struct RemovalAwareServerVerifier {
        inner: Arc<rustls::client::WebPkiServerVerifier>,
        removed: Arc<crate::removal::RemovedSet>,
        standing: Arc<crate::removal::KeyStanding>,
    }

    impl rustls::client::danger::ServerCertVerifier for RemovalAwareServerVerifier {
        fn verify_server_cert(
            &self,
            end_entity: &CertificateDer<'_>,
            intermediates: &[CertificateDer<'_>],
            server_name: &rustls::pki_types::ServerName<'_>,
            ocsp_response: &[u8],
            now: rustls::pki_types::UnixTime,
        ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
            let verified = self.inner.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)?;
            refuse_if_removed(&self.removed, end_entity)?;
            refuse_if_client(end_entity)?;
            refuse_if_revoked(&self.standing, end_entity)?;
            Ok(verified)
        }
        fn verify_tls12_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            self.inner.verify_tls12_signature(message, cert, dss)
        }
        fn verify_tls13_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            self.inner.verify_tls13_signature(message, cert, dss)
        }
        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            self.inner.supported_verify_schemes()
        }
    }

    /// The fleet's server check as [`NodeTls::fleet_server_verifier`] hands it out: one verifier for the
    /// node's life that delegates, at each call, to the check over the roots the node trusts then
    /// (identity-and-lifecycle I2). A client built once — the bulk fetch's, at `start()` — therefore
    /// follows `activate`'s change of roots without being rebuilt.
    #[derive(Debug)]
    pub(super) struct LiveFleetServerVerifier(pub(super) Arc<arc_swap::ArcSwap<Arc<dyn rustls::client::danger::ServerCertVerifier>>>);

    impl rustls::client::danger::ServerCertVerifier for LiveFleetServerVerifier {
        fn verify_server_cert(
            &self,
            end_entity: &CertificateDer<'_>,
            intermediates: &[CertificateDer<'_>],
            server_name: &rustls::pki_types::ServerName<'_>,
            ocsp_response: &[u8],
            now: rustls::pki_types::UnixTime,
        ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
            self.0.load().verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
        }
        fn verify_tls12_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            self.0.load().verify_tls12_signature(message, cert, dss)
        }
        fn verify_tls13_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            self.0.load().verify_tls13_signature(message, cert, dss)
        }
        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            self.0.load().supported_verify_schemes()
        }
    }

    /// Every `CERTIFICATE` section of a PEM document, in order; any other section (a key, a CRL) and any text between
    /// sections is skipped. A malformed section is an error. With [`pem_private_key`] this is where the node's TLS module
    /// reads the gateway's certificate and key and the CA and node certificates (`pem_cert_to_der`), so the parser behind
    /// them is chosen once (R9 of the external review). It is not the only PEM reader in the tree:
    /// `load_key_from_pkcs8_pem` reads a node's own key more strictly (PKCS#8 only), and `rcgen` and `reqwest` parse PEM
    /// themselves — unifying them is *not built*.
    pub(crate) fn pem_certs(pem: &[u8]) -> Result<Vec<CertificateDer<'static>>, String> {
        use rustls::pki_types::pem::PemObject;
        CertificateDer::pem_slice_iter(pem).collect::<Result<_, _>>().map_err(|e| e.to_string())
    }

    /// The first private key of a PEM document — PKCS#8 (`PRIVATE KEY`), PKCS#1 (`RSA PRIVATE KEY`) or SEC1
    /// (`EC PRIVATE KEY`), whichever comes first; `None` when it holds none. A malformed section is an error.
    pub(crate) fn pem_private_key(pem: &[u8]) -> Result<Option<PrivateKeyDer<'static>>, String> {
        use rustls::pki_types::pem::{self, PemObject};
        match PrivateKeyDer::from_pem_slice(pem) {
            Ok(key) => Ok(Some(key)),
            Err(pem::Error::NoItemsFound) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    pub(crate) fn pem_cert_to_der(pem: &str) -> Result<CertificateDer<'static>, GossipError> {
        let certs: Vec<CertificateDer<'static>> =
            pem_certs(pem.as_bytes())
                .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: parse CA cert PEM: {e}") })?;
        certs
            .into_iter()
            .next()
            .ok_or_else(|| GossipError::InvalidField { field: "tls", reason: "TLS: CA cert PEM contains no certificate".into() })
    }

    pub(crate) fn cert_der_to_pem(der: &[u8]) -> String {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let b64 = STANDARD.encode(der);
        // wrap at 64 chars
        let wrapped: String = b64
            .as_bytes()
            .chunks(64)
            .map(|c| std::str::from_utf8(c).expect("infallible: STANDARD base64 encoding produces ASCII-only bytes"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("-----BEGIN CERTIFICATE-----\n{wrapped}\n-----END CERTIFICATE-----\n")
    }

    // ── Public helpers ────────────────────────────────────────────────────

    pub fn sign_bytes(key: &SigningKey, msg: &[u8]) -> [u8; 64] {
        use ed25519_dalek::Signer;
        key.sign(msg).to_bytes()
    }

    pub fn verify_bytes(pub_key_bytes: &[u8; 32], msg: &[u8], sig: &[u8]) -> bool {
        let Ok(vk) = VerifyingKey::from_bytes(pub_key_bytes) else { return false };
        let Ok(arr): Result<[u8; 64], _> = sig.try_into() else { return false };
        let sig = ed25519_dalek::Signature::from_bytes(&arr);
        vk.verify_strict(msg, &sig).is_ok()
    }
}

#[cfg(feature = "tls")]
pub use imp::{ed25519_key_from_cert_der, gateway_server_config_from_pem, generate_rotation, generate_rotation_after, issue_client_cert, issue_client_cert_for_key, issue_consent_cert, issue_node_cert, p256_point_from_cert_der, issue_node_cert_for_key, load_or_generate, mint_ca, mint_ca_at, commit_successor, held_ca_commitment, pin_ca_commitment, sign_bytes, verify_bytes, writable_by_others};

#[cfg(all(test, feature = "tls"))]
mod key_extract_tests {
    use super::imp::{ed25519_key_from_cert_der, generate_node_cert};
    use crate::node_id::NodeId;
    use ed25519_dalek::SigningKey;
    use rcgen::KeyPair;

    /// Round-trip against a REAL generated node cert: the key extracted from the cert's SPKI must
    /// equal the signing key's verifying key (audit 2026-07-15 identity-auth Phase 1a). This is the
    /// primitive Phase 1b wires into the handshake to anchor a peer's CA-authenticated key.
    #[test]
    fn extracts_the_key_from_a_real_generated_cert() {
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let expected = signing.verifying_key().to_bytes();
        let ca = KeyPair::generate().unwrap();
        let node = NodeId::new("127.0.0.1", 9000).unwrap();
        let cert = generate_node_cert(&node, &signing, &ca, 1).unwrap();
        assert_eq!(ed25519_key_from_cert_der(cert.as_ref()), Some(expected),
            "extracted SPKI key must equal the cert's Ed25519 verifying key");
    }

    #[test]
    fn returns_none_on_absent_or_truncated_pattern_without_panic() {
        assert_eq!(ed25519_key_from_cert_der(&[0u8; 8]), None); // not a certificate
        // The Ed25519 SPKI prefix with fewer than 32 key bytes after it — not a certificate either.
        let truncated = [0x06, 0x03, 0x2B, 0x65, 0x70, 0x03, 0x21, 0x00, 0x01, 0x02];
        assert_eq!(ed25519_key_from_cert_der(&truncated), None);
        assert_eq!(ed25519_key_from_cert_der(&[]), None);
    }

    /// `06 03 2B 65 70 03 21 00` — the Ed25519 SPKI prefix the byte-scan (pre-2.33.0) looked for.
    const SPKI_PREFIX: [u8; 8] = [0x06, 0x03, 0x2B, 0x65, 0x70, 0x03, 0x21, 0x00];

    /// A self-signed certificate for `key` (any algorithm rcgen signs with) whose **serial number**
    /// is `planted` — bytes the issuer chooses freely, and which precede the real
    /// `SubjectPublicKeyInfo` in the encoding.
    fn cert_with_planted_serial(key: &KeyPair, planted: Vec<u8>) -> Vec<u8> {
        let mut params = rcgen::CertificateParams::new(vec![]).unwrap();
        params.subject_alt_names = vec![rcgen::SanType::IpAddress("127.0.0.1".parse().unwrap())];
        params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        params.not_after = rcgen::date_time_ymd(2099, 1, 1);
        params.serial_number = Some(rcgen::SerialNumber::from(planted));
        params.self_signed(key).unwrap().der().to_vec()
    }

    fn rcgen_key(signing: &SigningKey) -> KeyPair {
        use ed25519_dalek::pkcs8::EncodePrivateKey;
        KeyPair::try_from(signing.to_pkcs8_der().unwrap().as_bytes()).unwrap()
    }

    /// **The removal check reads the key the certificate is actually for** (post-360 plan row F).
    ///
    /// Before 2.33.0 `ed25519_key_from_cert_der` scanned the DER for the Ed25519 SPKI prefix and
    /// returned the first 32 bytes after it — wherever in the certificate that was. A certificate
    /// whose serial number carries `prefix ‖ V's key` reads as **V's**, whatever key it was issued
    /// for. The removal check (`refuse_if_removed`) and the handshake anchor
    /// (`GossipStream::peer_ed25519_key`) both read the key this way, so a member holding the CA key
    /// — the default posture before `secure-single-domain` rev 2, where every node holds it — could
    /// mint itself a certificate that anchors as another member and passes a removal of its own
    /// key. Now the key is read from the `SubjectPublicKeyInfo`'s structural position by the X.509
    /// parser rustls uses. Seen failing first: the planted key came back.
    #[test]
    fn a_planted_key_pattern_in_the_serial_does_not_fool_the_structural_read() {
        let victim = SigningKey::from_bytes(&[7u8; 32]).verifying_key().to_bytes();
        let attacker = SigningKey::from_bytes(&[9u8; 32]);
        let mut planted = SPKI_PREFIX.to_vec();
        planted.extend_from_slice(&victim);
        let forgery = cert_with_planted_serial(&rcgen_key(&attacker), planted);
        assert_eq!(
            ed25519_key_from_cert_der(&forgery),
            Some(attacker.verifying_key().to_bytes()),
            "the key read from a certificate is the key it was issued for, not a pattern in its serial",
        );
    }

    /// The same plant, through the check that matters: the attacker's own key is on the removal
    /// ledger, and the decoy in the serial is a key that is not. Seen failing first: `Ok(())`.
    #[test]
    fn a_removed_member_hiding_its_key_behind_a_planted_pattern_is_still_refused() {
        let attacker = SigningKey::from_bytes(&[11u8; 32]);
        let decoy = SigningKey::from_bytes(&[12u8; 32]).verifying_key().to_bytes();
        let mut planted = SPKI_PREFIX.to_vec();
        planted.extend_from_slice(&decoy);
        let forgery = cert_with_planted_serial(&rcgen_key(&attacker), planted);
        let removed = crate::removal::RemovedSet::default();
        removed.remove(&NodeId::new("127.0.0.1", 9001).unwrap(), [attacker.verifying_key().to_bytes()]);
        let cert = rustls::pki_types::CertificateDer::from(forgery);
        assert!(
            super::imp::refuse_if_removed(&removed, &cert).is_err(),
            "a removed member's certificate is refused whatever else its serial number says",
        );
    }

    /// **A certificate with no Ed25519 key is refused by the removal check** (#611's review, finding 6).
    /// Every node certificate this codebase issues or loads carries the node's Ed25519 identity key
    /// (`generate_node_cert`, `issue_node_cert`; `key_pem` is read as an Ed25519 PKCS8 key), so a
    /// certificate without one is not a member's — but the check skipped it, failing open: a CA-key
    /// holder could mint a P-256 certificate and pass whatever the ledger said. Seen failing first:
    /// `Ok(())`, with the ledger empty and with it non-empty.
    #[test]
    fn a_certificate_without_an_ed25519_key_is_refused_by_the_removal_check() {
        let p256 = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let cert = rustls::pki_types::CertificateDer::from(cert_with_planted_serial(&p256, vec![1, 2, 3]));
        let removed = crate::removal::RemovedSet::default();
        assert!(super::imp::refuse_if_removed(&removed, &cert).is_err(), "no Ed25519 key, no member — ledger empty");
        removed.remove(&NodeId::new("127.0.0.1", 9002).unwrap(), [[5u8; 32]]);
        assert!(super::imp::refuse_if_removed(&removed, &cert).is_err(), "and with removals on the ledger");
    }

    /// A certificate for a key that is not Ed25519 has no Ed25519 key to read, even when the
    /// pattern the scan looked for sits in its serial number. Seen failing first: the decoy came back.
    #[test]
    fn a_non_ed25519_certificate_has_no_ed25519_key_whatever_its_serial_carries() {
        let decoy = SigningKey::from_bytes(&[13u8; 32]).verifying_key().to_bytes();
        let mut planted = SPKI_PREFIX.to_vec();
        planted.extend_from_slice(&decoy);
        let p256 = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let cert = cert_with_planted_serial(&p256, planted);
        assert_eq!(ed25519_key_from_cert_der(&cert), None);
    }
}

#[cfg(test)]
#[cfg(feature = "tls")]
mod pem_parse_tests {
    //! The PEM readers' behaviour, pinned across the parser they sit on (R9: `rustls-pemfile` → `rustls-pki-types`).
    //! Five of the eight cases passed on `rustls-pemfile` before the swap; the other three (a malformed key section is an
    //! error naming the parse, CRLF line endings, an encrypted key reads as no key) were added after it, in #721's review.
    //! All eight pass on `rustls-pki-types`.
    use super::imp::{pem_certs, pem_private_key};
    use base64::{engine::general_purpose::STANDARD, Engine};
    use rustls::pki_types::PrivateKeyDer;

    fn section(label: &str, der: &[u8]) -> String {
        let b64 = STANDARD.encode(der);
        let body: Vec<&str> = b64.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).unwrap()).collect();
        format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n", body.join("\n"))
    }

    #[test]
    fn several_certificates_come_back_in_order_and_other_sections_are_skipped() {
        let doc = format!(
            "a comment before\n{}{}text between\n{}{}",
            section("CERTIFICATE", b"first"),
            section("PRIVATE KEY", b"not a cert"),
            section("X509 CRL", b"a crl"),
            section("CERTIFICATE", b"second"),
        );
        let certs = pem_certs(doc.as_bytes()).unwrap();
        let ders: Vec<&[u8]> = certs.iter().map(|c| c.as_ref()).collect();
        assert_eq!(ders, vec![&b"first"[..], &b"second"[..]]);
    }

    #[test]
    fn a_document_with_no_certificate_yields_none_and_garbage_is_not_an_error() {
        assert!(pem_certs(b"").unwrap().is_empty());
        assert!(pem_certs(b"no pem here at all\n").unwrap().is_empty());
        assert!(pem_certs(section("PRIVATE KEY", b"k").as_bytes()).unwrap().is_empty());
    }

    #[test]
    fn a_malformed_certificate_section_is_an_error() {
        let doc = "-----BEGIN CERTIFICATE-----\n!!!not base64!!!\n-----END CERTIFICATE-----\n";
        assert!(pem_certs(doc.as_bytes()).is_err());
        let unterminated = "-----BEGIN CERTIFICATE-----\nAAAA\n";
        assert!(pem_certs(unterminated.as_bytes()).is_err());
    }

    #[test]
    fn each_private_key_form_is_read_by_its_label_and_the_first_one_wins() {
        match pem_private_key(section("PRIVATE KEY", b"pkcs8").as_bytes()).unwrap() {
            Some(PrivateKeyDer::Pkcs8(k)) => assert_eq!(k.secret_pkcs8_der(), b"pkcs8"),
            other => panic!("PKCS#8: {other:?}"),
        }
        match pem_private_key(section("RSA PRIVATE KEY", b"pkcs1").as_bytes()).unwrap() {
            Some(PrivateKeyDer::Pkcs1(k)) => assert_eq!(k.secret_pkcs1_der(), b"pkcs1"),
            other => panic!("PKCS#1: {other:?}"),
        }
        match pem_private_key(section("EC PRIVATE KEY", b"sec1").as_bytes()).unwrap() {
            Some(PrivateKeyDer::Sec1(k)) => assert_eq!(k.secret_sec1_der(), b"sec1"),
            other => panic!("SEC1: {other:?}"),
        }
        let both = format!("{}{}{}", section("CERTIFICATE", b"c"), section("EC PRIVATE KEY", b"one"), section("PRIVATE KEY", b"two"));
        match pem_private_key(both.as_bytes()).unwrap() {
            Some(PrivateKeyDer::Sec1(k)) => assert_eq!(k.secret_sec1_der(), b"one"),
            other => panic!("first key: {other:?}"),
        }
    }

    /// A key section that does not decode is an error naming the parse — not "no private key", which would send an
    /// operator looking for a missing file instead of a corrupt one. Seen failing with `pem_private_key` mapping every
    /// error to `Ok(None)` (the review's plant on #721).
    #[test]
    fn a_malformed_key_section_is_an_error_naming_the_parse() {
        let doc = "-----BEGIN PRIVATE KEY-----\n!!!not base64!!!\n-----END PRIVATE KEY-----\n";
        let err = pem_private_key(doc.as_bytes()).expect_err("a malformed key section is an error");
        assert!(!err.is_empty());
        let cert = section("CERTIFICATE", b"c");
        let refused = super::imp::gateway_server_config_from_pem(&cert, doc).expect_err("refused");
        assert!(refused.to_string().contains("parse key PEM"), "{refused}");
    }

    /// A PEM file written on Windows (CRLF line endings) reads as the same certificate and key.
    #[test]
    fn crlf_line_endings_parse_as_the_same_certificate_and_key() {
        let lf = format!("{}{}", section("CERTIFICATE", b"a certificate body"), section("PRIVATE KEY", b"a key body"));
        let crlf = lf.replace('\n', "\r\n");
        let certs = pem_certs(crlf.as_bytes()).unwrap();
        assert_eq!(certs.len(), 1);
        assert_eq!(certs[0].as_ref(), b"a certificate body");
        match pem_private_key(crlf.as_bytes()).unwrap() {
            Some(PrivateKeyDer::Pkcs8(k)) => assert_eq!(k.secret_pkcs8_der(), b"a key body"),
            other => panic!("CRLF key: {other:?}"),
        }
    }

    /// An encrypted PKCS#8 key is not a key the node can use; it reads as "no private key", and the gateway says so.
    #[test]
    fn an_encrypted_private_key_reads_as_no_private_key() {
        let doc = section("ENCRYPTED PRIVATE KEY", b"ciphertext");
        assert!(pem_private_key(doc.as_bytes()).unwrap().is_none());
        let cert = section("CERTIFICATE", b"c");
        let refused = super::imp::gateway_server_config_from_pem(&cert, &doc).expect_err("refused");
        assert!(refused.to_string().contains("contained no private key"), "{refused}");
    }

    #[test]
    fn a_document_with_no_private_key_yields_none() {
        assert!(pem_private_key(b"").unwrap().is_none());
        assert!(pem_private_key(section("CERTIFICATE", b"c").as_bytes()).unwrap().is_none());
    }
}

/// The shared-CA race (federation suite: "b1 sees its one peer"): nodes that share `auto_cert_dir`
/// and start together must all end up with **one** CA.
#[cfg(test)]
#[cfg(feature = "tls")]
mod ca_race_tests {
    use super::imp::load_or_create_ca;
    use crate::config::TlsConfig;
    use std::sync::{Arc, Barrier};

    fn fresh_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("myc-ca-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// `mint_ca` under contention: eight concurrent mints into one directory publish exactly one CA, the other seven are
    /// refused, and the key on disk is the certificate's — never one mint's key beside another's certificate. Seen
    /// failing first with the check-then-rename publish: several mints returned `Ok` and the last rename won.
    #[test]
    fn concurrent_mints_publish_one_ca_and_refuse_the_rest() {
        let dir = fresh_dir("mint-race").join("ca");
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let handles: Vec<_> = (0..8).map(|_| {
            let (dir, barrier) = (dir.clone(), Arc::clone(&barrier));
            std::thread::spawn(move || { barrier.wait(); super::imp::mint_ca(&dir) })
        }).collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1, "{results:?}");
        let key = rcgen::KeyPair::from_pem(&std::fs::read_to_string(dir.join("ca-key.pem")).unwrap()).unwrap();
        let cert = super::imp::pem_cert_to_der(&std::fs::read_to_string(dir.join("ca-cert.pem")).unwrap()).unwrap();
        let spki = webpki::EndEntityCert::try_from(&cert).unwrap().subject_public_key_info().as_ref().to_vec();
        // `subject_public_key_info` is the SPKI's contents; the key pair's DER is the whole SEQUENCE around them.
        assert!(key.public_key_der().ends_with(&spki), "the published key is the published certificate's");
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 2, "no temporary file is left: {left:?}");
    }

    /// Every private key the TLS module writes is readable by its owner only: the CA key, an issued node key and a
    /// node's own generated identity key are created `0600` whatever the umask, and a key file left `0644` by an older
    /// release is narrowed when it is rewritten. Certificates stay as the umask leaves them. Seen failing first: all
    /// three were `0644` under umask 022.
    #[cfg(unix)]
    #[test]
    fn every_private_key_written_is_owner_only() {
        use crate::node_id::NodeId;
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let ca_dir = fresh_dir("modes-ca");
        load_or_create_ca(&TlsConfig { auto_cert_dir: ca_dir.clone(), ..TlsConfig::default() }).unwrap();
        assert_eq!(mode(&ca_dir.join("ca-key.pem")), 0o600, "the CA key");
        let out = fresh_dir("modes-issued");
        let node = NodeId::new("127.0.0.1", 9402).unwrap();
        let stale = out.join("127_0_0_1_9402.key.pem");
        std::fs::write(&stale, b"left by an older release").unwrap();
        std::fs::set_permissions(&stale, std::fs::Permissions::from_mode(0o644)).unwrap();
        let (_cert, key) = super::imp::issue_node_cert(&ca_dir, &node, &out).unwrap();
        assert_eq!(mode(&key), 0o600, "an issued node key, rewritten over a 0644 file");
        let own = fresh_dir("modes-own");
        std::fs::copy(ca_dir.join("ca-cert.pem"), own.join("ca-cert.pem")).unwrap();
        std::fs::copy(ca_dir.join("ca-key.pem"), own.join("ca-key.pem")).unwrap();
        let cfg = TlsConfig { auto_cert_dir: own.clone(), ..TlsConfig::default() };
        let node = NodeId::new("127.0.0.1", 9403).unwrap();
        super::imp::load_or_generate(&cfg, &node, Arc::new(crate::removal::RemovedSet::default()), Default::default()).unwrap();
        assert_eq!(mode(&own.join("127_0_0_1_9403.key")), 0o600, "a node's own generated identity key");
        for d in [ca_dir, out, own] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    /// Eight nodes start at the same instant on one empty directory, twenty times over. Every one
    /// must hold the same CA, and it must be the CA on disk. Before the fix, each could generate its
    /// own — the root cause of the intermittent federation peering failures.
    #[test]
    fn nodes_starting_together_on_a_shared_dir_all_get_one_ca() {
        for round in 0..20 {
            let dir = fresh_dir(&format!("race-{round}"));
            let cfg = Arc::new(TlsConfig { auto_cert_dir: dir.clone(), ..TlsConfig::default() });
            let barrier = Arc::new(Barrier::new(8));
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    let (cfg, barrier) = (Arc::clone(&cfg), Arc::clone(&barrier));
                    std::thread::spawn(move || {
                        barrier.wait();
                        load_or_create_ca(&cfg).expect("CA").0.as_ref().to_vec()
                    })
                })
                .collect();
            let cas: Vec<Vec<u8>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
            assert!(cas.windows(2).all(|w| w[0] == w[1]), "round {round}: nodes hold different CAs");
            let on_disk = super::imp::pem_cert_to_der(&std::fs::read_to_string(dir.join("ca-cert.pem")).unwrap()).unwrap();
            assert_eq!(on_disk.as_ref(), cas[0].as_slice(), "round {round}: the CA on disk differs");
            assert!(!dir.join("ca.lock").exists(), "round {round}: the lock was left behind");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// An existing CA is loaded, never replaced.
    #[test]
    fn an_existing_ca_is_loaded_not_replaced() {
        let dir = fresh_dir("existing");
        let cfg = TlsConfig { auto_cert_dir: dir.clone(), ..TlsConfig::default() };
        let first = load_or_create_ca(&cfg).unwrap().0.as_ref().to_vec();
        let second = load_or_create_ca(&cfg).unwrap().0.as_ref().to_vec();
        assert_eq!(first, second);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A lock left by a process that died mid-generation is an **error naming the lock** — never a
    /// second, different CA.
    #[test]
    fn a_stale_lock_is_an_error_not_a_second_ca() {
        let dir = fresh_dir("stale");
        std::fs::write(dir.join("ca.lock"), b"").unwrap();
        let cfg = TlsConfig { auto_cert_dir: dir.clone(), ..TlsConfig::default() };
        let Err(e) = load_or_create_ca(&cfg) else { panic!("a stale lock must not yield a CA") };
        assert!(e.to_string().contains("ca.lock"), "{e}");
        assert!(!dir.join("ca-cert.pem").exists(), "no CA was generated behind the lock");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Adopter-surface Y8: `mint_ca` is the issuer host's first step — a CA `issue_node_cert` signs with — and a
    /// second mint over an existing CA (either file) is refused by name, leaving the first untouched.
    #[test]
    fn mint_ca_makes_a_ca_that_issues_and_refuses_a_second() {
        use crate::node_id::NodeId;
        let ca_dir = fresh_dir("mint").join("ca");
        let cert = super::imp::mint_ca(&ca_dir).expect("mints into a new directory");
        assert_eq!(cert, ca_dir.join("ca-cert.pem"));
        assert!(ca_dir.join("ca-key.pem").exists());
        let before = std::fs::read(&cert).unwrap();
        let err = super::imp::mint_ca(&ca_dir).expect_err("a second CA over the first");
        assert!(err.to_string().contains("ca-cert.pem") && err.to_string().contains("minted once"), "{err}");
        assert_eq!(std::fs::read(&cert).unwrap(), before, "the first CA is untouched");
        std::fs::remove_file(&cert).unwrap();
        assert!(super::imp::mint_ca(&ca_dir).expect_err("the key alone also refuses").to_string().contains("ca-key.pem"));
        std::fs::write(&cert, &before).unwrap();
        let node_dir = fresh_dir("mint-node");
        super::imp::issue_node_cert(&ca_dir, &NodeId::new("127.0.0.1", 9401).unwrap(), &node_dir).expect("the minted CA issues");
    }

    /// A mint takes the lock a starting node takes: while another process holds `ca.lock` in the directory, `mint_ca`
    /// publishes nothing and is refused naming the lock — never a mint racing a node's generation, whose rename would
    /// replace the minted key and whose certificate would then make the mint's cleanup delete the node's key (#695's
    /// confirmation round, follow-up 2). Seen failing first: the mint ignored the lock and published.
    #[test]
    fn a_mint_takes_the_ca_lock_and_publishes_nothing_while_another_holds_it() {
        let dir = fresh_dir("mint-lock");
        std::fs::write(dir.join("ca.lock"), b"").unwrap();
        let err = super::imp::mint_ca(&dir).expect_err("another process holds the lock");
        assert!(err.to_string().contains("ca.lock"), "{err}");
        assert!(!dir.join("ca-key.pem").exists() && !dir.join("ca-cert.pem").exists(), "nothing published behind the lock");
        std::fs::remove_file(dir.join("ca.lock")).unwrap();
        super::imp::mint_ca(&dir).expect("mints once the lock is gone");
        assert!(!dir.join("ca.lock").exists(), "the mint releases the lock");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Mints and node starts racing on one empty directory: whatever wins, the key on disk is the certificate's, every
    /// node holds the CA on disk, and no lock is left.
    #[test]
    fn mints_and_node_starts_racing_leave_one_ca() {
        for round in 0..10 {
            let dir = fresh_dir(&format!("mint-node-race-{round}"));
            let cfg = Arc::new(TlsConfig { auto_cert_dir: dir.clone(), ..TlsConfig::default() });
            let barrier = Arc::new(Barrier::new(8));
            let nodes: Vec<_> = (0..4).map(|_| {
                let (cfg, barrier) = (Arc::clone(&cfg), Arc::clone(&barrier));
                std::thread::spawn(move || { barrier.wait(); load_or_create_ca(&cfg).map(|c| c.0.as_ref().to_vec()) })
            }).collect();
            let mints: Vec<_> = (0..4).map(|_| {
                let (dir, barrier) = (dir.clone(), Arc::clone(&barrier));
                std::thread::spawn(move || { barrier.wait(); super::imp::mint_ca(&dir).is_ok() })
            }).collect();
            let held: Vec<_> = nodes.into_iter().map(|h| h.join().unwrap()).collect();
            let minted = mints.into_iter().map(|h| h.join().unwrap()).filter(|ok| *ok).count();
            assert!(minted <= 1, "round {round}: {minted} mints each reported a CA");
            let key = rcgen::KeyPair::from_pem(&std::fs::read_to_string(dir.join("ca-key.pem")).unwrap()).unwrap();
            let cert = super::imp::pem_cert_to_der(&std::fs::read_to_string(dir.join("ca-cert.pem")).unwrap()).unwrap();
            let spki = webpki::EndEntityCert::try_from(&cert).unwrap().subject_public_key_info().as_ref().to_vec();
            assert!(key.public_key_der().ends_with(&spki), "round {round}: the key on disk is the certificate's");
            for h in held.into_iter().flatten() {
                assert_eq!(h, cert.as_ref().to_vec(), "round {round}: a node holds a CA other than the one on disk");
            }
            assert!(!dir.join("ca.lock").exists(), "round {round}: a lock was left behind");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// A mint interrupted between its two links leaves `ca-key.pem` without `ca-cert.pem`. That state is named — a mint
    /// that did not finish, from which nothing can have been issued, and how to recover — by `tls ca` and by `tls
    /// issue`, rather than "a CA is minted once" or a bare read error (#695's confirmation round, follow-up 3). Seen
    /// failing first: both said nothing of an interrupted mint.
    #[test]
    fn a_key_without_its_certificate_is_named_an_interrupted_mint() {
        use crate::node_id::NodeId;
        let dir = fresh_dir("half");
        super::imp::mint_ca(&dir).unwrap();
        std::fs::remove_file(dir.join("ca-cert.pem")).unwrap();
        // What a crash between the links leaves beside the key: the mint's own temporary hard link to it.
        std::fs::copy(dir.join("ca-key.pem"), dir.join(".ca-key.mint-1-dead")).unwrap();
        let again = super::imp::mint_ca(&dir).expect_err("a half-published CA").to_string();
        assert!(again.contains("interrupted") && again.contains("remove") && again.contains(".ca-key.mint-1-dead"), "{again}");
        let out = fresh_dir("half-out");
        let issue = super::imp::issue_node_cert(&dir, &NodeId::new("127.0.0.1", 9404).unwrap(), &out).expect_err("no CA to issue from").to_string();
        assert!(issue.contains("interrupted"), "{issue}");
        for d in [dir, out] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    /// The trust anchor keeps its integrity whatever the umask: a directory the TLS module creates for keys and
    /// certificates is `0700`, and every certificate it writes is `0644` — narrowed when it rewrites a file left wider
    /// (#695's confirmation round, follow-up 4). An existing directory is the operator's and is left as it is. Seen
    /// failing first: the directories were `0755` under umask 022 and a rewritten `0666` certificate stayed `0666`.
    #[cfg(unix)]
    #[test]
    fn key_directories_are_owner_only_and_certificates_are_0644() {
        use crate::node_id::NodeId;
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let root = fresh_dir("dir-modes");
        let ca_dir = root.join("ca");
        super::imp::mint_ca(&ca_dir).unwrap();
        assert_eq!(mode(&ca_dir), 0o700, "the CA directory mint_ca creates");
        assert_eq!(mode(&ca_dir.join("ca-cert.pem")), 0o644, "the minted CA certificate");
        let out = root.join("issued");
        let node = NodeId::new("127.0.0.1", 9405).unwrap();
        let (cert, _key) = super::imp::issue_node_cert(&ca_dir, &node, &out).unwrap();
        assert_eq!(mode(&out), 0o700, "the directory issue_node_cert creates");
        std::fs::set_permissions(&cert, std::fs::Permissions::from_mode(0o666)).unwrap();
        std::fs::remove_file(out.join("127_0_0_1_9405.key.pem")).unwrap();
        super::imp::issue_node_cert(&ca_dir, &node, &out).unwrap();
        assert_eq!(mode(&cert), 0o644, "a certificate rewritten over a 0666 file");
        let own = root.join("own");
        let cfg = TlsConfig { auto_cert_dir: own.clone(), ..TlsConfig::default() };
        super::imp::load_or_generate(&cfg, &NodeId::new("127.0.0.1", 9406).unwrap(), Arc::new(crate::removal::RemovedSet::default()), Default::default()).unwrap();
        assert_eq!(mode(&own), 0o700, "the auto_cert_dir a node creates");
        assert_eq!(mode(&own.join("ca-cert.pem")), 0o644, "a CA certificate a node generates");
        let existing = fresh_dir("dir-modes-existing");
        std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o755)).unwrap();
        super::imp::issue_node_cert(&ca_dir, &NodeId::new("127.0.0.1", 9407).unwrap(), &existing).unwrap();
        assert_eq!(mode(&existing), 0o755, "an existing directory is left as the operator made it");
        for d in [root, existing] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    /// A lone `ca-key.pem` with nothing a crash leaves beside it is a CA whose certificate was **lost**, not an
    /// interrupted mint: it may have issued every node's certificate, so neither `tls ca`, `tls issue` nor a node start
    /// tells anyone to delete it — they say the certificate is missing and that every node holds a copy, and nothing
    /// is minted over it (#699's review, findings 1 and 6). Seen failing first: `tls ca` and `tls issue` said "remove
    /// the key", and a node start minted a new CA over the fleet's key.
    #[test]
    fn a_lost_certificate_is_never_read_as_an_interrupted_mint() {
        use crate::node_id::NodeId;
        let dir = fresh_dir("lost-cert");
        super::imp::mint_ca(&dir).unwrap();
        let out = fresh_dir("lost-cert-out");
        super::imp::issue_node_cert(&dir, &NodeId::new("127.0.0.1", 9408).unwrap(), &out).unwrap();
        std::fs::remove_file(dir.join("ca-cert.pem")).unwrap();
        let key_before = std::fs::read(dir.join("ca-key.pem")).unwrap();
        for (door, msg) in [
            ("tls ca", super::imp::mint_ca(&dir).expect_err("a lost certificate").to_string()),
            ("tls issue", super::imp::issue_node_cert(&dir, &NodeId::new("127.0.0.1", 9409).unwrap(), &out).expect_err("no certificate").to_string()),
            ("a node start", load_or_create_ca(&TlsConfig { auto_cert_dir: dir.clone(), ..TlsConfig::default() }).expect_err("refused").to_string()),
        ] {
            assert!(msg.contains("missing") && msg.contains("copy"), "{door}: {msg}");
            assert!(!msg.contains("interrupted") && !msg.contains("remove"), "{door} must not advise deleting the key: {msg}");
        }
        assert_eq!(std::fs::read(dir.join("ca-key.pem")).unwrap(), key_before, "nothing minted over the fleet's key");
        assert!(!dir.join("ca-cert.pem").exists() && !dir.join("ca.lock").exists());
        for d in [dir, out] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    /// The crash a node's CA generation can suffer between its two renames — the key published, the certificate not —
    /// leaves the certificate's temporary beside the key, so every door reads it as an interrupted creation, even once
    /// the stale `ca.lock` has been removed as the lock's own message says (#699's confirmation round, N1). Seen
    /// failing first: the certificate's temporary was written only after the key's rename, so the crash left no marker
    /// and the lone key read as a lost certificate to be kept.
    #[test]
    fn a_crash_between_a_nodes_two_renames_reads_as_an_interrupted_creation() {
        let dir = fresh_dir("node-crash");
        let (cert_der, key) = super::imp::generate_ca_for_test();
        let staged = super::imp::stage_ca(&dir, &cert_der, &key).unwrap();
        std::fs::rename(&staged.key, dir.join("ca-key.pem")).unwrap(); // … and the process dies here
        let said = super::imp::mint_ca(&dir).expect_err("a half-published CA").to_string();
        assert!(said.contains("interrupted") && said.contains(".cert.tmp-"), "{said}");
        let at_start = load_or_create_ca(&TlsConfig { auto_cert_dir: dir.clone(), ..TlsConfig::default() }).expect_err("refused").to_string();
        assert!(at_start.contains("interrupted"), "{at_start}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A mint sweeps, while it holds the lock, the temporaries an earlier interrupted mint left — the hard link to a
    /// dead CA's key among them — so no dead key stays beside the new one (#699's review, finding 2). Seen failing
    /// first: the stale temporary survived the new mint.
    #[test]
    fn a_mint_sweeps_the_temporaries_a_crashed_mint_left() {
        let dir = fresh_dir("sweep");
        std::fs::write(dir.join(".ca-key.mint-1-dead"), b"a dead CA's key").unwrap();
        std::fs::write(dir.join(".ca-cert.mint-1-dead"), b"its certificate").unwrap();
        super::imp::mint_ca(&dir).expect("mints over nothing but stale temporaries");
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(left.iter().all(|n| !n.contains(".mint-")), "a stale temporary survived: {left:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An existing directory or CA certificate is never narrowed silently, but one writable by group or others is said:
    /// `writable_by_others` names each with the `chmod` to run, for the start log and `mycelium tls` (#699's review,
    /// finding 3). Seen failing first against a stub that named nothing.
    #[cfg(unix)]
    #[test]
    fn a_trust_anchor_others_could_replace_is_said_with_the_chmod() {
        use std::os::unix::fs::PermissionsExt;
        let dir = fresh_dir("writable");
        super::imp::mint_ca(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(super::imp::writable_by_others(&dir).is_empty(), "a 0700 directory and a 0644 certificate say nothing");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        std::fs::set_permissions(dir.join("ca-cert.pem"), std::fs::Permissions::from_mode(0o666)).unwrap();
        let said = super::imp::writable_by_others(&dir);
        assert!(said.iter().any(|w| w.contains("chmod 700")), "{said:?}");
        assert!(said.iter().any(|w| w.contains("chmod 644") && w.contains("ca-cert.pem")), "{said:?}");
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o777, "never narrowed silently");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o1777)).unwrap();
        let sticky = super::imp::writable_by_others(&dir);
        assert!(!sticky.iter().any(|w| w.contains("chmod 700")), "a sticky directory lets no one replace another's file: {sticky:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Missing parents of a key directory are created `0700` too, so a permissive umask cannot leave a world-writable
    /// directory above the trust anchor (#699's review, finding 4). Seen failing first: the parents were `0755`.
    #[cfg(unix)]
    #[test]
    fn missing_parents_of_a_key_directory_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let root = fresh_dir("parents");
        super::imp::mint_ca(&root.join("a").join("b").join("ca")).unwrap();
        assert_eq!(mode(&root.join("a")), 0o700, "a missing grandparent");
        assert_eq!(mode(&root.join("a").join("b")), 0o700, "a missing parent");
        assert_ne!(mode(&root), 0o700, "an existing ancestor is left as it was");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The lock is released however its holder ends — an error or a panic — so a crash inside the process never
    /// leaves `ca.lock` behind (#699's review, finding 7). Seen failing first against a guard without a `Drop`.
    #[test]
    fn the_ca_lock_is_released_when_its_holder_panics() {
        let dir = fresh_dir("lock-guard");
        let lock = dir.join("ca.lock");
        let held = lock.clone();
        let r = std::panic::catch_unwind(move || {
            let _guard = super::imp::take_ca_lock(&held).unwrap();
            panic!("the holder dies while holding the lock");
        });
        assert!(r.is_err());
        assert!(!lock.exists(), "a panic left ca.lock behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `id.ca_key_off_node` (plan §8): a node given a **pre-issued** certificate starts from the CA
    /// *certificate* alone — it neither needs nor looks for the CA key, and never mints a CA.
    /// Before the fix, `load_or_generate` ignored `cert_pem`, re-signed a node cert with the CA key,
    /// and — the key being absent — minted a **new CA** into the node's directory, key included.
    #[test]
    fn a_pre_issued_node_cert_starts_without_the_ca_key_and_mints_nothing() {
        use crate::node_id::NodeId;
        let ca_dir = fresh_dir("issuer");
        let node_dir = fresh_dir("node");
        let node = NodeId::new("127.0.0.1", 9400).unwrap();
        let ca_cfg = TlsConfig { auto_cert_dir: ca_dir.clone(), ..TlsConfig::default() };
        let ca_cert = load_or_create_ca(&ca_cfg).unwrap().0.as_ref().to_vec();
        let (cert_pem, key_pem) = super::imp::issue_node_cert(&ca_dir, &node, &node_dir).unwrap();
        std::fs::copy(ca_dir.join("ca-cert.pem"), node_dir.join("ca-cert.pem")).unwrap();
        assert!(!node_dir.join("ca-key.pem").exists(), "the key stays with the issuer");

        let cfg = TlsConfig { auto_cert_dir: node_dir.clone(), cert_pem: Some(cert_pem.clone()), key_pem: Some(key_pem.clone()), ca_cert_pem: None };
        let removed = Arc::new(crate::removal::RemovedSet::default());
        super::imp::load_or_generate(&cfg, &node, Arc::clone(&removed), Default::default()).expect("a pre-issued cert and the CA certificate are enough");
        assert!(!node_dir.join("ca-key.pem").exists(), "the node never minted a CA key");
        assert!(!node_dir.join("ca.lock").exists());
        let on_disk = super::imp::pem_cert_to_der(&std::fs::read_to_string(node_dir.join("ca-cert.pem")).unwrap()).unwrap();
        assert_eq!(on_disk.as_ref(), ca_cert.as_slice(), "the issuer's CA, not a fresh one");

        // A certificate without its key is refused by name; a missing CA certificate is an error, not a mint.
        let half = TlsConfig { key_pem: None, ..cfg.clone() };
        let Err(e) = super::imp::load_or_generate(&half, &node, Arc::clone(&removed), Default::default()) else { panic!("a cert without its key must be refused") };
        assert!(e.to_string().contains("key_pem"), "{e}");
        std::fs::remove_file(node_dir.join("ca-cert.pem")).unwrap();
        let Err(e) = super::imp::load_or_generate(&cfg, &node, removed, Default::default()) else { panic!("a missing CA cert must be an error, not a mint") };
        assert!(e.to_string().contains("CA cert"), "{e}");
        assert!(!node_dir.join("ca-cert.pem").exists() && !node_dir.join("ca-key.pem").exists(), "nothing minted");
        let _ = std::fs::remove_dir_all(&ca_dir);
        let _ = std::fs::remove_dir_all(&node_dir);
    }

    /// A certificate for `key` the fleet CA in `dir` issued **without** a `mycelium://` URI — what every
    /// node certificate was before row Q.
    fn legacy_cert(cfg: &TlsConfig, node: &crate::node_id::NodeId, key: &ed25519_dalek::SigningKey) -> rustls::pki_types::CertificateDer<'static> {
        use ed25519_dalek::pkcs8::EncodePrivateKey;
        let (_, ca_key) = load_or_create_ca(cfg).unwrap();
        let mut ca_params = rcgen::CertificateParams::new(vec![]).unwrap();
        ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        ca_params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        ca_params.not_after = rcgen::date_time_ymd(2099, 1, 1);
        let ca = ca_params.self_signed(&ca_key).unwrap();
        let kp = rcgen::KeyPair::try_from(key.to_pkcs8_der().unwrap().as_bytes()).unwrap();
        let mut p = rcgen::CertificateParams::new(vec![]).unwrap();
        p.subject_alt_names = vec![rcgen::SanType::IpAddress(node.to_socket_addr().ip())];
        p.not_before = rcgen::date_time_ymd(2024, 1, 1);
        p.not_after = rcgen::date_time_ymd(2099, 1, 1);
        rustls::pki_types::CertificateDer::from(p.signed_by(&kp, &ca, &ca_key).unwrap().der().to_vec())
    }

    /// **A certificate naming no node is refused once its key's owner revoked it** (the review of row Q's
    /// follow-up, finding 1). A node that ran before row Q on a pre-issued certificate without a URI
    /// (valid to 2099) is re-issued one with a URI and revokes the old key; whoever stole the old key
    /// and its certificate presents the URI-less one. The owner counted the revocation, so it is refused —
    /// judged against the owner, found by the key, never against another node's revocation (the plant:
    /// the same key revoked, uncounted, under another node refuses nothing). The standing is set
    /// directly: the fold counts a revocation of K0 only if K0 itself once had a certificate record
    /// naming its node; a key that never did is not refused at the handshake (stated). Seen failing
    /// first: the URI-less certificate was accepted.
    #[test]
    fn a_revoked_key_is_refused_behind_a_certificate_that_names_no_node() {
        use crate::node_id::NodeId;
        use crate::removal::{KeyStanding, NodeStanding, RemovedSet};
        let dir = fresh_dir("revoked-legacy");
        let cfg = TlsConfig { auto_cert_dir: dir.clone(), ..TlsConfig::default() };
        let (nb, nc, other) = (
            NodeId::new("127.0.0.1", 9521).unwrap(),
            NodeId::new("127.0.0.1", 9522).unwrap(),
            NodeId::new("127.0.0.1", 9523).unwrap(),
        );
        let b_standing = Arc::new(KeyStanding::default());
        let b = super::imp::load_or_generate(&cfg, &nb, Arc::new(RemovedSet::default()), Arc::clone(&b_standing)).unwrap();
        let k0 = ed25519_dalek::SigningKey::from_bytes(&[21u8; 32]);
        let k0_pub = k0.verifying_key().to_bytes();
        let legacy = legacy_cert(&cfg, &nc, &k0);
        assert!(super::cert_identity(legacy.as_ref()).is_some_and(|id| id.node_uris.is_empty()), "the certificate names no node");
        let ca_der = load_or_create_ca(&cfg).unwrap().0;
        let (_, thief, _, _, _) = super::imp::build_rustls_configs(legacy, &k0, &[ca_der], Arc::new(RemovedSet::default()), Default::default()).unwrap();
        let thief = Arc::new(thief);

        b_standing.set(&other, NodeStanding { anchored: Some([[2u8; 32]].into_iter().collect()), revoked: [k0_pub].into_iter().collect(), counted: Default::default() });
        assert_eq!(handshake(Arc::clone(&thief), b.server_config()), Ok(()),
            "plant: a revocation by a node the key is not anchored for refuses nothing");

        b_standing.set(&nc, NodeStanding { anchored: Some([k0_pub, [3u8; 32]].into_iter().collect()), revoked: [k0_pub].into_iter().collect(), counted: [k0_pub].into_iter().collect() });
        let after = handshake(thief, b.server_config());
        let _ = std::fs::remove_dir_all(&dir);
        assert!(after.as_ref().is_err_and(|e| e.starts_with("server refused") && e.contains(&nc.to_string())),
            "REVOKED: a URI-less certificate for a key its owner revoked was accepted: {after:?}");
    }

    /// Drive one handshake between `client` and `server` in memory; `Err` names the side that refused.
    fn handshake(client: Arc<rustls::ClientConfig>, server: Arc<rustls::ServerConfig>) -> Result<(), String> {
        let name = rustls::pki_types::ServerName::try_from("127.0.0.1").unwrap().to_owned();
        let mut c = rustls::ClientConnection::new(client, name).unwrap();
        let mut s = rustls::ServerConnection::new(server).unwrap();
        for _ in 0..16 {
            let mut buf = Vec::new();
            while c.wants_write() { c.write_tls(&mut buf).unwrap(); }
            if !buf.is_empty() {
                s.read_tls(&mut buf.as_slice()).unwrap();
                s.process_new_packets().map_err(|e| format!("server refused: {e}"))?;
            }
            let mut buf = Vec::new();
            while s.wants_write() { s.write_tls(&mut buf).unwrap(); }
            if !buf.is_empty() {
                c.read_tls(&mut buf.as_slice()).unwrap();
                c.process_new_packets().map_err(|e| format!("client refused: {e}"))?;
            }
            if !c.is_handshaking() && !s.is_handshaking() { return Ok(()); }
        }
        Err("the handshake did not finish".into())
    }

    /// **A revoked key's certificate is refused at the mTLS handshake, both ways** (post-360 plan row
    /// Q's follow-up). Row Q made a revoked key stop verifying at every signature check, but the
    /// handshake judged only the CA chain and the removal ledger, so the holder of a revoked key and
    /// its certificate — still CA-valid until it expires — was a connected peer. Now a certificate whose
    /// key the node it names (`mycelium://{node}`) has revoked is refused, by the server and by a
    /// dialling client. The plants: the same certificate passes before the revocation, and a revocation
    /// of that key in **another** node's standing refuses nothing (a node's revocations reach only keys
    /// judged for it, so one member cannot cut another off). Seen failing first: both handshakes
    /// completed after the revocation.
    #[test]
    fn a_revoked_keys_certificate_is_refused_at_the_handshake_both_ways() {
        use crate::node_id::NodeId;
        use crate::removal::{KeyStanding, NodeStanding, RemovedSet};
        let dir = fresh_dir("revoked-handshake");
        let cfg = TlsConfig { auto_cert_dir: dir.clone(), ..TlsConfig::default() };
        let (nb, nc, other) = (
            NodeId::new("127.0.0.1", 9501).unwrap(),
            NodeId::new("127.0.0.1", 9502).unwrap(),
            NodeId::new("127.0.0.1", 9503).unwrap(),
        );
        let b_standing = Arc::new(KeyStanding::default());
        let b = super::imp::load_or_generate(&cfg, &nb, Arc::new(RemovedSet::default()), Arc::clone(&b_standing)).unwrap();
        let c = super::imp::load_or_generate(&cfg, &nc, Arc::new(RemovedSet::default()), Default::default()).unwrap();
        let c_key = c.verifying_key_bytes();
        // Counted, and no longer among the anchored keys — as the fold publishes a key revoked long ago
        // once it falls outside the newest-16 cap or the node is saturated (the re-review's finding).
        let revoked = |key: [u8; 32]| NodeStanding {
            anchored: Some([[4u8; 32]].into_iter().collect()),
            revoked: [key].into_iter().collect(),
            counted: [key].into_iter().collect(),
        };

        assert_eq!(handshake(c.client_config(), b.server_config()), Ok(()), "plant: C dials B before any revocation");
        assert_eq!(handshake(b.client_config(), c.server_config()), Ok(()), "plant: B dials C before any revocation");

        b_standing.set(&other, revoked(c_key));
        assert_eq!(handshake(c.client_config(), b.server_config()), Ok(()),
            "another node's revocation of C's key does not refuse C's certificate");

        // A provisional revocation — C's key revoked, but not (yet) anchored for C here — cuts nothing:
        // a node that rotated must still connect while its new certificate record travels.
        b_standing.set(&nc, NodeStanding { anchored: Some([[1u8; 32]].into_iter().collect()), revoked: [c_key].into_iter().collect(), counted: Default::default() });
        assert_eq!(handshake(c.client_config(), b.server_config()), Ok(()),
            "a provisional revocation of a key not anchored for C does not refuse C's certificate");

        b_standing.set(&nc, revoked(c_key));
        let accept = handshake(c.client_config(), b.server_config());
        let dial = handshake(b.client_config(), c.server_config());
        let _ = std::fs::remove_dir_all(&dir);
        assert!(accept.as_ref().is_err_and(|e| e.starts_with("server refused") && e.contains("revoked")),
            "REVOKED: B accepted a certificate for a key C revoked: {accept:?}");
        assert!(dial.as_ref().is_err_and(|e| e.starts_with("client refused") && e.contains("revoked")),
            "REVOKED: B dialled C and accepted a certificate for a key C revoked: {dial:?}");
    }

    /// **A removed member cannot reconnect on a resumed session** (found by the test above). The mesh's
    /// rustls configs kept their defaults: a client cache of sessions and a server that issued TLS 1.3
    /// tickets. A resumed session re-runs no certificate verifier, so a member that completed one
    /// handshake before its removal (closure plan C5) — or before its key's revocation — reconnected
    /// on the cached session and was admitted. Resumption is now off on both sides. Seen failing first
    /// with resumption on: the handshake after the removal completed.
    #[test]
    fn a_removed_member_cannot_reconnect_on_a_resumed_session() {
        use crate::node_id::NodeId;
        use crate::removal::{KeyStanding, RemovedSet};
        let dir = fresh_dir("removed-resume");
        let cfg = TlsConfig { auto_cert_dir: dir.clone(), ..TlsConfig::default() };
        let (nb, nc) = (NodeId::new("127.0.0.1", 9511).unwrap(), NodeId::new("127.0.0.1", 9512).unwrap());
        let b_removed = Arc::new(RemovedSet::default());
        let b = super::imp::load_or_generate(&cfg, &nb, Arc::clone(&b_removed), Arc::new(KeyStanding::default())).unwrap();
        let c = super::imp::load_or_generate(&cfg, &nc, Arc::new(RemovedSet::default()), Default::default()).unwrap();
        assert_eq!(handshake(c.client_config(), b.server_config()), Ok(()), "plant: C connects before its removal");
        b_removed.remove(&nc, [c.verifying_key_bytes()]);
        let after = handshake(c.client_config(), b.server_config());
        let _ = std::fs::remove_dir_all(&dir);
        assert!(after.as_ref().is_err_and(|e| e.starts_with("server refused") && e.contains("removed")),
            "RESUMED: a removed member reconnected on its cached session: {after:?}");
    }

    /// **A client certificate is refused at the mesh, both ways** (identity-and-lifecycle I1, D1). The fleet CA
    /// issues a service's key a certificate naming `mycelium-client://{issuer}/{name}`; the chain check admits it,
    /// so without the scheme refusal a service's key — which its agent's shell can reach — would be a connected
    /// peer. Refused by B's server when the client dials, and by B's dialling client when the client serves. The
    /// plants: a node certificate, and a pre-row-Q certificate naming no node, are still admitted both ways, and
    /// `verify_member_cert` (the chain alone, which the client-cert fold uses) still accepts the client
    /// certificate. Seen failing first: both handshakes completed.
    #[test]
    fn a_client_certificate_is_refused_at_the_mesh_both_ways() {
        use crate::node_id::NodeId;
        use crate::removal::RemovedSet;
        let dir = fresh_dir("client-cert-mesh");
        let cfg = TlsConfig { auto_cert_dir: dir.clone(), ..TlsConfig::default() };
        let (nb, nc, nl) = (
            NodeId::new("127.0.0.1", 9541).unwrap(),
            NodeId::new("127.0.0.1", 9542).unwrap(),
            NodeId::new("127.0.0.1", 9543).unwrap(),
        );
        let b = super::imp::load_or_generate(&cfg, &nb, Arc::new(RemovedSet::default()), Default::default()).unwrap();
        let c = super::imp::load_or_generate(&cfg, &nc, Arc::new(RemovedSet::default()), Default::default()).unwrap();
        let (ca_der, ca_key) = load_or_create_ca(&cfg).unwrap();

        let client_key = ed25519_dalek::SigningKey::from_bytes(&[41u8; 32]);
        let client_cert = super::imp::generate_client_cert("fleet", "forecaster", &client_key, &ca_key, 1_700_000_000_000).unwrap();
        let id = super::client_identity(client_cert.as_ref()).expect("a client certificate reads as one");
        assert_eq!((id.principal(), id.key), ("token:fleet/forecaster".to_string(), client_key.verifying_key().to_bytes()));
        assert!(b.verify_member_cert(client_cert.as_ref()), "the chain check alone still accepts a client certificate");
        let (client_server, client_client, _, _, _) =
            super::imp::build_rustls_configs(client_cert, &client_key, std::slice::from_ref(&ca_der), Arc::new(RemovedSet::default()), Default::default()).unwrap();

        let legacy_key = ed25519_dalek::SigningKey::from_bytes(&[42u8; 32]);
        let legacy = legacy_cert(&cfg, &nl, &legacy_key);
        let (_, legacy_client, _, _, _) =
            super::imp::build_rustls_configs(legacy, &legacy_key, &[ca_der], Arc::new(RemovedSet::default()), Default::default()).unwrap();
        assert_eq!(handshake(c.client_config(), b.server_config()), Ok(()), "plant: a node certificate dials B");
        assert_eq!(handshake(Arc::new(legacy_client), b.server_config()), Ok(()), "plant: a pre-row-Q certificate naming no node dials B");

        let dialled = handshake(Arc::new(client_client), b.server_config());
        // As a server, an issued client certificate already fails the name check (it carries no IP); a CA holder can
        // mint one that names an IP as well, which is the case the scheme refusal itself must catch.
        let with_ip = {
            let kp = rcgen::KeyPair::try_from(ed25519_dalek::pkcs8::EncodePrivateKey::to_pkcs8_der(&client_key).unwrap().as_bytes()).unwrap();
            let mut p = rcgen::CertificateParams::new(vec![]).unwrap();
            p.subject_alt_names = vec![
                rcgen::SanType::IpAddress("127.0.0.1".parse().unwrap()),
                rcgen::SanType::URI(rcgen::Ia5String::try_from(super::client_uri("fleet", "forecaster")).unwrap()),
            ];
            p.not_before = rcgen::date_time_ymd(2024, 1, 1);
            p.not_after = rcgen::date_time_ymd(2099, 1, 1);
            let mut ca_params = rcgen::CertificateParams::new(vec![]).unwrap();
            ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
            ca_params.not_before = rcgen::date_time_ymd(2024, 1, 1);
            ca_params.not_after = rcgen::date_time_ymd(2099, 1, 1);
            let ca = ca_params.self_signed(&ca_key).unwrap();
            rustls::pki_types::CertificateDer::from(p.signed_by(&kp, &ca, &ca_key).unwrap().der().to_vec())
        };
        let (with_ip_server, _, _, _, _) =
            super::imp::build_rustls_configs(with_ip, &client_key, &[load_or_create_ca(&cfg).unwrap().0], Arc::new(RemovedSet::default()), Default::default()).unwrap();
        let _ = client_server;
        let served = handshake(b.client_config(), Arc::new(with_ip_server));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(dialled.as_ref().is_err_and(|e| e.starts_with("server refused") && e.contains("client certificate")),
            "ADMITTED: a client certificate dialled the mesh: {dialled:?}");
        assert!(served.as_ref().is_err_and(|e| e.starts_with("client refused") && e.contains("client certificate")),
            "ADMITTED: B dialled a peer serving a client certificate: {served:?}");
    }

    /// A client certificate's names are read exactly: one `mycelium-client://{issuer}/{name}`, two valid segments,
    /// no `mycelium://` beside it. A node certificate is no client's; a segment with `/`, a space, or empty is
    /// refused at issue.
    #[test]
    fn a_client_identity_is_read_exactly_and_bad_segments_are_refused_at_issue() {
        use crate::node_id::NodeId;
        let dir = fresh_dir("client-cert-identity");
        let cfg = TlsConfig { auto_cert_dir: dir.clone(), ..TlsConfig::default() };
        let (_, ca_key) = load_or_create_ca(&cfg).unwrap();
        let k = ed25519_dalek::SigningKey::from_bytes(&[43u8; 32]);
        let node = super::imp::generate_node_cert(&NodeId::new("127.0.0.1", 9544).unwrap(), &k, &ca_key, 1).unwrap();
        assert_eq!(super::client_identity(node.as_ref()), None, "a node certificate is no client's");
        for (issuer, name) in [("fleet", ""), ("", "x"), ("fleet", "a/b"), ("fleet", "a b"), ("..", "x"), ("fleet", &"n".repeat(129))] {
            assert!(super::imp::generate_client_cert(issuer, name, &k, &ca_key, 1).is_err(), "{issuer:?}/{name:?} refused at issue");
        }
        let ok = super::imp::generate_client_cert("fleet.example", "svc_1-a", &k, &ca_key, 7).unwrap();
        let id = super::client_identity(ok.as_ref()).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!((id.issuer.as_str(), id.name.as_str(), id.issued_ms), ("fleet.example", "svc_1-a", Some(7)));
    }

    /// **A client named `legacy` is refused** (#741's review, item 7): `token:{issuer}/legacy` is the legacy
    /// `gateway_auth_token`'s principal, so a client certificate naming it would be read as that credential's holder.
    /// Refused at issue, and a CA-minted certificate naming it is no client identity. Seen failing first: both
    /// accepted.
    #[test]
    fn a_client_named_legacy_is_refused_at_issue_and_read_as_no_client() {
        let dir = fresh_dir("client-legacy");
        let cfg = TlsConfig { auto_cert_dir: dir.clone(), ..TlsConfig::default() };
        let (_, ca_key) = load_or_create_ca(&cfg).unwrap();
        let k = ed25519_dalek::SigningKey::from_bytes(&[44u8; 32]);
        let issued = super::imp::generate_client_cert("fleet", "legacy", &k, &ca_key, 1);
        let minted = {
            let kp = rcgen::KeyPair::try_from(ed25519_dalek::pkcs8::EncodePrivateKey::to_pkcs8_der(&k).unwrap().as_bytes()).unwrap();
            let mut p = rcgen::CertificateParams::new(vec![]).unwrap();
            p.subject_alt_names = vec![rcgen::SanType::URI(rcgen::Ia5String::try_from(super::client_uri("fleet", "legacy")).unwrap())];
            p.serial_number = Some(rcgen::SerialNumber::from(2u64));
            let mut ca_params = rcgen::CertificateParams::new(vec![]).unwrap();
            ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
            let ca = ca_params.self_signed(&ca_key).unwrap();
            p.signed_by(&kp, &ca, &ca_key).unwrap().der().to_vec()
        };
        let _ = std::fs::remove_dir_all(&dir);
        assert!(issued.is_err(), "ISSUED: a client certificate naming `legacy`");
        assert_eq!(super::client_identity(&minted), None, "READ: a certificate naming `legacy` as a client identity");
    }

    // ---- Identity-and-lifecycle I7, PR 1: the consent certificate ----

    /// A fresh P-256 key pair and its SPKI PEM, as `dev` would export it.
    fn p256_key() -> (rcgen::KeyPair, [u8; 65], String) {
        let kp = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let point: [u8; 65] = kp.public_key_raw().try_into().unwrap();
        let pem = super::p256_spki_pem(&point);
        (kp, point, pem)
    }

    /// A certificate a CA-key holder mints by hand: `uris` as its names, over `public`, with `extensions`.
    fn minted(ca_key: &rcgen::KeyPair, public: &impl rcgen::PublicKeyData, uris: &[String], extensions: Vec<rcgen::CustomExtension>, serial: u64) -> Vec<u8> {
        let mut p = rcgen::CertificateParams::new(vec![]).unwrap();
        p.subject_alt_names = uris.iter().map(|u| rcgen::SanType::URI(rcgen::Ia5String::try_from(u.clone()).unwrap())).collect();
        p.serial_number = Some(rcgen::SerialNumber::from(serial));
        p.not_before = rcgen::date_time_ymd(2024, 1, 1);
        p.not_after = rcgen::date_time_ymd(2099, 1, 1);
        p.custom_extensions = extensions;
        let mut ca_params = rcgen::CertificateParams::new(vec![]).unwrap();
        ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        ca_params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        ca_params.not_after = rcgen::date_time_ymd(2099, 1, 1);
        let ca = ca_params.self_signed(ca_key).unwrap();
        p.signed_by(public, &ca, ca_key).unwrap().der().to_vec()
    }

    fn grade_ext(value: Vec<u8>) -> rcgen::CustomExtension {
        rcgen::CustomExtension::from_oid_content(super::CONSENT_GRADE_OID, value)
    }

    /// **I7 exit 1, the issue flow over a public key alone.** On a host holding the CA key and no private consent key,
    /// `issue_consent_cert` issues over a P-256 SPKI PEM with its grade, writes the certificate and **no key file**, and
    /// the certificate chains to the fleet CA (`verify_member_cert`, the chain check the fold judges by), names
    /// `mycelium-consent://{issuer}/{name}` and reads back as `consent:{issuer}/{name}` with the enrolled point and grade
    /// — for each grade. Our SPKI PEM is rcgen's own for the key, so the fixed header is the standard encoding. Seen
    /// failing first with the issuer writing no grade extension (`consent_identity` read `None`: no grade, no consent).
    #[test]
    fn a_consent_certificate_is_issued_over_a_p256_spki_alone_and_reads_back_its_principal_and_grade() {
        use crate::node_id::NodeId;
        use crate::removal::RemovedSet;
        let dir = fresh_dir("consent-issue");
        let ca_dir = dir.join("ca");
        super::imp::mint_ca(&ca_dir).unwrap();
        let cfg = TlsConfig { auto_cert_dir: ca_dir.clone(), ..TlsConfig::default() };
        let node = super::imp::load_or_generate(&cfg, &NodeId::new("127.0.0.1", 9561).unwrap(), Arc::new(RemovedSet::default()), Default::default()).unwrap();
        for (i, grade) in super::ConsentGrade::ALL.into_iter().enumerate() {
            let (kp, point, pem) = p256_key();
            assert_eq!(pem.replace('\n', ""), kp.public_key_pem().replace(['\n', '\r'], ""), "our SPKI PEM is the standard encoding");
            let out = dir.join(format!("out-{i}"));
            let cert = super::issue_consent_cert(&ca_dir, "fleet", "richard", &pem, grade, &out).expect("issued over the public key alone");
            assert_eq!(cert, out.join("consent-fleet-richard.cert.pem"));
            let written: Vec<_> = std::fs::read_dir(&out).unwrap().flatten().map(|e| e.file_name().into_string().unwrap()).collect();
            assert_eq!(written, vec!["consent-fleet-richard.cert.pem".to_string()], "a certificate and no key file");
            let der = super::imp::pem_cert_to_der(&std::fs::read_to_string(&cert).unwrap()).unwrap();
            assert!(node.verify_member_cert(der.as_ref()), "the chain verifies: an Ed25519 CA over a P-256 leaf");
            let id = super::consent_identity(der.as_ref()).expect("a consent certificate reads as one");
            assert_eq!((id.principal(), id.point, id.grade), ("consent:fleet/richard".to_string(), point, grade));
            assert!(id.issued_ms.is_some(), "dated in its serial, as a client certificate is");
            assert_eq!(super::client_identity(der.as_ref()), None, "a consent certificate is no client's");
            assert_eq!(super::cert_identity(der.as_ref()), None, "nor a mesh identity's: it carries no Ed25519 key");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **I7 exit 1's goldens.** The grade extension's OID — **provisional**, the documentation PEN 32473 until the
    /// registered arc arrives — and each grade's DER, an `UTF8String` of the grade's exact text, read back from an issued
    /// certificate by the walk in `cert_identity`'s reader. The OID's encoding is checked against the bytes rcgen wrote.
    #[test]
    fn the_grade_extensions_oid_and_each_grades_der_are_pinned() {
        const { assert!(super::CONSENT_GRADE_OID_IS_PROVISIONAL, "a release is blocked until the registered PEN arc replaces this OID") };
        assert_eq!(super::CONSENT_GRADE_OID, &[1, 3, 6, 1, 4, 1, 32473, 7, 1]);
        assert_eq!(super::consent_grade_oid_der(), vec![0x2B, 0x06, 0x01, 0x04, 0x01, 0x81, 0xFD, 0x59, 0x07, 0x01]);
        let golden: [(super::ConsentGrade, &str, &str); 3] = [
            (super::ConsentGrade::OsPresenceMacosUnbound, "os_presence(macos, unbound)", "0c1b6f735f70726573656e6365286d61636f732c20756e626f756e6429"),
            (super::ConsentGrade::Tty, "tty", "0c03747479"),
            (super::ConsentGrade::Recording, "recording", "0c097265636f7264696e67"),
        ];
        let dir = fresh_dir("consent-golden");
        let cfg = TlsConfig { auto_cert_dir: dir.clone(), ..TlsConfig::default() };
        let (_, ca_key) = load_or_create_ca(&cfg).unwrap();
        for (grade, text, der_hex) in golden {
            assert_eq!(grade.as_str(), text);
            assert_eq!(super::ConsentGrade::parse(text), Some(grade));
            let der: String = grade.extension_der().iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(der, der_hex, "{text}'s DER");
            let (_, point, _) = p256_key();
            let cert = super::imp::generate_consent_cert("fleet", "richard", &point, grade, &ca_key, 5).unwrap();
            let walk = super::tbs_walk(cert.as_ref()).unwrap();
            assert_eq!(walk.grade_ext.as_deref(), Some(grade.extension_der().as_slice()), "{text} read back");
            let mut tlv = vec![0x06, 0x0A];
            tlv.extend(super::consent_grade_oid_der());
            assert!(cert.as_ref().windows(tlv.len()).any(|w| w == tlv.as_slice()), "rcgen wrote the pinned OID");
        }
        assert_eq!(super::ConsentGrade::parse("service"), None, "service is no consent grade");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A shipped build fails closed on the provisional OID** (#800's review, finding 1). With the gate closed — as in
    /// a build without `cfg(test)` or `provisional-consent-grade` — a consent certificate carrying the provisional OID
    /// reads as no consent identity, `consent_identity_closed_by_provisional_oid` names why, and the issuer refuses;
    /// with it open the same certificate reads. Seen failing first with `consent_identity` ungated (the certificate read
    /// as `consent:fleet/richard` with the gate closed).
    #[test]
    fn a_closed_provisional_gate_reads_no_consent_certificate_and_issues_none() {
        let dir = fresh_dir("consent-gate");
        let cfg = TlsConfig { auto_cert_dir: dir.clone(), ..TlsConfig::default() };
        let (_, ca_key) = load_or_create_ca(&cfg).unwrap();
        let (_, point, _) = p256_key();
        let cert = super::imp::generate_consent_cert("fleet", "richard", &point, super::ConsentGrade::Tty, &ca_key, 9).unwrap();
        assert!(super::consent_identity(cert.as_ref()).is_some(), "plant: the test build reads it");
        assert!(!super::consent_identity_closed_by_provisional_oid(cert.as_ref()));
        let (read, named, issued) = super::with_provisional_consent_grade_closed(|| (
            super::consent_identity(cert.as_ref()),
            super::consent_identity_closed_by_provisional_oid(cert.as_ref()),
            super::imp::generate_consent_cert("fleet", "richard", &point, super::ConsentGrade::Tty, &ca_key, 10),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(read, None, "READ: a provisional-OID consent certificate with the gate closed");
        assert!(named, "the closed gate is named as the reason");
        assert!(issued.is_err_and(|e| e.to_string().contains("provisional")), "ISSUED with the gate closed");
        assert!(super::consent_identity(cert.as_ref()).is_some(), "the gate reopens after the closure");
    }

    /// **The one P-256 verifier, and its gate** (I7, P2; #800's review finding 1, at the verifier). An ASN.1 DER ECDSA
    /// signature by a consent key verifies under its point over the message, and not over another message, under
    /// another point, for a truncated signature or a point off the curve. With the provisional gate closed — a
    /// shipped build — the same signature verifies under nothing. Seen failing first with the gate check removed
    /// (verified with the gate closed).
    #[test]
    fn the_p256_verifier_verifies_a_consent_signature_and_nothing_with_the_gate_closed() {
        let (kp, point, _) = p256_key();
        let (_, other, _) = p256_key();
        let sig = super::sign_consent_p256_in_test_build(&kp.serialize_der(), b"a decision").expect("signed");
        assert!(super::verify_consent_p256(&point, b"a decision", &sig));
        assert!(!super::verify_consent_p256(&point, b"another decision", &sig), "another message");
        assert!(!super::verify_consent_p256(&other, b"a decision", &sig), "another key");
        assert!(!super::verify_consent_p256(&point, b"a decision", &sig[..sig.len() - 1]), "a truncated signature");
        assert!(!super::verify_consent_p256(&[0u8; 65], b"a decision", &sig), "no point");
        let closed = super::with_provisional_consent_grade_closed(|| super::verify_consent_p256(&point, b"a decision", &sig));
        assert!(!closed, "VERIFIED: a consent signature with the provisional gate closed");
        assert!(super::verify_consent_p256(&point, b"a decision", &sig), "the gate reopens after the closure");
    }

    /// **A point not on P-256 is refused at issue and at read** (#800's review, finding 4). `04 ‖ 0^64` and a point
    /// with `y` off by one are refused by the SPKI reader (`not_p256: the point is not on the curve`), by the issuer,
    /// and — minted by a CA-key holder — read as no consent identity, so such a certificate never becomes a consent
    /// principal's newest mark. Seen failing first with only the `04` prefix checked (all three accepted).
    #[test]
    fn a_point_off_the_curve_is_refused_at_issue_and_at_read() {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let dir = fresh_dir("consent-curve");
        let cfg = TlsConfig { auto_cert_dir: dir.clone(), ..TlsConfig::default() };
        let (_, ca_key) = load_or_create_ca(&cfg).unwrap();
        let (_, good, _) = p256_key();
        let mut zero = [0u8; 65];
        zero[0] = 0x04;
        let mut nudged = good;
        nudged[64] ^= 0x01;
        for (what, point) in [("04 || 0^64", zero), ("y off by one", nudged)] {
            assert!(!super::p256_point_on_curve(&point), "{what}");
            let mut der = super::P256_SPKI_HEADER.to_vec();
            der.extend_from_slice(&point);
            let pem = format!("-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n", STANDARD.encode(der));
            let read = super::p256_point_from_spki_pem(&pem);
            assert!(read.as_ref().is_err_and(|e| e.contains("not on the curve")), "{what}: {read:?}");
            assert!(super::imp::generate_consent_cert("fleet", "x", &point, super::ConsentGrade::Tty, &ca_key, 1).is_err(), "ISSUED: {what}");
            let minted_cert = minted(&ca_key, &super::imp::P256Public(point), &[super::consent_uri("fleet", "x")],
                vec![grade_ext(super::ConsentGrade::Tty.extension_der())], 4);
            assert_eq!(super::consent_identity(&minted_cert), None, "READ: {what}");
        }
        assert!(super::p256_point_on_curve(&good), "plant: a real key's point is on the curve");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **No shipped binary writes or reads the provisional OID: the rule.** While `CONSENT_GRADE_OID_IS_PROVISIONAL`
    /// holds, only a test build may; once the registered arc replaces it, any build may. The wiring is
    /// `a_closed_provisional_gate_reads_no_consent_certificate_and_issues_none`; the shipped binary's refusal is a run
    /// against a `cargo build` binary (no `provisional-consent-grade`), recorded in the plan's §8. Seen failing first
    /// with the rule returning `true` whatever the build.
    #[test]
    fn a_provisional_grade_oid_is_written_only_by_a_test_build() {
        assert!(!super::consent_grade_oid_open_when(true, false), "WRITTEN: the provisional OID by a shipped build");
        assert!(super::consent_grade_oid_open_when(true, true));
        assert!(super::consent_grade_oid_open_when(false, false));
    }

    /// **I7 exit 1's refusals at the key.** The SPKI reader refuses, by name: an Ed25519 SPKI, a P-384 SPKI, a
    /// compressed point, raw X9.63 (hex or bytes, no PEM), and two blocks. `issue_consent_cert` refuses each the same
    /// way and writes nothing. Seen failing first with the reader taking the last 65 bytes of any SPKI and no header
    /// check: the P-384 SPKI was read as a point (the Ed25519 one was refused only for being shorter than 65 bytes).
    #[test]
    fn the_consent_issuer_refuses_a_key_that_is_not_a_p256_spki_by_name() {
        use base64::{engine::general_purpose::STANDARD, Engine};
        let pem_of = |der: &[u8]| format!("-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n", STANDARD.encode(der));
        let mut ed = vec![0x30, 0x2A, 0x30, 0x05, 0x06, 0x03, 0x2B, 0x65, 0x70, 0x03, 0x21, 0x00];
        ed.extend([7u8; 32]);
        let p384 = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P384_SHA384).unwrap().public_key_pem();
        let (_, point, good) = p256_key();
        let mut compressed = super::P256_SPKI_HEADER.to_vec();
        compressed.extend_from_slice(&point);
        compressed[1] = 0x39;
        compressed[24] = 0x22;
        compressed.truncate(super::P256_SPKI_HEADER.len());
        compressed.push(0x02);
        compressed.extend_from_slice(&point[1..33]);
        let raw_hex: String = point.iter().map(|b| format!("{b:02x}")).collect();
        let cases: [(&str, String, &str); 6] = [
            ("ed25519", pem_of(&ed), "not_p256"),
            ("p384", p384, "not_p256"),
            ("compressed", pem_of(&compressed), "not_p256"),
            ("raw x9.63 hex", raw_hex, "not_spki_pem"),
            ("raw x9.63 base64", STANDARD.encode(point), "not_spki_pem"),
            ("two blocks", format!("{good}{good}"), "not_spki_pem"),
        ];
        let dir = fresh_dir("consent-refuse");
        let ca_dir = dir.join("ca");
        super::imp::mint_ca(&ca_dir).unwrap();
        for (what, input, reason) in cases {
            let read = super::p256_point_from_spki_pem(&input);
            assert!(read.as_ref().is_err_and(|e| e.starts_with(reason)), "{what}: {read:?}");
            let out = dir.join("out");
            let issued = super::issue_consent_cert(&ca_dir, "fleet", "richard", &input, super::ConsentGrade::Tty, &out);
            assert!(issued.as_ref().is_err_and(|e| e.to_string().contains(reason)), "{what} issued: {issued:?}");
            assert!(!out.exists(), "{what}: nothing written");
        }
        assert_eq!(super::p256_point_from_spki_pem(&good).unwrap(), point);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **The scheme is bound to the key kind, and no grade is no consent** (I7, H1, H2). A CA-key holder can mint any
    /// of these; none reads as a consent identity, and none as a client identity where it names the consent scheme:
    /// the consent scheme over an Ed25519 key; the client scheme over a P-256 key; a certificate naming **both** schemes
    /// (over either key); the consent scheme beside a node's `mycelium://`; a consent certificate with no grade, with a
    /// grade outside the set (`service`, or a near miss), with its grade twice, or with a grade that is not an
    /// `UTF8String`. The plant: the same mint with one consent URI and one grade reads as a consent identity. Seen
    /// failing first with `client_identity` ignoring the consent scheme (the both-schemes Ed25519 certificate read as
    /// `token:fleet/richard`).
    #[test]
    fn the_consent_scheme_is_bound_to_a_p256_key_and_a_certificate_without_a_grade_counts_as_neither() {
        let dir = fresh_dir("consent-bound");
        let cfg = TlsConfig { auto_cert_dir: dir.clone(), ..TlsConfig::default() };
        let (_, ca_key) = load_or_create_ca(&cfg).unwrap();
        let ed = rcgen::KeyPair::try_from(
            ed25519_dalek::pkcs8::EncodePrivateKey::to_pkcs8_der(&ed25519_dalek::SigningKey::from_bytes(&[61u8; 32])).unwrap().as_bytes(),
        ).unwrap();
        let (p256, _, _) = p256_key();
        let (consent, client, node) = (super::consent_uri("fleet", "richard"), super::client_uri("fleet", "richard"), "mycelium://127.0.0.1:9562".to_string());
        let tty = || vec![grade_ext(super::ConsentGrade::Tty.extension_der())];
        let plant = minted(&ca_key, &p256, std::slice::from_ref(&consent), tty(), 3);
        assert_eq!(super::consent_identity(&plant).map(|i| (i.principal(), i.grade)), Some(("consent:fleet/richard".into(), super::ConsentGrade::Tty)), "plant");
        let none_consent: Vec<(&str, Vec<u8>)> = vec![
            ("consent scheme over ed25519", minted(&ca_key, &ed, std::slice::from_ref(&consent), tty(), 3)),
            ("client scheme over p256", minted(&ca_key, &p256, std::slice::from_ref(&client), tty(), 3)),
            ("both schemes over p256", minted(&ca_key, &p256, &[consent.clone(), client.clone()], tty(), 3)),
            ("both schemes over ed25519", minted(&ca_key, &ed, &[client.clone(), consent.clone()], tty(), 3)),
            ("consent beside a node", minted(&ca_key, &p256, &[consent.clone(), node.clone()], tty(), 3)),
            ("two consent names", minted(&ca_key, &p256, &[consent.clone(), super::consent_uri("fleet", "other")], tty(), 3)),
            ("no grade", minted(&ca_key, &p256, std::slice::from_ref(&consent), vec![], 3)),
            ("grade service", minted(&ca_key, &p256, std::slice::from_ref(&consent), vec![grade_ext(vec![0x0C, 7, b's', b'e', b'r', b'v', b'i', b'c', b'e'])], 3)),
            ("grade near miss", minted(&ca_key, &p256, std::slice::from_ref(&consent), vec![grade_ext(vec![0x0C, 4, b't', b't', b'y', b' '])], 3)),
            ("grade twice", minted(&ca_key, &p256, std::slice::from_ref(&consent), vec![grade_ext(super::ConsentGrade::Tty.extension_der()), grade_ext(super::ConsentGrade::Recording.extension_der())], 3)),
            ("grade not utf8string", minted(&ca_key, &p256, std::slice::from_ref(&consent), vec![grade_ext(vec![0x13, 3, b't', b't', b'y'])], 3)),
            ("grade with trailing bytes", minted(&ca_key, &p256, std::slice::from_ref(&consent), vec![grade_ext(vec![0x0C, 3, b't', b't', b'y', 0x00])], 3)),
        ];
        let _ = std::fs::remove_dir_all(&dir);
        for (what, der) in &none_consent {
            assert_eq!(super::consent_identity(der), None, "COUNTED AS CONSENT: {what}");
            assert_eq!(super::client_identity(der), None, "COUNTED AS A CLIENT: {what}");
        }
        assert!(super::imp::generate_consent_cert("fleet", "legacy", &p256.public_key_raw().try_into().unwrap(), super::ConsentGrade::Tty, &ca_key, 1).is_err(),
            "a reserved name is refused at issue");
    }

    /// **I7 exit 8, the mesh.** A P-256 consent certificate is refused by the mesh's key check with the reason the row
    /// names — `refuse_if_removed`: "a mesh certificate must carry an Ed25519 identity key" — and an **Ed25519**
    /// certificate a CA-key holder minted with the consent scheme (and an IP, so the name check passes) is refused by
    /// the widened scheme check, `refuse_if_client`, at a real handshake when it dials B. The plant: the same mint
    /// without the consent URI dials B. Seen failing first with `refuse_if_client` checking the client scheme only (the
    /// Ed25519 consent-scheme certificate joined).
    #[test]
    fn a_consent_certificate_is_refused_at_the_mesh_for_its_key_and_for_its_scheme() {
        use crate::node_id::NodeId;
        use crate::removal::RemovedSet;
        let dir = fresh_dir("consent-mesh");
        let cfg = TlsConfig { auto_cert_dir: dir.clone(), ..TlsConfig::default() };
        let nb = NodeId::new("127.0.0.1", 9563).unwrap();
        let b = super::imp::load_or_generate(&cfg, &nb, Arc::new(RemovedSet::default()), Default::default()).unwrap();
        let (ca_der, ca_key) = load_or_create_ca(&cfg).unwrap();
        let (_, point, _) = p256_key();
        let p256_cert = super::imp::generate_consent_cert("fleet", "richard", &point, super::ConsentGrade::Tty, &ca_key, 1_700_000_000_000).unwrap();
        let by_key = super::imp::refuse_if_removed(&RemovedSet::default(), &p256_cert);
        assert!(by_key.as_ref().is_err_and(|e| e.to_string().contains("a mesh certificate must carry an Ed25519 identity key")), "{by_key:?}");

        let k = ed25519_dalek::SigningKey::from_bytes(&[62u8; 32]);
        let kp = rcgen::KeyPair::try_from(ed25519_dalek::pkcs8::EncodePrivateKey::to_pkcs8_der(&k).unwrap().as_bytes()).unwrap();
        let mint = |uris: Vec<rcgen::SanType>| {
            let mut p = rcgen::CertificateParams::new(vec![]).unwrap();
            p.subject_alt_names = uris;
            p.not_before = rcgen::date_time_ymd(2024, 1, 1);
            p.not_after = rcgen::date_time_ymd(2099, 1, 1);
            let mut ca_params = rcgen::CertificateParams::new(vec![]).unwrap();
            ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
            ca_params.not_before = rcgen::date_time_ymd(2024, 1, 1);
            ca_params.not_after = rcgen::date_time_ymd(2099, 1, 1);
            let ca = ca_params.self_signed(&ca_key).unwrap();
            rustls::pki_types::CertificateDer::from(p.signed_by(&kp, &ca, &ca_key).unwrap().der().to_vec())
        };
        let ip = rcgen::SanType::IpAddress("127.0.0.1".parse().unwrap());
        let plant = mint(vec![ip.clone()]);
        let scheme = mint(vec![ip, rcgen::SanType::URI(rcgen::Ia5String::try_from(super::consent_uri("fleet", "richard")).unwrap())]);
        let dial = |cert| {
            let (_, client, _, _, _) = super::imp::build_rustls_configs(cert, &k, std::slice::from_ref(&ca_der), Arc::new(RemovedSet::default()), Default::default()).unwrap();
            handshake(Arc::new(client), b.server_config())
        };
        let planted = dial(plant);
        let dialled = dial(scheme);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(planted, Ok(()), "plant: the same mint without the consent scheme dials B");
        assert!(dialled.as_ref().is_err_and(|e| e.starts_with("server refused") && e.contains("consent certificate")),
            "ADMITTED: an Ed25519 certificate naming the consent scheme dialled the mesh: {dialled:?}");
    }
}

/// Identity-and-lifecycle I2, PR 1: the reload path over a root set, the roots generation, the distinct
/// successor subject, and the refusal of a multi-certificate `ca_cert_pem`.
#[cfg(all(test, feature = "tls"))]
mod succession_tests {
    use super::{NodeTls, TbsName};
    use crate::{config::TlsConfig, node_id::NodeId, removal::RemovedSet};
    use rustls::pki_types::{pem::PemObject, CertificateDer};
    use std::{path::{Path, PathBuf}, sync::Arc};

    fn fresh_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("myc-i2-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn der_of(p: &Path) -> Vec<u8> {
        CertificateDer::from_pem_file(p).unwrap().as_ref().to_vec()
    }

    /// A minted CA in `{base}/{name}`: its directory and its certificate's DER.
    fn ca(base: &Path, name: &str) -> (PathBuf, Vec<u8>) {
        let dir = base.join(name);
        let cert = super::mint_ca(&dir).unwrap();
        (dir, der_of(&cert))
    }

    /// A node on the secure posture: a certificate issued off-node under `ca_dir`, trusting `trust`.
    fn node(base: &Path, name: &str, port: u16, ca_dir: &Path, trust: &Path) -> (NodeId, NodeTls) {
        let id = NodeId::new("127.0.0.1", port).unwrap();
        let out = base.join(format!("{name}-issued"));
        let (cert, key) = super::issue_node_cert(ca_dir, &id, &out).unwrap();
        let cfg = TlsConfig {
            auto_cert_dir: base.join(format!("{name}-node")),
            cert_pem: Some(cert),
            key_pem: Some(key),
            ca_cert_pem: Some(trust.to_path_buf()),
        };
        let tls = super::load_or_generate(&cfg, &id, Arc::new(RemovedSet::default()), Default::default()).unwrap();
        assert!(!cfg.auto_cert_dir.join("ca-key.pem").exists(), "the secure posture: no CA key on the node");
        (id, tls)
    }

    fn handshake(client: Arc<rustls::ClientConfig>, server: Arc<rustls::ServerConfig>) -> Result<(), String> {
        let name = rustls::pki_types::ServerName::try_from("127.0.0.1").unwrap().to_owned();
        let mut c = rustls::ClientConnection::new(client, name).unwrap();
        let mut s = rustls::ServerConnection::new(server).unwrap();
        for _ in 0..16 {
            let mut buf = Vec::new();
            while c.wants_write() { c.write_tls(&mut buf).unwrap(); }
            if !buf.is_empty() {
                s.read_tls(&mut buf.as_slice()).unwrap();
                s.process_new_packets().map_err(|e| format!("server refused: {e}"))?;
            }
            let mut buf = Vec::new();
            while s.wants_write() { s.write_tls(&mut buf).unwrap(); }
            if !buf.is_empty() {
                c.read_tls(&mut buf.as_slice()).unwrap();
                c.process_new_packets().map_err(|e| format!("client refused: {e}"))?;
            }
            if !c.is_handshaking() && !s.is_handshaking() { return Ok(()); }
        }
        Err("did not finish".into())
    }

    /// The chain check alone over `roots`, as a verifier's error.
    fn chain_error(roots: &[&[u8]], cert: &[u8]) -> Option<rustls::Error> {
        let mut store = rustls::RootCertStore::empty();
        for r in roots { store.add(CertificateDer::from(r.to_vec())).unwrap(); }
        let v = rustls::server::WebPkiClientVerifier::builder_with_provider(Arc::new(store), Arc::new(rustls::crypto::ring::default_provider()))
            .build().unwrap();
        v.verify_client_cert(&CertificateDer::from(cert.to_vec()), &[], rustls::pki_types::UnixTime::now()).err()
    }

    /// **Exit 7: a distinct successor subject.** Two CAs minted by `tls ca` have distinct subjects, each naming
    /// its fleet and its generation; a node certificate's issuer name is its own CA's subject, byte for byte; and a
    /// refusal across the two CAs reads `UnknownIssuer`. Seen failing first on the code before I2: both CAs carried
    /// rcgen's default subject, and the cross-CA refusal read `BadSignature` (spike S-succeed, finding 3).
    #[test]
    fn two_minted_cas_have_distinct_subjects_and_a_cross_ca_refusal_reads_unknown_issuer() {
        let base = fresh_dir("subject");
        let (ca1, der1) = ca(&base, "ca1");
        let (_ca2, der2) = ca(&base, "ca2");
        let (s1, s2) = (super::tbs_name(&der1, TbsName::Subject).unwrap(), super::tbs_name(&der2, TbsName::Subject).unwrap());
        assert_ne!(s1, s2, "two minted CAs share no name");
        for d in [&der1, &der2] {
            let cn = super::ca_subject_cn(d).expect("the one subject shape tls ca writes");
            assert!(cn.starts_with("Mycelium fleet ") && cn.ends_with(" CA generation 0"), "{cn}");
        }
        let id = NodeId::new("127.0.0.1", 9601).unwrap();
        let (cert, _) = super::issue_node_cert(&ca1, &id, &base.join("issued")).unwrap();
        let cert = der_of(&cert);
        assert_eq!(super::tbs_name(&cert, TbsName::Issuer), Some(s1), "the issuer name is CA1's subject");
        assert!(chain_error(&[&der1], &cert).is_none(), "and it chains to CA1");
        let refused = chain_error(&[&der2], &cert).expect("CA2 does not verify CA1's certificate");
        assert!(
            matches!(refused, rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer)),
            "across two CAs the refusal names the issuer: {refused:?}"
        );

        // Mixed versions, stated in deprecations.md §111: an issuer before I2 rebuilt the CA with rcgen's default
        // subject. Under a CA minted by this release its certificate names the wrong issuer and chains to no root.
        let ca1_key = rcgen::KeyPair::from_pem(&std::fs::read_to_string(ca1.join("ca-key.pem")).unwrap()).unwrap();
        let k = ed25519_dalek::SigningKey::from_bytes(&[4u8; 32]);
        let old_issuer = super::imp::generate_node_cert(&id, &k, &ca1_key, 1).unwrap();
        assert!(matches!(chain_error(&[&der1], old_issuer.as_ref()),
            Some(rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer))),
            "an older issuer's certificate under a minted CA chains to no root");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// **A CA already deployed keeps its name.** A node's own CA, and every CA before I2, has rcgen's default
    /// subject; a certificate issued under it still names that subject as its issuer and verifies. A CA whose
    /// subject this issuer cannot reproduce is refused at issue, never signed under another name.
    #[test]
    fn a_default_subject_ca_keeps_its_name_and_an_unreproducible_subject_is_refused() {
        let (ca_der, ca_key) = super::imp::generate_ca_for_test();
        assert_eq!(super::ca_subject_cn(ca_der.as_ref()).as_deref(), Some("rcgen self signed cert"));
        let k = ed25519_dalek::SigningKey::from_bytes(&[3u8; 32]);
        let id = NodeId::new("127.0.0.1", 9602).unwrap();
        let cert = super::imp::node_cert_under(&id, &k.verifying_key().to_bytes(), ca_der.as_ref(), &ca_key, 1).unwrap();
        assert_eq!(super::tbs_name(cert.as_ref(), TbsName::Issuer), super::tbs_name(ca_der.as_ref(), TbsName::Subject));
        assert!(chain_error(&[ca_der.as_ref()], cert.as_ref()).is_none());

        let mut params = rcgen::CertificateParams::new(vec![]).unwrap();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        params.distinguished_name.push(rcgen::DnType::OrganizationName, "two attributes");
        let odd = params.self_signed(&ca_key).unwrap();
        assert_eq!(super::ca_subject_cn(odd.der()), None);
        let said = super::imp::node_cert_under(&id, &k.verifying_key().to_bytes(), odd.der(), &ca_key, 1).unwrap_err().to_string();
        assert!(said.contains("subject is not one this issuer writes"), "{said}");
    }

    /// **Exit 14: no silent root drop.** A `ca_cert_pem` holding two certificates is refused at start, naming the
    /// field, the path and the upgrade note; one certificate starts; a chain in `cert_pem` (the node's certificate
    /// and its CA's) is unaffected; and the on-node CA's `ca-cert.pem` is read as before. Seen failing first: the
    /// two-certificate file started, trusting only its first certificate.
    #[test]
    fn a_multi_certificate_ca_cert_pem_is_refused_by_name_and_a_cert_pem_chain_is_not() {
        let base = fresh_dir("multi");
        let (ca1, der1) = ca(&base, "ca1");
        let (_ca2, _) = ca(&base, "ca2");
        let both = base.join("both.pem");
        let pem = |p: &Path| std::fs::read_to_string(p).unwrap();
        std::fs::write(&both, pem(&ca1.join("ca-cert.pem")) + &pem(&base.join("ca2/ca-cert.pem"))).unwrap();
        let id = NodeId::new("127.0.0.1", 9603).unwrap();
        let (cert, key) = super::issue_node_cert(&ca1, &id, &base.join("issued")).unwrap();
        let cfg = |trust: &Path, cert: &Path| TlsConfig {
            auto_cert_dir: base.join("node"),
            cert_pem: Some(cert.to_path_buf()),
            key_pem: Some(key.clone()),
            ca_cert_pem: Some(trust.to_path_buf()),
        };
        let refused = super::load_or_generate(&cfg(&both, &cert), &id, Arc::new(RemovedSet::default()), Default::default())
            .err().expect("two roots in ca_cert_pem are refused").to_string();
        assert!(refused.contains("tls.ca_cert_pem") && refused.contains("holds 2 certificates") && refused.contains("both.pem")
            && refused.contains("deprecations.md §111"), "{refused}");

        // The same file as `{auto_cert_dir}/ca-cert.pem` beside a pre-issued `cert_pem`, `ca_cert_pem` unset — the
        // operator's root on that posture (`tls issue` says to copy it there; #796's review, finding 1). Seen failing
        // first with the refusal keyed on `ca_cert_pem` alone: the node started trusting the first certificate.
        let posture = base.join("posture");
        std::fs::create_dir_all(&posture).unwrap();
        std::fs::copy(&both, posture.join("ca-cert.pem")).unwrap();
        let mut beside = TlsConfig::default();
        beside.auto_cert_dir = posture.clone();
        beside.cert_pem = Some(cert.clone());
        beside.key_pem = Some(key.clone());
        let refused = super::load_or_generate(&beside, &id, Arc::new(RemovedSet::default()), Default::default())
            .err().expect("two roots beside a pre-issued cert_pem are refused").to_string();
        assert!(refused.contains("holds 2 certificates") && refused.contains("posture"), "{refused}");

        let chain = base.join("chain.pem");
        std::fs::write(&chain, pem(&cert) + &pem(&ca1.join("ca-cert.pem"))).unwrap();
        let tls = super::load_or_generate(&cfg(&ca1.join("ca-cert.pem"), &chain), &id, Arc::new(RemovedSet::default()), Default::default())
            .expect("a chain in cert_pem starts");
        assert_eq!(tls.roots_der(), vec![der1.clone()]);
        assert_eq!(tls.node_cert_der(), der_of(&cert), "the chain's first certificate is the node's");

        // The on-node CA: `ca_cert_pem` unset, `ca-cert.pem` in `auto_cert_dir` — read as before.
        let own = base.join("own");
        let own_cfg = TlsConfig { auto_cert_dir: own.clone(), ..TlsConfig::default() };
        let _ = super::load_or_generate(&own_cfg, &id, Arc::new(RemovedSet::default()), Default::default()).unwrap();
        let first = pem(&own.join("ca-cert.pem"));
        std::fs::write(own.join("ca-cert.pem"), first + &pem(&base.join("ca2/ca-cert.pem"))).unwrap();
        assert!(super::load_or_generate(&own_cfg, &id, Arc::new(RemovedSet::default()), Default::default()).is_ok(),
            "the on-node CA's ca-cert.pem is not ca_cert_pem, and is unaffected");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// **The reload path, live, no restart (exits 1 and 11 at the TLS layer).** A node on `cert_pem` with no CA
    /// key widens to two roots, cuts over to its current key re-issued under the successor, and narrows to the
    /// successor alone, through `activate` only. At each phase: the chain check behind `verify_member_cert`, the
    /// mesh configs (a handshake each way with a peer), the fleet server check **handed out before the first
    /// change** (the bulk fetch's, built once at start), and the gateway's one config (it presents the successor
    /// certificate). After narrowing, a peer that trusts both roots but holds an old-root certificate is refused by
    /// the narrowed node's accept side and by its dial side (spike M2). Each change of roots bumps the roots
    /// generation once; a same-CA rotation does not.
    #[test]
    fn a_cert_pem_node_widens_cuts_over_and_narrows_live_through_activate() {
        let base = fresh_dir("live");
        let (ca1, der1) = ca(&base, "ca1");
        let (ca2, der2) = ca(&base, "ca2");
        let r1 = ca1.join("ca-cert.pem");
        let (ia, a) = node(&base, "a", 9611, &ca1, &r1);
        let (ib, b) = node(&base, "b", 9612, &ca1, &r1);
        let (_if, f) = node(&base, "f", 9613, &ca1, &r1);
        let fleet_at_start = a.fleet_server_verifier();
        let a2 = super::issue_node_cert_for_key(&ca2, &ia, a.verifying_key_bytes(), 0).unwrap();
        let b2 = super::issue_node_cert_for_key(&ca2, &ib, b.verifying_key_bytes(), 0).unwrap();
        assert!(!a.verify_member_cert(&b2), "before: A does not know CA2");
        assert_eq!(a.roots_generation(), 0);

        // Widen: every node trusts both roots, keeping its CA1 certificate.
        for t in [&a, &b, &f] {
            t.activate(t.reroot_material(t.node_cert_der(), vec![der1.clone(), der2.clone()]).unwrap()).unwrap();
            assert_eq!(t.roots_generation(), 1);
        }
        assert!(a.verify_member_cert(&b2), "widened: A's record check accepts a successor certificate");
        // Cut over: B presents its successor certificate.
        b.activate(b.reroot_material(b2.clone(), vec![der1.clone(), der2.clone()]).unwrap()).unwrap();
        assert_eq!(b.node_cert_der(), b2);
        assert_eq!(handshake(b.client_config(), a.server_config()), Ok(()), "B (CA2) → A (two roots)");
        assert_eq!(handshake(a.client_config(), b.server_config()), Ok(()), "A (CA1) → B (CA2)");
        // Narrow: A presents its successor certificate and trusts CA2 alone.
        a.activate(a.reroot_material(a2.clone(), vec![der2.clone()]).unwrap()).unwrap();
        b.activate(b.reroot_material(b2.clone(), vec![der2.clone()]).unwrap()).unwrap();
        assert_eq!(a.roots_generation(), 2, "A widened and narrowed: two changes of roots");
        assert_eq!(handshake(b.client_config(), a.server_config()), Ok(()), "B → A, both narrowed");
        assert_eq!(handshake(a.client_config(), b.server_config()), Ok(()), "A → B, both narrowed");
        assert!(!a.verify_member_cert(&f.node_cert_der()), "narrowed: an old-root record no longer verifies");
        let f_to_a = handshake(f.client_config(), a.server_config());
        assert!(f_to_a.as_ref().is_err_and(|e| e.starts_with("server refused")), "A's accept side refuses F's CA1 cert: {f_to_a:?}");
        let a_to_f = handshake(a.client_config(), f.server_config());
        assert!(a_to_f.as_ref().is_err_and(|e| e.starts_with("client refused")), "A's dial side refuses F's CA1 cert: {a_to_f:?}");

        // Exit 11: the fleet server check handed out at start now judges by CA2 alone.
        let name = rustls::pki_types::ServerName::try_from("127.0.0.1").unwrap();
        let now = rustls::pki_types::UnixTime::now();
        assert!(fleet_at_start.verify_server_cert(&CertificateDer::from(b2.clone()), &[], &name, &[], now).is_ok(),
            "the bulk fetch's verifier, held since start, accepts a successor server certificate");
        assert!(fleet_at_start.verify_server_cert(&CertificateDer::from(f.node_cert_der()), &[], &name, &[], now).is_err(),
            "and refuses an old-root one after narrowing");
        // Exit 11: the gateway's one config presents A's successor certificate, to a client trusting CA2 only.
        let mut roots = rustls::RootCertStore::empty();
        roots.add(CertificateDer::from(der2.clone())).unwrap();
        let ca2_only = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions().unwrap()
            .with_root_certificates(roots).with_no_client_auth();
        assert_eq!(handshake(Arc::new(ca2_only), a.gateway_server_config()), Ok(()), "the gateway presents A's CA2 certificate");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A same-CA rotation carries no roots: the chain checks stay and the roots generation does not move.
    #[test]
    fn a_same_ca_rotation_leaves_the_roots_and_their_generation() {
        let base = fresh_dir("rotate");
        let cfg = TlsConfig { auto_cert_dir: base.clone(), ..TlsConfig::default() };
        let id = NodeId::new("127.0.0.1", 9614).unwrap();
        let tls = super::load_or_generate(&cfg, &id, Arc::new(RemovedSet::default()), Default::default()).unwrap();
        let before = tls.roots_der();
        tls.activate(super::generate_rotation(&cfg, &id, Arc::new(RemovedSet::default()), Default::default()).unwrap()).unwrap();
        assert_eq!((tls.roots_generation(), tls.roots_der()), (0, before));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// `reroot_material` refuses, by name: no roots, a certificate for another key, and a certificate the new
    /// roots do not verify.
    #[test]
    fn a_change_of_roots_is_refused_without_roots_for_another_key_or_unverified() {
        let base = fresh_dir("refuse");
        let (ca1, der1) = ca(&base, "ca1");
        let (ca2, der2) = ca(&base, "ca2");
        let (ia, a) = node(&base, "a", 9615, &ca1, &ca1.join("ca-cert.pem"));
        let said = |r: Result<super::RotationMaterial, crate::error::GossipError>| r.err().expect("refused").to_string();
        assert!(said(a.reroot_material(a.node_cert_der(), vec![])).contains("at least one CA certificate"));
        let other = super::issue_node_cert_for_key(&ca2, &ia, [9u8; 32], 0);
        // [9; 32] is not a valid point for every check, but the certificate is only parsed for its key.
        if let Ok(other) = other {
            assert!(said(a.reroot_material(other, vec![der2.clone()])).contains("not for this node's current key"));
        }
        assert!(said(a.reroot_material(a.node_cert_der(), vec![der2.clone()])).contains("does not verify against the new roots"));
        assert_eq!(a.roots_generation(), 0, "nothing was activated");
        assert!(a.reroot_material(a.node_cert_der(), vec![der1, der2]).is_ok());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// **Exit 18's ordering (#768's M3): `activate` swaps the verifier before it bumps the roots generation.** A
    /// reader that loads the generation and then the verifier, between the two steps, tags what it judged with
    /// the **old** generation — so a verdict tagged with the new generation was always reached by the new
    /// verifier. Seen failing first with the bump moved before the swap: the plant read the new generation and the
    /// old verifier, and tagged an old-roots refusal as new.
    #[test]
    fn a_verdict_tagged_with_the_new_generation_was_reached_by_the_new_verifier() {
        let base = fresh_dir("order");
        let (ca1, der1) = ca(&base, "ca1");
        let (ca2, der2) = ca(&base, "ca2");
        let (ib, _) = node(&base, "b", 9617, &ca1, &ca1.join("ca-cert.pem"));
        let (_, a) = node(&base, "a", 9616, &ca1, &ca1.join("ca-cert.pem"));
        let successor = super::issue_node_cert_for_key(&ca2, &ib, ed25519_dalek::SigningKey::from_bytes(&[5u8; 32]).verifying_key().to_bytes(), 0).unwrap();
        let seen = std::rc::Rc::new(std::cell::Cell::new(None));
        let s = std::rc::Rc::clone(&seen);
        let cert = successor.clone();
        super::ACTIVATE_PLANT.with(|p| *p.borrow_mut() = Some(Box::new(move |t: &NodeTls| s.set(Some(t.verify_member_cert_tagged(&cert))))));
        a.activate(a.reroot_material(a.node_cert_der(), vec![der1, der2]).unwrap()).unwrap();
        let (generation, verified) = seen.get().expect("the plant ran");
        assert!(generation == 0 || verified, "a verdict tagged with the new generation ({generation}) came from the old verifier");
        assert_eq!(a.verify_member_cert_tagged(&successor), (1, true));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// **The reader's order (#796's review, finding 3): the generation is loaded before the chain check.** A
    /// change of roots between the reader's two loads (a plant there) leaves the verdict tagged with the old
    /// generation, so it is re-judged. Seen failing first with the two loads swapped: the reader loaded the old
    /// check, then the new generation, and tagged an old-roots refusal as current.
    #[test]
    fn a_change_of_roots_between_the_readers_two_loads_tags_the_old_generation() {
        let base = fresh_dir("reader");
        let (ca1, der1) = ca(&base, "ca1");
        let (ca2, der2) = ca(&base, "ca2");
        let (_, a) = node(&base, "a", 9618, &ca1, &ca1.join("ca-cert.pem"));
        let id = NodeId::new("127.0.0.1", 9619).unwrap();
        let successor = super::issue_node_cert_for_key(&ca2, &id, ed25519_dalek::SigningKey::from_bytes(&[6u8; 32]).verifying_key().to_bytes(), 0).unwrap();
        let wide = a.reroot_material(a.node_cert_der(), vec![der1, der2]).unwrap();
        super::READER_PLANT.with(|p| *p.borrow_mut() = Some(Box::new(move |t: &NodeTls| t.activate(wide).unwrap())));
        let (generation, verified) = a.verify_member_cert_tagged(&successor);
        assert_eq!(a.roots_generation(), 1, "the plant widened the roots");
        assert!(generation == 0 || verified, "tagged {generation} (current) with an old-roots refusal");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// **A same-CA rotation after a change of roots keeps one trust (#796's review, finding 2).** A node with its
    /// CA key on-node widens to its own CA and a successor, then rotates (`rotation_after`, what `rotate_identity`
    /// calls). A peer holding a successor certificate is accepted at the handshake **and** by `verify_member_cert`,
    /// as before the rotation. Seen failing first with the rotation built over the configuration's root: the
    /// handshake refused the peer while `verify_member_cert` accepted it.
    #[test]
    fn a_rotation_after_a_change_of_roots_builds_over_the_current_roots() {
        let base = fresh_dir("rotate-roots");
        let cfg = { let mut c = TlsConfig::default(); c.auto_cert_dir = base.join("own"); c };
        let id = NodeId::new("127.0.0.1", 9620).unwrap();
        let a = super::load_or_generate(&cfg, &id, Arc::new(RemovedSet::default()), Default::default()).unwrap();
        let own = a.roots_der().remove(0);
        let (ca2, der2) = ca(&base, "ca2");
        let (_, peer) = node(&base, "peer", 9621, &ca2, &ca2.join("ca-cert.pem"));
        let wide = vec![own.clone(), der2.clone()];
        peer.activate(peer.reroot_material(peer.node_cert_der(), wide.clone()).unwrap()).unwrap();
        a.activate(a.reroot_material(a.node_cert_der(), wide).unwrap()).unwrap();
        assert_eq!(handshake(peer.client_config(), a.server_config()), Ok(()), "widened: the CA2 peer → A");
        a.activate(a.rotation_after(&cfg, &id, 0).unwrap()).unwrap();
        assert!(a.verify_member_cert(&peer.node_cert_der()), "the record check still accepts the CA2 peer");
        assert_eq!(handshake(peer.client_config(), a.server_config()), Ok(()), "and so does the handshake, after rotating");
        assert_eq!(handshake(a.client_config(), peer.server_config()), Ok(()), "both ways");
        assert_eq!(a.roots_generation(), 1, "a same-CA rotation does not move the roots");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// **Stale rotation material is refused (#796's review, finding 2).** Material built over the configuration's
    /// root (the free `generate_rotation_after`), or over a root set that changed before it was activated, would
    /// split the node's trust; `activate` refuses it, naming the generations, and leaves the node as it was. Seen
    /// failing first with the check removed: the stale material was activated.
    #[test]
    fn rotation_material_built_over_other_roots_is_refused() {
        let base = fresh_dir("rotate-stale");
        let cfg = { let mut c = TlsConfig::default(); c.auto_cert_dir = base.join("own"); c };
        let id = NodeId::new("127.0.0.1", 9622).unwrap();
        let a = super::load_or_generate(&cfg, &id, Arc::new(RemovedSet::default()), Default::default()).unwrap();
        let own = a.roots_der().remove(0);
        let (_ca2, der2) = ca(&base, "ca2");
        let from_file = super::generate_rotation(&cfg, &id, Arc::new(RemovedSet::default()), Default::default()).unwrap();
        let before = a.rotation_after(&cfg, &id, 0).unwrap();
        a.activate(a.reroot_material(a.node_cert_der(), vec![own, der2]).unwrap()).unwrap();
        let key = a.verifying_key_bytes();
        for stale in [from_file, before] {
            let said = a.activate(stale).expect_err("stale material is refused").to_string();
            assert!(said.contains("cannot be activated at generation 1"), "{said}");
        }
        assert_eq!(a.verifying_key_bytes(), key, "nothing was activated");
        let _ = std::fs::remove_dir_all(&base);
    }
}

/// Identity-and-lifecycle I2, PR 2a: the CA generation (`caGeneration`, R1) — its provisional OID and encoding pinned,
/// its reader, the gate a shipped build closes, and legacy closed from the roots alone.
#[cfg(all(test, feature = "tls"))]
mod ca_generation_tests {
    use super::{CaGenerationError, NodeTls};
    use crate::{config::TlsConfig, node_id::NodeId, removal::{KeyStanding, RemovedSet}};
    use rustls::pki_types::{pem::PemObject, CertificateDer};
    use std::{path::{Path, PathBuf}, sync::Arc};

    fn fresh_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("myc-i2-gen-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn der_of(p: &Path) -> Vec<u8> {
        CertificateDer::from_pem_file(p).unwrap().as_ref().to_vec()
    }

    /// A self-signed CA carrying `extensions` (OID arcs, extnValue contents) — malformed values a hand-rolled issuer
    /// could write.
    fn ca_with(extensions: Vec<(&[u64], Vec<u8>)>) -> Vec<u8> {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
        let mut p = rcgen::CertificateParams::new(vec![]).unwrap();
        p.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        p.custom_extensions = extensions.into_iter().map(|(oid, v)| rcgen::CustomExtension::from_oid_content(oid, v)).collect();
        p.self_signed(&key).unwrap().der().to_vec()
    }

    /// **The OID and the encoding, pinned** (decision (e): a golden, as I7 pins its grade). Provisional: the release is
    /// blocked until the registered PEN arc replaces it, and this test changes with it.
    #[test]
    fn the_provisional_ca_generation_oid_and_its_encoding_are_pinned() {
        const { assert!(super::CA_GENERATION_OID_IS_PROVISIONAL, "a release is blocked until the registered PEN arc replaces this OID") };
        assert_eq!(super::CA_GENERATION_OID, &[1, 3, 6, 1, 4, 1, 32473, 2, 1]);
        assert_ne!(super::CA_GENERATION_OID, super::CONSENT_GRADE_OID, "its own sub-arc, not I7's");
        assert_eq!(super::ca_generation_oid_der(), vec![0x2B, 0x06, 0x01, 0x04, 0x01, 0x81, 0xFD, 0x59, 0x02, 0x01]);
        assert_eq!(super::ca_generation_extension_der(0), vec![0x02, 0x01, 0x00]);
        assert_eq!(super::ca_generation_extension_der(1), vec![0x02, 0x01, 0x01]);
        assert_eq!(super::ca_generation_extension_der(127), vec![0x02, 0x01, 0x7F]);
        assert_eq!(super::ca_generation_extension_der(128), vec![0x02, 0x02, 0x00, 0x80]);
        assert_eq!(super::ca_generation_extension_der(u64::MAX), vec![0x02, 0x09, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
    }

    /// **R1: a generation is read, and a malformed one is never 0.** A CA without the extension is generation 0, one
    /// minted at n reads n, and a value that is not one minimal non-negative `INTEGER` fitting a `u64`, or the
    /// extension twice, is `Malformed` — and is refused at load and at a change of roots, never read as 0. Seen failing
    /// first with the reader taking a malformed value as 0: the negative and twice cases read `Ok(0)`.
    #[test]
    fn a_ca_generation_is_read_and_a_malformed_one_is_never_zero() {
        let (g0, _) = super::imp::generate_ca_for_test();
        assert_eq!(super::ca_generation(g0.as_ref()), Ok(0), "no extension: generation 0, every CA before I2");
        let (g3, _) = super::imp::generate_ca_at_generation_for_test(7, 3);
        assert_eq!(super::ca_generation(g3.as_ref()), Ok(3));
        let oid = super::CA_GENERATION_OID;
        for (what, value) in [
            ("negative", vec![0x02, 0x01, 0xFF]),
            ("not minimal", vec![0x02, 0x02, 0x00, 0x01]),
            ("not an INTEGER", vec![0x04, 0x01, 0x01]),
            ("trailing bytes", vec![0x02, 0x01, 0x01, 0x00]),
            ("too long for a u64", vec![0x02, 0x0A, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            ("empty", vec![0x02, 0x00]),
        ] {
            assert_eq!(super::ca_generation(&ca_with(vec![(oid, value)])), Err(CaGenerationError::Malformed), "{what}");
        }
        let twice = ca_with(vec![(oid, super::ca_generation_extension_der(1)), (oid, super::ca_generation_extension_der(2))]);
        assert_eq!(super::ca_generation(&twice), Err(CaGenerationError::Malformed), "twice");
        // Refused at load, naming why.
        let base = fresh_dir("malformed");
        let path = base.join("ca-cert.pem");
        std::fs::write(&path, super::imp::cert_der_to_pem(&twice)).unwrap();
        let refused = super::imp::ca_cert_to_der(&std::fs::read_to_string(&path).unwrap(), &path, true).unwrap_err().to_string();
        assert!(refused.contains("caGeneration") && refused.contains("malformed"), "{refused}");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// **The shipped build's refusal, by name** (decision (e), I7's #800 precedent). With the gate closed, as in a build
    /// without `cfg(test)` or `provisional-ca-generation`: a CA under the provisional OID reads `Provisional` — never 0
    /// — and is refused at load naming the OID; no generation above 0 is minted; a generation-0 CA, which carries no
    /// extension, is read and minted as before. Seen failing first with the reader ignoring the gate: the successor read
    /// as generation 1 in a closed build.
    #[test]
    fn a_closed_provisional_gate_reads_no_ca_generation_and_mints_none() {
        let (g1, _) = super::imp::generate_ca_at_generation_for_test(7, 1);
        let base = fresh_dir("closed");
        let path = base.join("ca-cert.pem");
        std::fs::write(&path, super::imp::cert_der_to_pem(g1.as_ref())).unwrap();
        super::with_provisional_ca_generation_closed(|| {
            assert!(!super::ca_generation_oid_open());
            assert_eq!(super::ca_generation(g1.as_ref()), Err(CaGenerationError::Provisional));
            let refused = super::imp::ca_cert_to_der(&std::fs::read_to_string(&path).unwrap(), &path, true).unwrap_err().to_string();
            assert!(refused.contains("1.3.6.1.4.1.32473.2.1") && refused.contains("provisional"), "{refused}");
            let minted = super::mint_ca_at(&base.join("succ"), Some(7), 1).unwrap_err().to_string();
            assert!(minted.contains("not minted by this build"), "{minted}");
            assert!(!base.join("succ").join("ca-cert.pem").exists(), "nothing written");
            let (g0, _) = super::imp::generate_ca_for_test();
            assert_eq!(super::ca_generation(g0.as_ref()), Ok(0));
            super::mint_ca_at(&base.join("zero"), None, 0).expect("generation 0 writes no extension");
        });
        assert_eq!(super::ca_generation(g1.as_ref()), Ok(1), "the gate reopens");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// **R1 (exit 15's flag): legacy closes from the roots alone**, whenever the root set is built — at start and at
    /// each change of roots. A node started on generation 0 is open, widening a first succession (0 and 1) stays open,
    /// narrowing to 1 closes, and a node **started** on a generation-1 root — the operator's recovery, or a node
    /// installed after the succession, neither of which has `succession/` (#768's H2) — is closed from its first
    /// moment. A later succession's widened node (1 and 2) stays closed. Seen failing first with the flag set only at
    /// `activate`, never at start: the node started on the successor's root answered as legacy-open.
    #[test]
    fn legacy_closes_only_when_every_root_is_generation_one_or_later() {
        let (g0, _) = super::imp::generate_ca_for_test();
        let (g1, _) = super::imp::generate_ca_at_generation_for_test(7, 1);
        let (g2, _) = super::imp::generate_ca_at_generation_for_test(7, 2);
        let (g0, g1, g2) = (g0.as_ref().to_vec(), g1.as_ref().to_vec(), g2.as_ref().to_vec());
        assert!(!super::roots_close_legacy(std::slice::from_ref(&g0)));
        assert!(!super::roots_close_legacy(&[g0.clone(), g1.clone()]), "a first succession's widened node is open");
        assert!(super::roots_close_legacy(std::slice::from_ref(&g1)));
        assert!(super::roots_close_legacy(&[g1.clone(), g2.clone()]), "a later succession's widened node stays closed");
        assert!(!super::roots_close_legacy(&[]));

        let base = fresh_dir("closed-at-start");
        let ca0 = base.join("ca0");
        super::mint_ca(&ca0).unwrap();
        let ca1 = base.join("ca1");
        let ca1_cert = super::mint_ca_at(&ca1, Some(9), 1).unwrap();
        let start = |name: &str, ca_dir: &Path, trust: &Path, port: u16| -> (NodeTls, Arc<KeyStanding>) {
            let id = NodeId::new("127.0.0.1", port).unwrap();
            let (cert, key) = super::issue_node_cert(ca_dir, &id, &base.join(format!("{name}-issued"))).unwrap();
            let cfg = TlsConfig { auto_cert_dir: base.join(format!("{name}-node")), cert_pem: Some(cert), key_pem: Some(key), ca_cert_pem: Some(trust.to_path_buf()) };
            let standing = Arc::new(KeyStanding::default());
            (super::load_or_generate(&cfg, &id, Arc::new(RemovedSet::default()), Arc::clone(&standing)).unwrap(), standing)
        };
        let (a, a_standing) = start("a", &ca0, &ca0.join("ca-cert.pem"), 9711);
        assert!(!a_standing.legacy_closed(), "generation 0: open, as today");
        let succ = der_of(&ca1_cert);
        let a1 = super::issue_node_cert_for_key(&ca1, &NodeId::new("127.0.0.1", 9711).unwrap(), a.verifying_key_bytes(), 0).unwrap();
        a.activate(a.reroot_material(a.node_cert_der(), vec![der_of(&ca0.join("ca-cert.pem")), succ.clone()]).unwrap()).unwrap();
        assert!(!a_standing.legacy_closed(), "widened in the first succession: still open");
        a.activate(a.reroot_material(a1, vec![succ.clone()]).unwrap()).unwrap();
        assert!(a_standing.legacy_closed(), "narrowed to generation 1: closed");
        let (_b, b_standing) = start("b", &ca1, &ca1_cert, 9712);
        assert!(b_standing.legacy_closed(), "started on the successor's files, with no succession/: closed");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// **#796's review, must-address 6: a handshake judged under old roots records no anchor.** The writer reads the
    /// roots generation before it loads the client config; an anchor tagged with an older generation than the node's
    /// now reaches no sink, so it cannot land after the agent cleared the anchors at a change of roots. Seen failing
    /// first with the tag ignored: the old-roots anchor was recorded.
    #[test]
    fn an_anchor_from_a_handshake_judged_under_old_roots_is_not_recorded() {
        let base = fresh_dir("anchor-tag");
        let ca1 = base.join("ca1");
        super::mint_ca(&ca1).unwrap();
        let ca2 = base.join("ca2");
        let ca2_cert = super::mint_ca(&ca2).unwrap();
        let id = NodeId::new("127.0.0.1", 9713).unwrap();
        let (cert, key) = super::issue_node_cert(&ca1, &id, &base.join("issued")).unwrap();
        let cfg = TlsConfig { auto_cert_dir: base.join("node"), cert_pem: Some(cert), key_pem: Some(key), ca_cert_pem: Some(ca1.join("ca-cert.pem")) };
        let t = super::load_or_generate(&cfg, &id, Arc::new(RemovedSet::default()), Default::default()).unwrap();
        let recorded = Arc::new(std::sync::Mutex::new(Vec::<[u8; 32]>::new()));
        let sink = Arc::clone(&recorded);
        t.set_anchor_sink(Arc::new(move |_, k| sink.lock().unwrap().push(k)));
        let peer = NodeId::new("127.0.0.1", 9714).unwrap();
        let before = t.roots_generation();
        t.activate(t.reroot_material(t.node_cert_der(), vec![der_of(&ca1.join("ca-cert.pem")), der_of(&ca2_cert)]).unwrap()).unwrap();
        t.record_anchor(&peer, [1u8; 32], before);
        assert!(recorded.lock().unwrap().is_empty(), "judged under the old roots: not recorded");
        t.record_anchor(&peer, [2u8; 32], t.roots_generation());
        assert_eq!(*recorded.lock().unwrap(), vec![[2u8; 32]], "judged under the current roots: recorded");
        let _ = std::fs::remove_dir_all(&base);
    }
}

/// Identity-and-lifecycle I2, PR 2b: the old CA's commitment to its successor (`ca-commit.pem`, `tls commit`) and the
/// node's pins — one per committing CA, from the operator's file only, persisted, a second from the same CA refused.
#[cfg(all(test, feature = "tls"))]
mod ca_commit_tests {
    use super::{CaCommitError, CaCommitment, CommitmentPins, OfferedCommitment as Offered, PinnedCommitment};
    use crate::config::TlsConfig;
    use rustls::pki_types::{pem::PemObject, CertificateDer};
    use std::path::{Path, PathBuf};

    fn fresh_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("myc-i2-commit-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn der_of(p: &Path) -> Vec<u8> {
        CertificateDer::from_pem_file(p).unwrap().as_ref().to_vec()
    }

    fn ca_key(dir: &Path) -> ed25519_dalek::SigningKey {
        use ed25519_dalek::pkcs8::DecodePrivateKey;
        use base64::Engine;
        let pem = std::fs::read_to_string(dir.join("ca-key.pem")).unwrap();
        let b64: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
        ed25519_dalek::SigningKey::from_pkcs8_der(&base64::engine::general_purpose::STANDARD.decode(b64).unwrap()).unwrap()
    }

    /// A fleet CA (generation 0) in `base/ca1`, committed to a successor in `base/ca2`: (old dir, old DER, the
    /// commitment as read from `ca-commit.pem`).
    fn committed(base: &Path) -> (PathBuf, Vec<u8>, CaCommitment) {
        let d1 = base.join("ca1");
        let old = der_of(&super::imp::mint_ca(&d1).unwrap());
        let (file, _) = super::imp::commit_successor(&d1, &base.join("ca2")).unwrap();
        (d1, old, CaCommitment::from_pem(&std::fs::read_to_string(file).unwrap()).unwrap())
    }

    fn node_cfg(dir: &Path) -> TlsConfig {
        std::fs::create_dir_all(dir).unwrap();
        TlsConfig { auto_cert_dir: dir.to_path_buf(), ..TlsConfig::default() }
    }

    /// **Decision (a): the old CA signs `ca-commit.pem`, naming its successor, and a node assesses it against a root it
    /// already trusts.** `tls commit` mints the successor at the old CA's generation plus one, in the same fleet, and the
    /// commitment verifies against the old root and no other. Each thing a node cannot assess is refused by its name: a
    /// commitment whose named signer is no trusted root; one the named CA did not sign (a successor swapped under the
    /// signature; a commitment signed by another key that names the old CA — the forged record); a successor of the
    /// wrong generation; a successor that is the old CA again. Seen failing first with the signature check removed
    /// (the swapped successor and the forged commitment verified) and, in the same build, with the generation check
    /// removed (the generation-2 successor verified).
    #[test]
    fn a_commitment_is_the_old_cas_signature_over_its_successor_and_every_other_is_refused_by_name() {
        let base = fresh_dir("verify");
        let (d1, old, c) = committed(&base);
        let successor = der_of(&base.join("ca2").join("ca-cert.pem"));
        assert_eq!(c.successor_der, successor);
        assert_eq!(c.verify(std::slice::from_ref(&old)), Ok(1), "the successor is generation 1");
        assert_eq!(super::ca_generation(&successor), Ok(1));
        let fleet = |der: &[u8]| super::ca_subject_cn(der).unwrap().split(' ').nth(2).unwrap().to_string();
        assert_eq!(fleet(&old), fleet(&successor), "the same fleet");
        assert_ne!(super::ca_subject_cn(&old), super::ca_subject_cn(&successor), "a distinct subject");
        // The round trip, and what the file must not hold.
        assert_eq!(CaCommitment::from_pem(&c.to_pem()).unwrap(), c);
        assert!(!c.to_pem().contains("PRIVATE KEY"));

        let other = der_of(&super::imp::mint_ca(&base.join("other")).unwrap());
        assert_eq!(c.verify(std::slice::from_ref(&other)), Err(CaCommitError::UnknownPredecessor), "a root this node does not trust");
        assert_eq!(c.verify(&[]), Err(CaCommitError::UnknownPredecessor));
        assert_eq!(c.verify(&[other.clone(), old.clone()]), Ok(1), "found among several roots");

        // A successor swapped under the old CA's signature.
        let attacker = der_of(&super::imp::mint_ca_at(&base.join("attacker"), None, 1).unwrap());
        let swapped = CaCommitment { successor_der: attacker.clone(), ..c.clone() };
        assert_eq!(swapped.verify(std::slice::from_ref(&old)), Err(CaCommitError::BadSignature));
        // The forged record: signed by a key that is not the old CA's, naming the old CA as its signer.
        let forged_sig = super::sign_bytes(&ca_key(&base.join("attacker")), &CaCommitment::message(&c.predecessor, &attacker));
        let forged = CaCommitment { predecessor: c.predecessor, successor_der: attacker.clone(), sig: forged_sig };
        assert_eq!(forged.verify(std::slice::from_ref(&old)), Err(CaCommitError::BadSignature));
        assert!(CaCommitment::sign(&old, &ca_key(&base.join("attacker")), &attacker).is_err(), "a key that is not the certificate's signs nothing");

        // The generation: its predecessor's plus one, exactly.
        let key = ca_key(&d1);
        let gen2 = der_of(&super::imp::mint_ca_at(&base.join("gen2"), None, 2).unwrap());
        let signed = |successor: &[u8]| CaCommitment { predecessor: c.predecessor, successor_der: successor.to_vec(), sig: super::sign_bytes(&key, &CaCommitment::message(&c.predecessor, successor)) };
        assert_eq!(signed(&gen2).verify(std::slice::from_ref(&old)), Err(CaCommitError::WrongGeneration { predecessor: 0, successor: 2 }));
        assert_eq!(signed(&other).verify(std::slice::from_ref(&old)), Err(CaCommitError::WrongGeneration { predecessor: 0, successor: 0 }));
        assert_eq!(signed(&old).verify(std::slice::from_ref(&old)), Err(CaCommitError::WrongGeneration { predecessor: 0, successor: 0 }));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// **A `ca-commit.pem` is exactly one certificate and one commitment section, and no key.** Each other shape is
    /// refused as malformed, never read as "no commitment". Seen failing first with the private-key refusal removed.
    #[test]
    fn a_malformed_commitment_document_is_refused_and_one_carrying_a_key_too() {
        let base = fresh_dir("malformed");
        let (d1, _, c) = committed(&base);
        let pem = c.to_pem();
        let malformed = |doc: &str| matches!(CaCommitment::from_pem(doc), Err(CaCommitError::Malformed(_)));
        assert!(malformed(""), "nothing");
        assert!(malformed(&std::fs::read_to_string(d1.join("ca-cert.pem")).unwrap()), "a certificate alone");
        assert!(malformed(&format!("{pem}{}", std::fs::read_to_string(d1.join("ca-cert.pem")).unwrap())), "two certificates");
        let section = &pem[pem.find("-----BEGIN MYCELIUM").unwrap()..];
        assert!(malformed(&format!("{pem}{section}")), "two commitment sections");
        assert!(malformed(&format!("{pem}{}", std::fs::read_to_string(d1.join("ca-key.pem")).unwrap())), "a private key beside it");
        assert!(malformed(&pem.replace("-----END MYCELIUM CA COMMITMENT-----", "")), "no END line");
        let short = pem.replace(section, "-----BEGIN MYCELIUM CA COMMITMENT-----\nAQID\n-----END MYCELIUM CA COMMITMENT-----\n");
        assert!(malformed(&short), "a section of the wrong length");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// **`tls commit`: a CA commits once.** The first run mints the successor and publishes `ca-commit.pem` beside the
    /// old CA; a second run — into another successor directory — is refused naming the commitment, mints nothing and
    /// leaves the first commitment as it was. Seen failing first with the existing-commitment check removed and the
    /// file published by a plain write: the second run replaced the commitment with one naming another successor.
    #[test]
    fn tls_commit_commits_once_and_a_second_run_changes_nothing() {
        let base = fresh_dir("once");
        let (d1, _, first) = committed(&base);
        let before = std::fs::read(d1.join(super::CA_COMMIT_FILE)).unwrap();
        let err = super::imp::commit_successor(&d1, &base.join("ca3")).expect_err("a second commitment");
        assert!(err.to_string().contains("ca-commit.pem") && err.to_string().contains("commits once"), "{err}");
        assert!(!base.join("ca3").exists(), "no second successor minted");
        assert_eq!(std::fs::read(d1.join(super::CA_COMMIT_FILE)).unwrap(), before);
        assert_eq!(CaCommitment::from_pem(std::str::from_utf8(&before).unwrap()).unwrap(), first);
        // No CA key in the directory: refused, naming it, and nothing written.
        let keyless = base.join("keyless");
        std::fs::create_dir_all(&keyless).unwrap();
        std::fs::copy(d1.join("ca-cert.pem"), keyless.join("ca-cert.pem")).unwrap();
        assert!(super::imp::commit_successor(&keyless, &base.join("ca4")).expect_err("no key").to_string().contains("ca-key.pem"));
        assert!(!base.join("ca4").exists() && !keyless.join(super::CA_COMMIT_FILE).exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// **Decision (e): nothing new ships under the provisional OID.** With the gate closed, as a shipped build has it,
    /// `tls commit` is refused by name before it writes anything — no successor directory, no commitment — and a node
    /// given a commitment made by a test build refuses it by name and pins nothing. Seen failing first with
    /// `tls commit`'s own gate check removed: the refusal came from the mint, after the successor's directory was made.
    #[test]
    fn a_closed_provisional_gate_refuses_tls_commit_and_a_commitment_and_writes_nothing() {
        let base = fresh_dir("gate");
        let d1 = base.join("ca1");
        let old = der_of(&super::imp::mint_ca(&d1).unwrap());
        let err = super::with_provisional_ca_generation_closed(|| super::imp::commit_successor(&d1, &base.join("ca2"))).expect_err("closed");
        assert!(err.to_string().contains("1.3.6.1.4.1.32473.2.1") && err.to_string().contains("provisional") && err.to_string().contains("tls commit"), "{err}");
        assert!(!base.join("ca2").exists(), "no successor directory");
        assert!(!d1.join(super::CA_COMMIT_FILE).exists(), "no commitment");

        let (file, _) = super::imp::commit_successor(&d1, &base.join("ca2")).unwrap();
        let cfg = node_cfg(&base.join("node"));
        std::fs::copy(&file, cfg.auto_cert_dir.join(super::CA_COMMIT_FILE)).unwrap();
        let err = super::with_provisional_ca_generation_closed(|| super::imp::pin_ca_commitment(&cfg, std::slice::from_ref(&old))).expect_err("closed");
        assert!(err.to_string().contains("provisional") && err.to_string().contains("ca-commit.pem"), "{err}");
        assert!(!cfg.auto_cert_dir.join(super::SUCCESSION_DIR).exists(), "nothing pinned");
        let _ = std::fs::remove_dir_all(&base);
    }

    fn pin_path(cfg: &TlsConfig, c: &CaCommitment) -> PathBuf {
        cfg.auto_cert_dir.join(super::SUCCESSION_DIR).join(super::ca_commit_pin_file(&c.predecessor))
    }

    fn pinned(c: &CaCommitment, first: bool, active: bool) -> PinnedCommitment {
        PinnedCommitment { commitment: c.clone(), first, active }
    }

    fn pins(cfg: &TlsConfig, roots: &[Vec<u8>]) -> CommitmentPins {
        super::imp::pin_ca_commitment(cfg, roots).unwrap()
    }

    /// **The decisions of 2026-11-15 (a) and 2026-11-16: a node pins the first commitment its operator places, persists
    /// it, and refuses another from that CA.** No file, no pin. The operator's file is assessed and pinned under the
    /// node's own `succession/`, in a file named for the committing CA, and never written. The pin is read back at the
    /// next start, with the operator's file gone too. A **second commitment validly signed by the old CA's key** — what
    /// a holder of a stolen CA key would place — is refused: the pin stands and the refusal is reported with the
    /// other's digest. A forged commitment (not signed by the old CA) offered to a node with no pin refuses the start by
    /// name and pins nothing. Seen failing first with a second commitment from a pinned CA not refused: no conflict
    /// was reported.
    #[test]
    fn a_node_pins_the_operators_commitment_persists_it_and_refuses_another() {
        let base = fresh_dir("pin");
        let (d1, old, c) = committed(&base);
        let roots = vec![old.clone()];
        let cfg = node_cfg(&base.join("node"));
        let file = cfg.auto_cert_dir.join(super::CA_COMMIT_FILE);
        assert_eq!(pins(&cfg, &roots), CommitmentPins { pins: vec![], offered: Offered::Nothing });

        std::fs::copy(d1.join(super::CA_COMMIT_FILE), &file).unwrap();
        let placed = std::fs::read(&file).unwrap();
        assert_eq!(pins(&cfg, &roots), CommitmentPins { pins: vec![pinned(&c, true, true)], offered: Offered::Nothing });
        assert_eq!(std::fs::read(&file).unwrap(), placed, "the operator's file is never written");
        let pin = pin_path(&cfg, &c);
        assert_eq!(CaCommitment::from_pem(&std::fs::read_to_string(&pin).unwrap()).unwrap(), c, "persisted under succession/, named for its CA");
        assert_eq!(std::fs::read_dir(pin.parent().unwrap()).unwrap().count(), 1, "no temporary left");
        let read_back = CommitmentPins { pins: vec![pinned(&c, false, true)], offered: Offered::Nothing };
        assert_eq!(pins(&cfg, &roots), read_back);
        std::fs::remove_file(&file).unwrap();
        assert_eq!(pins(&cfg, &roots), read_back, "the pin is the node's memory");

        // A second commitment the old CA's key did sign: a stolen key's.
        let rival = der_of(&super::imp::mint_ca_at(&base.join("rival"), None, 1).unwrap());
        let second = CaCommitment::sign(&old, &ca_key(&d1), &rival).unwrap();
        assert_eq!(second.verify(&roots), Ok(1), "it verifies: only the pin refuses it");
        std::fs::write(&file, second.to_pem()).unwrap();
        assert_eq!(pins(&cfg, &roots), CommitmentPins { pins: vec![pinned(&c, false, true)], offered: Offered::Conflict(second.digest()) });
        assert_eq!(CaCommitment::from_pem(&std::fs::read_to_string(&pin).unwrap()).unwrap(), c, "the pin stands");
        assert_eq!(std::fs::read_dir(pin.parent().unwrap()).unwrap().count(), 1, "and no second pin was written");

        // A forged commitment, offered to a node with no pin.
        let fresh = node_cfg(&base.join("node2"));
        let forged = CaCommitment { sig: super::sign_bytes(&ca_key(&base.join("rival")), &CaCommitment::message(&c.predecessor, &rival)), ..second.clone() };
        std::fs::write(fresh.auto_cert_dir.join(super::CA_COMMIT_FILE), forged.to_pem()).unwrap();
        let err = super::imp::pin_ca_commitment(&fresh, &roots).expect_err("forged");
        assert!(err.to_string().contains("ca-commit.pem") && err.to_string().contains("did not sign"), "{err}");
        assert!(!fresh.auto_cert_dir.join(super::SUCCESSION_DIR).exists(), "nothing pinned");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// CA0 in `base/ca1` committed to CA1 (`base/ca2`), and CA1 to CA2 (`base/ca3`): the three certificates and the
    /// two commitments.
    fn two_successions(base: &Path) -> ([Vec<u8>; 3], [CaCommitment; 2]) {
        let (_, ca0, first) = committed(base);
        let (file, _) = super::imp::commit_successor(&base.join("ca2"), &base.join("ca3")).unwrap();
        let second = CaCommitment::from_pem(&std::fs::read_to_string(file).unwrap()).unwrap();
        ([ca0, first.successor_der.clone(), second.successor_der.clone()], [first, second])
    }

    fn place(cfg: &TlsConfig, c: &CaCommitment) {
        std::fs::write(cfg.auto_cert_dir.join(super::CA_COMMIT_FILE), c.to_pem()).unwrap();
    }

    /// **The decision of 2026-11-18: a pin is per committing CA.** A node pinned CA0's commitment to CA1 and now trusts
    /// only CA1 (that pin is spent). CA1's own commitment to CA2, placed by the operator, is **pinned** — a second pin,
    /// in its own file, beside the first — and is not a conflict: a different promiser, about its own succession. Both
    /// pins are read back at the next start, the first spent and the second active. A **second commitment from CA1**
    /// is then refused as a conflict and pins nothing. The reviewer's F3 test, adopted to the decision. Seen failing
    /// first with one pin for the node (a commitment by a CA the node holds no pin for reported, not pinned): CA1's
    /// commitment was not pinned.
    #[test]
    fn the_successors_own_commitment_is_pinned_beside_a_spent_pin_and_a_second_from_it_is_refused() {
        let base = fresh_dir("per-ca");
        let ([ca0, ca1, _], [first, second]) = two_successions(&base);
        let cfg = node_cfg(&base.join("node"));
        place(&cfg, &first);
        pins(&cfg, std::slice::from_ref(&ca0));
        let now = std::slice::from_ref(&ca1);
        place(&cfg, &second);
        let got = pins(&cfg, now);
        assert_eq!(got.offered, Offered::Nothing, "not a conflict, and not left unpinned");
        assert!(got.pins.contains(&pinned(&first, false, false)) && got.pins.contains(&pinned(&second, true, true)), "{got:?}");
        assert_eq!(got.pins.len(), 2);
        assert!(pin_path(&cfg, &first).exists() && pin_path(&cfg, &second).exists(), "one file per committing CA");
        // The restart with two pins.
        let again = pins(&cfg, now);
        assert!(again.pins.contains(&pinned(&first, false, false)) && again.pins.contains(&pinned(&second, false, true)), "{again:?}");
        assert_eq!((again.pins.len(), &again.offered), (2, &Offered::Nothing));
        // A second commitment CA1's key signed.
        let rival = der_of(&super::imp::mint_ca_at(&base.join("rival"), None, 2).unwrap());
        let stolen = CaCommitment::sign(&ca1, &ca_key(&base.join("ca2")), &rival).unwrap();
        place(&cfg, &stolen);
        let got = pins(&cfg, now);
        assert_eq!(got.offered, Offered::Conflict(stolen.digest()), "the same CA's second commitment");
        assert_eq!(got.pins.len(), 2, "nothing more pinned");
        assert_eq!(CaCommitment::from_pem(&std::fs::read_to_string(pin_path(&cfg, &second)).unwrap()).unwrap(), second, "CA1's pin stands");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// **The decision of 2026-11-18: a node pins a CA's commitment only once it trusts that CA.** While the node still
    /// trusts only CA0, CA1's commitment to CA2 is refused by name — its committing CA is no root this node trusts —
    /// and pins nothing: reported beside the pin the node holds, and, on a node with no pin, refusing the start. Seen
    /// failing first with a commitment pinned whenever the node holds no pin for its CA, assessed or not: CA1's
    /// commitment was pinned by a node that did not trust CA1.
    #[test]
    fn a_commitment_from_a_ca_the_node_does_not_trust_yet_is_refused_by_name_and_pins_nothing() {
        let base = fresh_dir("not-yet");
        let ([ca0, _, _], [first, second]) = two_successions(&base);
        let cfg = node_cfg(&base.join("node"));
        place(&cfg, &first);
        let roots = std::slice::from_ref(&ca0);
        pins(&cfg, roots);
        place(&cfg, &second);
        let got = pins(&cfg, roots);
        assert_eq!(got, CommitmentPins { pins: vec![pinned(&first, false, true)], offered: Offered::UntrustedCa });
        assert!(!pin_path(&cfg, &second).exists(), "nothing pinned");
        let fresh = node_cfg(&base.join("node2"));
        place(&fresh, &second);
        let err = super::imp::pin_ca_commitment(&fresh, roots).expect_err("no pin, an untrusted CA's commitment").to_string();
        assert!(err.contains("ca-commit.pem") && err.contains("no root this node trusts"), "{err}");
        assert!(!fresh.auto_cert_dir.join(super::SUCCESSION_DIR).exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// **A pin outlives the root that made it; one this node can no longer place refuses the start, by its own name;
    /// and removing one pin leaves the others.** Widened, the first pin is active; narrowed, it is spent. After the
    /// second succession — the node trusts only CA2 — the first pin stands because the second continues from it. A
    /// node whose roots are unrelated to a pin is refused the start naming **that pin's file** and the way out, which
    /// is to remove that one file: with it removed the node starts and its other pin is untouched. Seen failing first
    /// with an unplaceable pin read as no pin: the node started, and would have pinned that CA's next commitment.
    #[test]
    fn a_spent_pin_is_kept_and_an_unassessable_one_refuses_the_start_until_that_pin_is_removed() {
        let base = fresh_dir("spent");
        let ([ca0, ca1, ca2], [first, second]) = two_successions(&base);
        let cfg = node_cfg(&base.join("node"));
        place(&cfg, &first);
        pins(&cfg, std::slice::from_ref(&ca0));
        assert_eq!(pins(&cfg, &[ca0.clone(), ca1.clone()]).pins, vec![pinned(&first, false, true)], "widened: active");
        assert_eq!(pins(&cfg, std::slice::from_ref(&ca1)).pins, vec![pinned(&first, false, false)], "narrowed: spent");
        place(&cfg, &second);
        pins(&cfg, std::slice::from_ref(&ca1));
        // Narrowed again: only CA2. The first pin's successor is neither a root nor absent from the chain.
        let got = pins(&cfg, std::slice::from_ref(&ca2));
        assert_eq!(got.pins.len(), 2, "{got:?}");
        assert!(got.pins.iter().all(|p| !p.active), "both spent");
        // Roots unrelated to either pin: refused, naming the pin.
        let unrelated = der_of(&super::imp::mint_ca(&base.join("unrelated")).unwrap());
        let err = super::imp::pin_ca_commitment(&cfg, std::slice::from_ref(&unrelated)).expect_err("unplaceable").to_string();
        assert!(err.contains("pin-") && err.contains("remove") && err.contains("phase"), "{err}");
        assert!(err.contains("2 pinned CA commitment(s)"), "both pins are named in the one refusal: {err}");
        // The first pin alone cannot be placed once the second is gone and the roots are CA2's; removing the first is
        // the way out, and the second is untouched.
        std::fs::remove_file(cfg.auto_cert_dir.join(super::CA_COMMIT_FILE)).unwrap();
        std::fs::remove_file(pin_path(&cfg, &first)).unwrap();
        assert_eq!(pins(&cfg, std::slice::from_ref(&ca2)).pins, vec![pinned(&second, false, false)], "one pin removed, the other untouched");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// **A pin file is the pin its name says.** A pin's name is its committing CA's digest; a file under another CA's
    /// name — moved, or copied over — refuses the start naming the file, as a pin that does not parse does. Seen
    /// failing first with the name not compared: the node held a pin for a CA under another's name.
    #[test]
    fn a_pin_under_another_cas_name_and_a_damaged_pin_refuse_the_start_naming_the_file() {
        let base = fresh_dir("misnamed");
        let ([ca0, ca1, _], [first, second]) = two_successions(&base);
        let cfg = node_cfg(&base.join("node"));
        place(&cfg, &first);
        pins(&cfg, std::slice::from_ref(&ca0));
        std::fs::remove_file(cfg.auto_cert_dir.join(super::CA_COMMIT_FILE)).unwrap();
        let wrong = cfg.auto_cert_dir.join(super::SUCCESSION_DIR).join(super::ca_commit_pin_file(&second.predecessor));
        std::fs::copy(pin_path(&cfg, &first), &wrong).unwrap();
        let err = super::imp::pin_ca_commitment(&cfg, &[ca0.clone(), ca1.clone()]).expect_err("misnamed").to_string();
        assert!(err.contains("another CA than its name says") && err.contains("remove"), "{err}");
        std::fs::write(&wrong, b"damaged").unwrap();
        let err = super::imp::pin_ca_commitment(&cfg, std::slice::from_ref(&ca0)).expect_err("damaged").to_string();
        assert!(err.contains("pin-") && err.contains("malformed") && err.contains("remove"), "{err}");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// **#812's review F3 (a) and its confirm round's C1: beside a pin, an operator's file the node cannot assess is
    /// never called a conflict, and never passed over.** On a node that holds a pin:
    /// - a document no CA signed (a garbage signature under the pinned CA's name), text that is not a commitment, an
    ///   **empty** file, and the pinned commitment **with the CA's private key in the file** each **refuse the start**
    ///   naming the file, exactly as on a node with no pin — none is a `Conflict`;
    /// - a well-formed commitment from a CA the node does not trust is the one file the start goes on with, reported
    ///   as `UntrustedCa`, nothing pinned;
    /// - only a second commitment the pinned CA's key signed is a `Conflict`.
    ///
    /// The pin stands through all of it. The reviewer's tests, adopted. Seen failing first, one build each: with a
    /// differing file for a pinned CA called a conflict without being assessed; and with every file a node with a pin
    /// cannot assess passed over (the start went on with the empty file, the text, the key and the garbage signature).
    #[test]
    fn beside_a_pin_an_unverifiable_operator_file_is_not_a_conflict() {
        let base = fresh_dir("offered");
        let (d1, old, c) = committed(&base);
        let roots = std::slice::from_ref(&old);
        let cfg = node_cfg(&base.join("node"));
        let file = cfg.auto_cert_dir.join(super::CA_COMMIT_FILE);
        place(&cfg, &c);
        pins(&cfg, roots);
        let offered = |doc: &str| {
            std::fs::write(&file, doc).unwrap();
            super::imp::pin_ca_commitment(&cfg, roots).map(|got| {
                assert_eq!(got.pins, vec![pinned(&c, false, true)], "the pin stands, and nothing more is pinned");
                got.offered
            })
        };
        let refused = |doc: &str, why: &str| {
            let err = offered(doc).expect_err(why).to_string();
            assert!(err.contains("ca-commit.pem") && err.contains("is refused"), "{why}: {err}");
            assert_eq!(std::fs::read_dir(pin_path(&cfg, &c).parent().unwrap()).unwrap().count(), 1, "{why}: the pin is as it was");
            err
        };
        let attacker = der_of(&super::imp::mint_ca_at(&base.join("attacker"), None, 1).unwrap());
        let forged = CaCommitment { predecessor: c.predecessor, successor_der: attacker.clone(), sig: [7u8; 64] };
        assert!(refused(&forged.to_pem(), "no CA signed it").contains("did not sign"));
        refused("not a commitment", "text");
        refused("", "an empty file");
        let with_key = format!("{}{}", c.to_pem(), std::fs::read_to_string(d1.join("ca-key.pem")).unwrap());
        assert!(refused(&with_key, "the CA's key in the file").contains("private key"));
        let elsewhere = CaCommitment { predecessor: [9u8; 32], ..forged.clone() };
        assert_eq!(offered(&elsewhere.to_pem()).unwrap(), Offered::UntrustedCa, "a CA this node does not trust: the start goes on");
        let second = CaCommitment::sign(&old, &ca_key(&d1), &attacker).unwrap();
        assert_eq!(offered(&second.to_pem()).unwrap(), Offered::Conflict(second.digest()), "the pinned CA's key signed this one");
        assert_eq!(offered(&c.to_pem()).unwrap(), Offered::Nothing);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// **#812's confirm round C2: one refusal names every pin that does not stand.** Four successions, the node
    /// trusting only the last CA; the operator removes the pin of the third. The two older pins stood only through
    /// it: the next start is refused **naming both files**, and says that a pin of an earlier succession stands only
    /// through the pins after it. With both removed the node starts, the newest pin untouched. The reviewer's
    /// scenario. Seen failing first with the refusal naming the first such pin alone.
    #[test]
    fn removing_a_middle_pin_refuses_the_start_naming_every_pin_that_no_longer_stands() {
        let base = fresh_dir("middle");
        let ([_, _, _], [first, second]) = two_successions(&base);
        let chain = |from: &str, to: &str| {
            let (file, _) = super::imp::commit_successor(&base.join(from), &base.join(to)).unwrap();
            CaCommitment::from_pem(&std::fs::read_to_string(file).unwrap()).unwrap()
        };
        let (third, fourth) = (chain("ca3", "ca4"), chain("ca4", "ca5"));
        let cfg = node_cfg(&base.join("node"));
        // Pinned one succession at a time, each while its committing CA is the node's root.
        for c in [&first, &second, &third, &fourth] {
            place(&cfg, c);
            let root = if c.predecessor == first.predecessor { der_of(&base.join("ca1").join("ca-cert.pem")) } else {
                [&first, &second, &third].iter().find(|p| super::sha256(&p.successor_der) == c.predecessor).unwrap().successor_der.clone()
            };
            pins(&cfg, std::slice::from_ref(&root));
        }
        std::fs::remove_file(cfg.auto_cert_dir.join(super::CA_COMMIT_FILE)).unwrap();
        let last = std::slice::from_ref(&fourth.successor_der);
        assert_eq!(pins(&cfg, last).pins.len(), 4, "the whole chain stands");
        std::fs::remove_file(pin_path(&cfg, &third)).unwrap();
        let err = super::imp::pin_ca_commitment(&cfg, last).expect_err("two pins no longer stand").to_string();
        let name = |c: &CaCommitment| super::ca_commit_pin_file(&c.predecessor);
        assert!(err.contains(&name(&first)) && err.contains(&name(&second)), "both are named: {err}");
        assert!(!err.contains(&name(&fourth)), "the newest pin stands: {err}");
        assert!(err.contains("stands only") && err.contains("every file named here"), "{err}");
        std::fs::remove_file(pin_path(&cfg, &first)).unwrap();
        std::fs::remove_file(pin_path(&cfg, &second)).unwrap();
        assert_eq!(pins(&cfg, last).pins, vec![pinned(&fourth, false, false)], "the newest pin is untouched");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// **#812's review F5: a `ca-commit.pem` that is not a commitment is named as an interrupted run, with what to
    /// do.** An empty file under the commitment's name refuses `tls commit` saying it is not a commitment, that no node
    /// can pin from it, and how to go on — not "this CA has already committed". Seen failing first with any existing
    /// file read as a commitment already made.
    #[test]
    fn an_empty_commitment_file_is_named_as_an_interrupted_run_not_as_a_commitment() {
        let base = fresh_dir("empty");
        let d1 = base.join("ca1");
        super::imp::mint_ca(&d1).unwrap();
        std::fs::write(d1.join(super::CA_COMMIT_FILE), b"").unwrap();
        let err = super::imp::commit_successor(&d1, &base.join("ca2")).expect_err("an empty file").to_string();
        assert!(err.contains("not a commitment") && err.contains("interrupted") && err.contains("remove"), "{err}");
        assert!(!err.contains("already committed"), "{err}");
        assert!(!base.join("ca2").exists(), "nothing minted");
        std::fs::remove_file(d1.join(super::CA_COMMIT_FILE)).unwrap();
        super::imp::commit_successor(&d1, &base.join("ca2")).expect("commits once the empty file is gone");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// **#812's review F7: an unreadable generation is blamed on the certificate that carries it.** A commitment whose
    /// committing CA's own generation cannot be read is refused as the predecessor's, not the successor's. Seen
    /// failing first with both mapped to `SuccessorGeneration`.
    #[test]
    fn an_unreadable_predecessor_generation_is_named_as_the_predecessors() {
        let base = fresh_dir("pred-gen");
        let (_, old, c) = committed(&base);
        // Closed, a generation-0 root (no extension) still reads; the successor's does not.
        let got = super::with_provisional_ca_generation_closed(|| c.verify(std::slice::from_ref(&old)));
        assert!(matches!(got, Err(CaCommitError::SuccessorGeneration(_))), "{got:?}");
        // A committing CA of generation 1 (it carries the extension), read by a closed build.
        let (file2, _) = super::imp::commit_successor(&base.join("ca2"), &base.join("ca3")).unwrap();
        let next = CaCommitment::from_pem(&std::fs::read_to_string(&file2).unwrap()).unwrap();
        let got = super::with_provisional_ca_generation_closed(|| next.verify(std::slice::from_ref(&c.successor_der)));
        assert!(matches!(got, Err(CaCommitError::PredecessorGeneration(_))), "{got:?}");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// **#812's second confirm round, D3: a differing commitment naming a CA the node has moved past is said to be
    /// that, not "placed early".** A node holds only a spent pin for CA0 (it trusts CA1). A second commitment CA0's
    /// key signed is offered: the node can no longer check CA0's signature, so it is not a conflict; it is reported as
    /// differing from the pin held for a CA the node has moved past, pins nothing, and the start goes on. The early
    /// file keeps its own wording. Seen failing first with the case reported as a commitment from a CA the node does
    /// not trust yet: the text asked whether it was placed before the node cut over.
    #[test]
    fn a_second_commitment_naming_a_ca_the_node_moved_past_is_not_described_as_early() {
        let base = fresh_dir("moved-past");
        let (d1, old, c) = committed(&base);
        let cfg = node_cfg(&base.join("node"));
        place(&cfg, &c);
        pins(&cfg, std::slice::from_ref(&old));
        let rival = der_of(&super::imp::mint_ca_at(&base.join("rival"), None, 1).unwrap());
        let second = CaCommitment::sign(&old, &ca_key(&d1), &rival).unwrap();
        place(&cfg, &second);
        let got = pins(&cfg, std::slice::from_ref(&c.successor_der));
        assert_eq!(got, CommitmentPins { pins: vec![pinned(&c, false, false)], offered: Offered::DiffersFromSpentPin(second.digest()) });
        let said = got.offered.describe().unwrap();
        assert!(said.contains("already pinned") && said.contains("moved past") && said.contains("differs from that pin"), "{said}");
        assert!(!said.contains("before this node cut over"), "{said}");
        assert!(Offered::UntrustedCa.describe().unwrap().contains("before this node cut over"), "the early file keeps its wording");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// **#812's review F5 and its confirm rounds' C6, witnessed: `tls commit` makes the promise durable before it
    /// names it.** From the steps the command reports as each returns: the successor's key and certificate are
    /// synced, then the successor's directory and that directory's entry in its parent, then the commitment's bytes,
    /// **then** its name is linked, then the directory holding the name is synced. Seen failing first with the sync of
    /// the commitment's bytes removed: the name was linked with no file sync before it.
    #[test]
    fn tls_commit_syncs_the_successor_and_the_commitment_before_it_links_the_name() {
        let base = fresh_dir("sync-order");
        let d1 = base.join("ca1");
        super::imp::mint_ca(&d1).unwrap();
        let d2 = base.join("ca2");
        let mut steps: Vec<(&'static str, PathBuf)> = Vec::new();
        let (out, _) = super::imp::commit_successor_observed(&d1, &d2, &mut |what, path| steps.push((what, path.to_path_buf()))).unwrap();
        let named: Vec<(&str, String)> = steps.iter().map(|(what, p)| (*what, p.file_name().unwrap().to_string_lossy().into_owned())).collect();
        let tmp = named[4].1.clone();
        assert!(tmp.starts_with(".ca-commit.tmp-"), "{named:?}");
        let want: Vec<(&str, String)> = vec![
            ("file.sync", "ca-key.pem".into()),
            ("file.sync", "ca-cert.pem".into()),
            ("dir.sync", "ca2".into()),
            ("dir.sync", base.file_name().unwrap().to_string_lossy().into_owned()),
            ("file.sync", tmp),
            ("link", "ca-commit.pem".into()),
            ("dir.sync", "ca1".into()),
        ];
        assert_eq!(named, want, "the order of the durability steps");
        assert_eq!(steps[5].1, out);
        let _ = std::fs::remove_dir_all(&base);
    }
}
