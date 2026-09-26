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
    /// (no client cert) can connect. Built here so it rotates with the identity;
    /// used only when `GossipConfig::gateway_tls` reuses the node cert.
    #[cfg(feature = "tls")]
    gateway_server_config: arc_swap::ArcSwap<rustls::ServerConfig>,
    /// Identity-anchor sink (identity-auth Phase 1b): records a directly-connected peer's
    /// CA-validated Ed25519 key. Installed once at agent start with a closure capturing the
    /// `CoreCtx` anchor maps; the outbound writer calls it after each handshake. `OnceLock` so
    /// the hot connect path reads it lock-free and it is set exactly once. Not rotated (the sink
    /// captures long-lived map `Arc`s, independent of the swapped cert/config).
    #[cfg(feature = "tls")]
    peer_anchor_sink: std::sync::OnceLock<AnchorSink>,
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
    /// The current server-only rustls config for the HTTP gateway (node-cert reuse
    /// path). Rotates with the identity via [`activate`](NodeTls::activate).
    pub fn gateway_server_config(&self) -> std::sync::Arc<rustls::ServerConfig> {
        self.gateway_server_config.load_full()
    }

    /// Install the identity-anchor sink (identity-auth Phase 1b) — called once at agent start
    /// with a closure that records a peer's CA-validated key into the `CoreCtx` anchor maps.
    /// Idempotent (first wins).
    pub fn set_anchor_sink(&self, sink: AnchorSink) {
        let _ = self.peer_anchor_sink.set(sink);
    }

    /// Record a directly-connected peer's CA-validated key via the installed sink; no-op if none
    /// is installed. Called by the outbound writer after a completed handshake.
    pub fn record_anchor(&self, peer: &crate::node_id::NodeId, key: [u8; 32]) {
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
    pub fn activate(&self, m: RotationMaterial) {
        self.signing_key.store(m.signing_key);
        self.server_config.store(m.server_config);
        self.client_config.store(m.client_config);
        self.gateway_server_config.store(m.gateway_server_config);
    }
}

/// A freshly-generated identity key + CA-signed cert + rustls configs, not yet
/// activated. Produced by `generate_rotation`; consumed by `NodeTls::activate`.
#[cfg(feature = "tls")]
pub struct RotationMaterial {
    pub verifying_key: [u8; 32],
    server_config: std::sync::Arc<rustls::ServerConfig>,
    client_config: std::sync::Arc<rustls::ClientConfig>,
    signing_key: std::sync::Arc<ed25519_dalek::SigningKey>,
    gateway_server_config: std::sync::Arc<rustls::ServerConfig>,
}

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
            let der = pem_cert_to_der(&pem)?;
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
            match fs::OpenOptions::new().write(true).create_new(true).open(&lock_path) {
                Ok(_) => {
                    let result = if present() {
                        load()
                    } else {
                        generate_and_publish_ca(dir, &auto_ca_cert_path, &auto_ca_key_path)
                    };
                    let _ = fs::remove_file(&lock_path);
                    return result;
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

    /// Generate a CA and publish it: each file written under a temporary name, then renamed into
    /// place. Called only while holding `ca.lock`.
    fn generate_and_publish_ca(
        dir: &Path,
        cert_path: &Path,
        key_path: &Path,
    ) -> Result<(CertificateDer<'static>, KeyPair), GossipError> {
        let err = |reason: String| GossipError::InvalidField { field: "tls", reason };
        let ca_key_pair = KeyPair::generate_for(&PKCS_ED25519).map_err(|e| err(format!("TLS: generate CA key: {e}")))?;
        let mut ca_params = CertificateParams::new(vec![]).map_err(|e| err(format!("TLS: CA params: {e}")))?;
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        ca_params.not_after = rcgen::date_time_ymd(2099, 1, 1);
        let ca_cert = ca_params.self_signed(&ca_key_pair).map_err(|e| err(format!("TLS: self-sign CA: {e}")))?;
        let ca_cert_der = CertificateDer::from(ca_cert.der().to_vec());

        let pid = std::process::id();
        let publish = |path: &Path, contents: String, what: &str| -> Result<(), GossipError> {
            let tmp = dir.join(format!(".{what}.tmp-{pid}"));
            fs::write(&tmp, contents).map_err(|e| err(format!("TLS: write CA {what}: {e}")))?;
            fs::rename(&tmp, path).map_err(|e| err(format!("TLS: publish CA {what}: {e}")))
        };
        publish(key_path, ca_key_pair.serialize_pem(), "key")?;
        publish(cert_path, cert_der_to_pem(ca_cert_der.as_ref()), "cert")?;

        tracing::info!("TLS: generated new cluster CA in {dir:?} — distribute ca-cert.pem to all nodes");
        Ok((ca_cert_der, ca_key_pair))
    }

    pub fn load_or_generate(
        cfg: &TlsConfig,
        node_id: &NodeId,
        removed: Arc<crate::removal::RemovedSet>,
    ) -> Result<NodeTls, GossipError> {
        fs::create_dir_all(&cfg.auto_cert_dir).map_err(|e| {
            GossipError::InvalidField { field: "tls", reason: format!("TLS: cannot create cert dir {:?}: {e}", cfg.auto_cert_dir) }
        })?;

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

        // ── 2. CA cert + key ──────────────────────────────────────────────
        let (ca_cert_der, ca_key_pair) = load_or_create_ca(cfg)?;

        // ── 3. Node cert (regenerated every startup, signed by CA) ───────
        let node_cert_der = generate_node_cert(node_id, &signing_key, &ca_key_pair)?;

        // ── 4. Build rustls configs ───────────────────────────────────────
        let (server_config, client_config, gateway_server_config) =
            build_rustls_configs(node_cert_der, &signing_key, ca_cert_der, removed)?;

        Ok(NodeTls {
            server_config: arc_swap::ArcSwap::from_pointee(server_config),
            client_config: arc_swap::ArcSwap::from_pointee(client_config),
            signing_key: arc_swap::ArcSwap::from_pointee(signing_key),
            gateway_server_config: arc_swap::ArcSwap::from_pointee(gateway_server_config),
            peer_anchor_sink: std::sync::OnceLock::new(),
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
    ) -> Result<super::RotationMaterial, GossipError> {
        let signing_key = generate_key()?;
        let verifying_key = signing_key.verifying_key().to_bytes();

        // Persist the new key (raw 32 bytes), same layout as load_or_generate.
        let sanitized = node_id.as_str().replace([':', '.'], "_");
        let auto_key_path = cfg.auto_cert_dir.join(format!("{sanitized}.key"));
        save_key_raw(&signing_key, &auto_key_path)?;

        let (ca_cert_der, ca_key_pair) = load_existing_ca(cfg)?;
        let node_cert_der = generate_node_cert(node_id, &signing_key, &ca_key_pair)?;
        let (server_config, client_config, gateway_server_config) =
            build_rustls_configs(node_cert_der, &signing_key, ca_cert_der, removed)?;

        Ok(super::RotationMaterial {
            verifying_key,
            server_config: Arc::new(server_config),
            client_config: Arc::new(client_config),
            signing_key: Arc::new(signing_key),
            gateway_server_config: Arc::new(gateway_server_config),
        })
    }

    /// Load the existing cluster CA cert + key (load-only; errors if absent —
    /// unlike `load_or_generate`, rotation must never mint a new CA).
    fn load_existing_ca(cfg: &TlsConfig) -> Result<(CertificateDer<'static>, KeyPair), GossipError> {
        let auto_ca_cert_path = cfg.auto_cert_dir.join("ca-cert.pem");
        let auto_ca_key_path  = cfg.auto_cert_dir.join("ca-key.pem");
        let ca_cert_path = cfg.ca_cert_pem.clone().unwrap_or(auto_ca_cert_path);
        let pem = fs::read_to_string(&ca_cert_path).map_err(|e| GossipError::InvalidField {
            field: "tls", reason: format!("TLS: rotation needs an existing CA cert ({ca_cert_path:?}): {e}"),
        })?;
        let ca_cert_der = pem_cert_to_der(&pem)?;
        let key_pem = fs::read_to_string(&auto_ca_key_path).map_err(|e| GossipError::InvalidField {
            field: "tls", reason: format!("TLS: rotation needs the CA key ({auto_ca_key_path:?}): {e}"),
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
        fs::write(path, key.as_bytes())
            .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: write key {:?}: {e}", path) })
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

    pub(super) fn generate_node_cert(
        node_id: &NodeId,
        signing_key: &SigningKey,
        ca_key_pair: &KeyPair,
    ) -> Result<CertificateDer<'static>, GossipError> {
        use ed25519_dalek::pkcs8::EncodePrivateKey;

        // Convert ed25519-dalek key → rcgen KeyPair via PKCS8 DER
        let pkcs8 = signing_key
            .to_pkcs8_der()
            .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: encode node key: {e}") })?;
        let node_key_pair = KeyPair::try_from(pkcs8.as_bytes())
            .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: rcgen node key: {e}") })?;

        // Add the node's IP address as a Subject Alternative Name
        let ip: std::net::IpAddr = node_id.to_socket_addr().ip();
        let mut params = CertificateParams::new(vec![])
            .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: node cert params: {e}") })?;
        params.subject_alt_names = vec![SanType::IpAddress(ip)];
        params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        params.not_after  = rcgen::date_time_ymd(2099, 1, 1);

        // Reconstruct the CA Certificate for signing from the key pair + known fixed params.
        // rcgen 0.13 removed CertificateParams::from_ca_cert_der; since Mycelium always
        // generates its own CA with these exact params, reconstruction is deterministic.
        // Rustls verifies the chain via the SubjectKeyIdentifier (public-key hash), not the
        // serial number, so the reconstructed cert's AKI matches the saved ca-cert.pem.
        let mut ca_params = CertificateParams::new(vec![])
            .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: CA params for signing: {e}") })?;
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        ca_params.not_after  = rcgen::date_time_ymd(2099, 1, 1);
        let signing_ca = ca_params
            .self_signed(ca_key_pair)
            .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: reconstruct CA for signing: {e}") })?;

        let node_cert = params
            .signed_by(&node_key_pair, &signing_ca, ca_key_pair)
            .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: sign node cert: {e}") })?;

        Ok(CertificateDer::from(node_cert.der().to_vec()))
    }

    /// Extract the Ed25519 public key from a node cert's `SubjectPublicKeyInfo` (audit 2026-07-15,
    /// identity-authentication Phase 1a — `docs/design/identity-authentication.md`).
    ///
    /// The trust anchor for peer identity is the **CA-signed cert**: rustls validates the peer's cert
    /// against the cluster CA during the handshake, so by the time this runs the DER is a well-formed,
    /// CA-issued Ed25519 cert produced by [`generate_node_cert`]. That makes a targeted, length-checked
    /// scan for the fixed Ed25519 SPKI prefix both safe (the input is validated) and dependency-free
    /// (no x509 parser needed). The Ed25519 SPKI is exactly
    /// `30 2A 30 05 06 03 2B 65 70 03 21 00 ‖ key[32]` — OID `1.3.101.112` then a 33-byte BIT STRING
    /// (0 unused bits + the 32-byte key). Returns `None` for a non-Ed25519 or malformed input (never
    /// panics — the slice access is bounds-checked).
    pub fn ed25519_key_from_cert_der(der: &[u8]) -> Option<[u8; 32]> {
        // OID 1.3.101.112 (Ed25519) followed by BIT STRING tag+len+unused-bits: `06 03 2B 65 70 03 21 00`.
        const SPKI_PREFIX: &[u8] = &[0x06, 0x03, 0x2B, 0x65, 0x70, 0x03, 0x21, 0x00];
        let start = der.windows(SPKI_PREFIX.len()).position(|w| w == SPKI_PREFIX)? + SPKI_PREFIX.len();
        let bytes = der.get(start..start + 32)?;
        let mut key = [0u8; 32];
        key.copy_from_slice(bytes);
        Some(key)
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
        let mut cert_cursor = std::io::Cursor::new(cert_pem.as_bytes());
        let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_cursor)
            .collect::<Result<_, _>>()
            .map_err(|e| GossipError::InvalidField { field: "gateway_tls", reason: format!("gateway TLS: parse cert PEM: {e}") })?;
        if certs.is_empty() {
            return Err(GossipError::InvalidField { field: "gateway_tls", reason: "gateway TLS: cert PEM contained no certificates".into() });
        }
        let mut key_cursor = std::io::Cursor::new(key_pem.as_bytes());
        let key = rustls_pemfile::private_key(&mut key_cursor)
            .map_err(|e| GossipError::InvalidField { field: "gateway_tls", reason: format!("gateway TLS: parse key PEM: {e}") })?
            .ok_or_else(|| GossipError::InvalidField { field: "gateway_tls", reason: "gateway TLS: key PEM contained no private key".into() })?;
        ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|e| GossipError::InvalidField { field: "gateway_tls", reason: format!("gateway TLS: build server config: {e}") })
    }

    fn build_rustls_configs(
        node_cert_der: CertificateDer<'static>,
        signing_key: &SigningKey,
        ca_cert_der: CertificateDer<'static>,
        removed: Arc<crate::removal::RemovedSet>,
    ) -> Result<(ServerConfig, ClientConfig, ServerConfig), GossipError> {
        use ed25519_dalek::pkcs8::EncodePrivateKey;

        ensure_crypto_provider();

        // Convert signing key to rustls PrivateKeyDer
        let pkcs8 = signing_key
            .to_pkcs8_der()
            .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: encode key for rustls: {e}") })?;
        let key_der: PrivateKeyDer<'static> =
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pkcs8.as_bytes().to_vec()));

        // Build root store from CA cert
        let mut root_store = RootCertStore::empty();
        root_store
            .add(ca_cert_der.clone())
            .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: add CA to root store: {e}") })?;
        let root_store = Arc::new(root_store);

        // Server config: require client cert verified against CA
        let verifier = WebPkiClientVerifier::builder(Arc::clone(&root_store))
            .build()
            .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: build client verifier: {e}") })?;

        // Closure plan C5: a removed member's certificate is refused at the handshake.
        let verifier: Arc<dyn rustls::server::danger::ClientCertVerifier> =
            Arc::new(RemovalAwareClientVerifier { inner: verifier, removed: Arc::clone(&removed) });
        let server_config = ServerConfig::builder()
            .with_client_cert_verifier(verifier)
            .with_single_cert(vec![node_cert_der.clone()], key_der.clone_key())
            .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: build server config: {e}") })?;

        // Gateway server config: SAME node cert/key, but NO client-cert demand, so
        // ordinary HTTP clients can connect (the node-cert-reuse gateway-TLS path).
        let gateway_server_config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![node_cert_der.clone()], key_der.clone_key())
            .map_err(|e| GossipError::InvalidField { field: "gateway_tls", reason: format!("gateway TLS: build server config: {e}") })?;

        // Client config: present node cert, verify server against CA
        let server_verifier = rustls::client::WebPkiServerVerifier::builder(Arc::clone(&root_store))
            .build()
            .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: build server verifier: {e}") })?;
        let client_config = ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(RemovalAwareServerVerifier { inner: server_verifier, removed }))
            .with_client_auth_cert(vec![node_cert_der], key_der)
            .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: build client config: {e}") })?;

        Ok((server_config, client_config, gateway_server_config))
    }

    /// Refuse a certificate whose identity key belongs to a removed member (closure plan C5).
    fn refuse_if_removed(
        removed: &crate::removal::RemovedSet,
        end_entity: &CertificateDer<'_>,
    ) -> Result<(), rustls::Error> {
        if !removed.is_empty()
            && let Some(key) = ed25519_key_from_cert_der(end_entity.as_ref())
            && removed.is_key_removed(&key)
        {
            return Err(rustls::Error::General("mycelium: this identity has been removed from the mesh".into()));
        }
        Ok(())
    }

    /// The CA check, then the removal check, for certificates peers present to this node's server.
    #[derive(Debug)]
    struct RemovalAwareClientVerifier {
        inner: Arc<dyn rustls::server::danger::ClientCertVerifier>,
        removed: Arc<crate::removal::RemovedSet>,
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

    /// The CA check, then the removal check, for certificates the servers this node dials present.
    #[derive(Debug)]
    struct RemovalAwareServerVerifier {
        inner: Arc<rustls::client::WebPkiServerVerifier>,
        removed: Arc<crate::removal::RemovedSet>,
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

    pub(crate) fn pem_cert_to_der(pem: &str) -> Result<CertificateDer<'static>, GossipError> {
        let mut cursor = std::io::Cursor::new(pem.as_bytes());
        let certs: Vec<CertificateDer<'static>> =
            rustls_pemfile::certs(&mut cursor)
                .collect::<Result<_, _>>()
                .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: parse CA cert PEM: {e}") })?;
        certs
            .into_iter()
            .next()
            .ok_or_else(|| GossipError::InvalidField { field: "tls", reason: "TLS: CA cert PEM contains no certificate".into() })
    }

    fn cert_der_to_pem(der: &[u8]) -> String {
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
pub use imp::{ed25519_key_from_cert_der, gateway_server_config_from_pem, generate_rotation, load_or_generate, sign_bytes, verify_bytes};

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
        let cert = generate_node_cert(&node, &signing, &ca).unwrap();
        assert_eq!(ed25519_key_from_cert_der(cert.as_ref()), Some(expected),
            "extracted SPKI key must equal the cert's Ed25519 verifying key");
    }

    #[test]
    fn returns_none_on_absent_or_truncated_pattern_without_panic() {
        assert_eq!(ed25519_key_from_cert_der(&[0u8; 8]), None); // no SPKI prefix
        // Prefix present but fewer than 32 key bytes follow → None, no panic (bounds-checked).
        let truncated = [0x06, 0x03, 0x2B, 0x65, 0x70, 0x03, 0x21, 0x00, 0x01, 0x02];
        assert_eq!(ed25519_key_from_cert_der(&truncated), None);
        assert_eq!(ed25519_key_from_cert_der(&[]), None);
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
}

