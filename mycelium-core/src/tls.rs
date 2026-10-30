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
    /// This node's current certificate (DER), published as its certificate record
    /// (`sys/identity-cert/{node}/{key}`, post-360 plan row Q). Rotates with the identity.
    #[cfg(feature = "tls")]
    node_cert: arc_swap::ArcSwap<Vec<u8>>,
    /// The fleet CA's chain check, without the removal check — what a certificate record is judged
    /// by (`verify_member_cert`). Fixed for the node's life: rotation reuses the CA.
    #[cfg(feature = "tls")]
    cert_verifier: std::sync::Arc<dyn rustls::server::danger::ClientCertVerifier>,
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
        self.node_cert.store(std::sync::Arc::new(m.node_cert_der));
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
        let cert = rustls::pki_types::CertificateDer::from(der);
        let now = rustls::pki_types::UnixTime::since_unix_epoch(std::time::Duration::from_millis(
            crate::sim_seam::wall_now_ms(),
        ));
        self.cert_verifier.verify_client_cert(&cert, &[], now).is_ok()
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
    /// The new key's certificate, DER — published as its certificate record before the cutover.
    pub node_cert_der: Vec<u8>,
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

/// The URI a node certificate names its node by: `mycelium://{node}`.
pub fn node_uri(node: &crate::node_id::NodeId) -> String {
    format!("mycelium://{node}")
}

/// Read a certificate's [`CertIdentity`]. The key is read by the X.509 parser rustls uses
/// (`ed25519_key_from_cert_der`); the serial and the subject alternative names by a minimal,
/// bounds-checked DER walk of the `TBSCertificate` (the parser exposes neither). `None` for anything
/// that is not a certificate carrying an Ed25519 key. Says nothing about who issued it: judge the
/// chain with [`NodeTls::verify_member_cert`] first. Never panics.
#[cfg(feature = "tls")]
pub fn cert_identity(der: &[u8]) -> Option<CertIdentity> {
    let key = imp::ed25519_key_from_cert_der(der)?;
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
    let mut node_uris = Vec::new();
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
            if oid != [0x55, 0x1D, 0x11] { continue; } // 2.5.29.17 subjectAltName
            let (mut tag, mut value, mut n2) = der_tlv(e)?;
            if tag == 0x01 {
                // critical BOOLEAN
                e = n2;
                (tag, value, n2) = der_tlv(e)?;
            }
            let _ = n2;
            if tag != 0x04 { return None; }
            let (tag, mut names, _) = der_tlv(value)?;
            if tag != 0x30 { return None; }
            while !names.is_empty() {
                let (tag, name, n3) = der_tlv(names)?;
                names = n3;
                if tag == 0x86 {
                    // [6] IMPLICIT IA5String: uniformResourceIdentifier
                    if let Ok(uri) = std::str::from_utf8(name) { node_uris.push(uri.to_string()); }
                }
            }
        }
    }
    Some(CertIdentity { key, node_uris, issued_ms })
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

    /// A fresh self-signed Ed25519 CA: its certificate and key pair, written nowhere.
    fn generate_ca() -> Result<(CertificateDer<'static>, KeyPair), GossipError> {
        let err = |reason: String| GossipError::InvalidField { field: "tls", reason };
        let ca_key_pair = KeyPair::generate_for(&PKCS_ED25519).map_err(|e| err(format!("TLS: generate CA key: {e}")))?;
        let mut ca_params = CertificateParams::new(vec![]).map_err(|e| err(format!("TLS: CA params: {e}")))?;
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        ca_params.not_after = rcgen::date_time_ymd(2099, 1, 1);
        let ca_cert = ca_params.self_signed(&ca_key_pair).map_err(|e| err(format!("TLS: self-sign CA: {e}")))?;
        Ok((CertificateDer::from(ca_cert.der().to_vec()), ca_key_pair))
    }

    /// Generate a CA and publish it: each file written under a temporary name, then renamed into
    /// place. Called only while holding `ca.lock`.
    fn generate_and_publish_ca(
        dir: &Path,
        cert_path: &Path,
        key_path: &Path,
    ) -> Result<(CertificateDer<'static>, KeyPair), GossipError> {
        let err = |reason: String| GossipError::InvalidField { field: "tls", reason };
        let (ca_cert_der, ca_key_pair) = generate_ca()?;

        let pid = std::process::id();
        let publish = |path: &Path, contents: String, what: &str| -> Result<(), GossipError> {
            let tmp = dir.join(format!(".{what}.tmp-{pid}"));
            let written = if what == "key" { write_private(&tmp, contents.as_bytes()) } else { write_public(&tmp, contents.as_bytes()) };
            written.map_err(|e| err(format!("TLS: write CA {what}: {e}")))?;
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
        standing: Arc<crate::removal::KeyStanding>,
    ) -> Result<NodeTls, GossipError> {
        create_private_dir(&cfg.auto_cert_dir).map_err(|e| {
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
                let node_cert_der = generate_node_cert(node_id, &signing_key, &ca_key_pair, super::issuance_after(0))?;
                (ca_cert_der, node_cert_der)
            }
        };

        // ── 4. Build rustls configs ───────────────────────────────────────
        let cert_bytes = node_cert_der.as_ref().to_vec();
        let (server_config, client_config, gateway_server_config, cert_verifier) =
            build_rustls_configs(node_cert_der, &signing_key, ca_cert_der, removed, standing)?;

        Ok(NodeTls {
            server_config: arc_swap::ArcSwap::from_pointee(server_config),
            client_config: arc_swap::ArcSwap::from_pointee(client_config),
            signing_key: arc_swap::ArcSwap::from_pointee(signing_key),
            gateway_server_config: arc_swap::ArcSwap::from_pointee(gateway_server_config),
            peer_anchor_sink: std::sync::OnceLock::new(),
            node_cert: arc_swap::ArcSwap::from_pointee(cert_bytes),
            cert_verifier,
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
        let signing_key = generate_key()?;
        let verifying_key = signing_key.verifying_key().to_bytes();

        // Persist the new key (raw 32 bytes), same layout as load_or_generate.
        let sanitized = node_id.as_str().replace([':', '.'], "_");
        let auto_key_path = cfg.auto_cert_dir.join(format!("{sanitized}.key"));
        save_key_raw(&signing_key, &auto_key_path)?;

        let (ca_cert_der, ca_key_pair) = load_existing_ca(cfg)?;
        let node_cert_der = generate_node_cert(node_id, &signing_key, &ca_key_pair, super::issuance_after(issued_floor_ms))?;
        let cert_bytes = node_cert_der.as_ref().to_vec();
        let (server_config, client_config, gateway_server_config, _) =
            build_rustls_configs(node_cert_der, &signing_key, ca_cert_der, removed, standing)?;

        Ok(super::RotationMaterial {
            verifying_key,
            server_config: Arc::new(server_config),
            client_config: Arc::new(client_config),
            signing_key: Arc::new(signing_key),
            gateway_server_config: Arc::new(gateway_server_config),
            node_cert_der: cert_bytes,
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
    /// leaves `ca-key.pem` alone: nothing can be issued from it, and the next mint names it and how to recover.
    pub fn mint_ca(ca_dir: &Path) -> Result<std::path::PathBuf, GossipError> {
        let err = |reason: String| GossipError::InvalidField { field: "tls", reason };
        let exists = |p: &Path| err(format!("TLS: {p:?} exists — a CA is minted once; a second one over it is a new fleet"));
        let (cert, key) = (ca_dir.join("ca-cert.pem"), ca_dir.join("ca-key.pem"));
        let refuse_existing = || -> Result<(), GossipError> {
            if let Some(why) = interrupted_mint(ca_dir) {
                return Err(err(why));
            }
            for p in [&cert, &key] {
                if p.exists() {
                    return Err(exists(p));
                }
            }
            Ok(())
        };
        refuse_existing()?;
        create_private_dir(ca_dir).map_err(|e| err(format!("TLS: create {ca_dir:?}: {e}")))?;
        let lock_path = ca_dir.join("ca.lock");
        match fs::OpenOptions::new().write(true).create_new(true).open(&lock_path) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(err(format!(
                    "TLS: another process holds {lock_path:?} — a node or a mint is creating a CA here; if none is, one \
                     crashed while creating it: remove the lock (and a ca-key.pem left without ca-cert.pem) and mint again"
                )));
            }
            Err(e) => return Err(err(format!("TLS: cannot take CA lock {lock_path:?}: {e}"))),
        }
        let held = (|| {
            refuse_existing()?;
            publish_minted_ca(ca_dir, &cert, &key, &exists)
        })();
        let _ = fs::remove_file(&lock_path);
        held?;
        tracing::info!("TLS: minted a fleet CA in {ca_dir:?} — distribute ca-cert.pem, never ca-key.pem");
        Ok(cert)
    }

    /// `ca_dir` holds `ca-key.pem` without `ca-cert.pem`: the state a mint leaves when it stops between its two links
    /// (it publishes the key first). Named, with the recovery, by `tls ca` and `tls issue`. A certificate without a key
    /// is not this state — a node's directory holds exactly that — so it reads as an existing CA.
    fn interrupted_mint(ca_dir: &Path) -> Option<String> {
        let (cert, key) = (ca_dir.join("ca-cert.pem"), ca_dir.join("ca-key.pem"));
        (key.exists() && !cert.exists()).then(|| format!(
            "TLS: {ca_dir:?} holds ca-key.pem without ca-cert.pem — a CA mint was interrupted between publishing its key \
             and its certificate, so nothing can have been issued from it: remove {key:?} (and ca.lock, if present) and mint again"
        ))
    }

    /// Publish a freshly generated CA in `ca_dir`, while holding `ca.lock`.
    fn publish_minted_ca(ca_dir: &Path, cert: &Path, key: &Path, exists: &dyn Fn(&Path) -> GossipError) -> Result<(), GossipError> {
        let err = |reason: String| GossipError::InvalidField { field: "tls", reason };
        let no_links = |what: &str, e: std::io::Error| err(format!(
            "TLS: publish CA {what} by hard link in {ca_dir:?}: {e} — `tls ca` needs a filesystem with hard links (FAT, exFAT \
             and some SMB mounts have none): mint on a local disk and copy the directory"
        ));
        let (ca_cert_der, ca_key_pair) = generate_ca()?;
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
        let (_ca_cert_der, ca_key_pair) = load_existing_ca(&ca_cfg)?;
        let signing_key = generate_key()?;
        let node_cert_der = generate_node_cert(node_id, &signing_key, &ca_key_pair, super::issuance_after(0))?;
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

    /// The CA **certificate** alone (`ca_cert_pem`, else `{auto_cert_dir}/ca-cert.pem`): load-only,
    /// an error naming the path when absent — never a mint. The path a pre-issued node cert takes.
    fn load_ca_cert_only(cfg: &TlsConfig) -> Result<CertificateDer<'static>, GossipError> {
        let ca_cert_path = cfg.ca_cert_pem.clone().unwrap_or(cfg.auto_cert_dir.join("ca-cert.pem"));
        let pem = fs::read_to_string(&ca_cert_path).map_err(|e| GossipError::InvalidField {
            field: "tls", reason: format!("TLS: a pre-issued node cert needs the CA cert ({ca_cert_path:?}): {e}"),
        })?;
        pem_cert_to_der(&pem)
    }

    /// Load the existing cluster CA cert + key (load-only; errors if absent —
    /// unlike `load_or_generate`, rotation must never mint a new CA).
    fn load_existing_ca(cfg: &TlsConfig) -> Result<(CertificateDer<'static>, KeyPair), GossipError> {
        let auto_ca_cert_path = cfg.auto_cert_dir.join("ca-cert.pem");
        let auto_ca_key_path  = cfg.auto_cert_dir.join("ca-key.pem");
        let ca_cert_path = cfg.ca_cert_pem.clone().unwrap_or(auto_ca_cert_path);
        if cfg.ca_cert_pem.is_none()
            && let Some(why) = interrupted_mint(&cfg.auto_cert_dir)
        {
            return Err(GossipError::InvalidField { field: "tls", reason: why });
        }
        let pem = fs::read_to_string(&ca_cert_path).map_err(|e| GossipError::InvalidField {
            field: "tls", reason: format!("TLS: needs an existing CA cert ({ca_cert_path:?}) — never minted here: {e}"),
        })?;
        let ca_cert_der = pem_cert_to_der(&pem)?;
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
    /// parents are created as the umask leaves them. A directory that already exists is the operator's and is left as
    /// it is. Elsewhere than unix a plain create.
    fn create_private_dir(dir: &Path) -> std::io::Result<()> {
        if dir.is_dir() {
            return Ok(());
        }
        if let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
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
        issued_ms: u64,
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
        // Post-360 plan row Q: the node's NodeId as a URI, so a certificate names *its node* and not
        // just a host (nodes on one host share the IP), and the issuance time in the serial — the
        // order a CA re-issue outranks earlier keys by. `notBefore` keeps its fixed date: dating it
        // now would make a peer whose clock is behind refuse the handshake.
        let uri = rcgen::Ia5String::try_from(super::node_uri(node_id))
            .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: node URI: {e}") })?;
        params.subject_alt_names = vec![SanType::IpAddress(ip), SanType::URI(uri)];
        params.serial_number = Some(rcgen::SerialNumber::from(issued_ms));
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

    pub(super) fn build_rustls_configs(
        node_cert_der: CertificateDer<'static>,
        signing_key: &SigningKey,
        ca_cert_der: CertificateDer<'static>,
        removed: Arc<crate::removal::RemovedSet>,
        standing: Arc<crate::removal::KeyStanding>,
    ) -> Result<(ServerConfig, ClientConfig, ServerConfig, Arc<dyn rustls::server::danger::ClientCertVerifier>), GossipError> {
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
        let mut client_config = ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(RemovalAwareServerVerifier { inner: server_verifier, removed, standing }))
            .with_client_auth_cert(vec![node_cert_der], key_der)
            .map_err(|e| GossipError::InvalidField { field: "tls", reason: format!("TLS: build client config: {e}") })?;
        client_config.resumption = rustls::client::Resumption::disabled();

        Ok((server_config, client_config, gateway_server_config, chain_verifier))
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
pub use imp::{ed25519_key_from_cert_der, gateway_server_config_from_pem, generate_rotation, generate_rotation_after, issue_node_cert, load_or_generate, mint_ca, sign_bytes, verify_bytes};

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
        let again = super::imp::mint_ca(&dir).expect_err("a half-published CA").to_string();
        assert!(again.contains("interrupted") && again.contains("remove"), "{again}");
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
        load_or_create_ca(&TlsConfig { auto_cert_dir: own.clone(), ..TlsConfig::default() }).ok();
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
        let (_, thief, _, _) = super::imp::build_rustls_configs(legacy, &k0, ca_der, Arc::new(RemovedSet::default()), Default::default()).unwrap();
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
}
