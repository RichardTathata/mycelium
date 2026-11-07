# Spike S-presence — 2026-11-07

The spike row of [`identity-and-lifecycle.md`](../identity-and-lifecycle.md) wave 1b, for I7 and decision D7. The
throwaway code and its raw outputs are on branch `spike-s-presence` (commit `42ed4197`, `spikes/s-presence/`): two
Swift probes (`probe.swift` on the Security framework, `blob.swift` on CryptoKit) and a Rust verifier crate pinned to
the lockfile's `ring` 0.17.14, `rcgen` 0.13.2 and `rustls-webpki` 0.103.16, built offline. No spike ships.

**Machine:** macOS 26.6.2 (25G83), Apple M2 Max, Touch ID enrolled. `security find-identity -v -p codesigning` reports
**0 valid identities** and no provisioning profile is installed, so every probe ran **ad-hoc signed**
(`flags=0x20002(adhoc,linker-signed)`). That is the signature `cargo build` puts on every Apple-silicon binary,
including what `release.yml`'s `macos` job ships. **No Touch ID prompt was shown:** every signing call that needed
presence ran with `LAContext.interactionNotAllowed = true`.

## What ran and what it measured

| # | Question | Result |
|---|---|---|
| 1 | Does a plain (ad-hoc) binary reach the Secure Enclave? | **Yes, for a key not stored in the keychain.** `SecKeyCreateRandomKey` with `kSecAttrTokenIDSecureEnclave` and `kSecAttrIsPermanent = false` creates a P-256 key. Its `kSecAttrTokenID` reads back `com.apple.setoken` (`kSecAttrTokenIDSecureEnclave`), and it signs (71-byte DER). |
| 2 | The negative probe: does a presence-gated key refuse to sign without interaction? | **Yes.** An ephemeral key with `[.privateKeyUsage, .biometryCurrentSet]`, bound at creation to a context with `interactionNotAllowed`, fails `SecKeyCreateSignature` with `com.apple.LocalAuthentication -1004 "User interaction required."`. |
| 3 | A key stored in the keychain, which D7 assumed: data-protection keychain, or the legacy file keychain | **Refused, both:** `OSStatus -34018` (`errSecMissingEntitlement`, "failed to add key to keychain"). Storing a Secure Enclave key needs a `keychain-access-groups` entitlement, which needs a provisioning profile, which needs an Apple Developer team. |
| 4 | Persisting without the keychain: CryptoKit `SecureEnclave.P256.Signing.PrivateKey(accessControl:)` with `[.privateKeyUsage, .biometryCurrentSet]`, its `dataRepresentation` written to a file | **Works ad-hoc.** The 569-byte blob is the key wrapped by this device's Secure Enclave. It restores **in a new process**, and signing there with an `interactionNotAllowed` context fails with `-1004` (the error carries `BiometryDatabaseHash`: the key is bound to the enrolled set). A blob with one byte flipped is accepted at construction, but signing with it fails (`CryptoTokenKit -3`, "unable to sign digest"). |
| 5 | One P-256-aware verifier | **Yes, with nothing new in the tree.** Both probes' signatures (non-presence keys) verify under `ring::signature::ECDSA_P256_SHA256_ASN1` on the 65-byte X9.63 point. `ring` is already a direct dependency of `mycelium-core` under `tls`. A flipped message byte is refused. |
| 6 | Can the fleet CA certify a Secure Enclave key? | **Yes.** `rcgen` 0.13's public `PublicKeyData` trait, implemented over the bare point with `PKCS_ECDSA_P256_SHA256`, lets an **Ed25519** CA (made as `mycelium tls` makes one) issue a client-auth leaf over the Secure Enclave key, with no private half. `webpki` verifies the chain Ed25519 CA → P-256 leaf, verifies the Secure Enclave signature against the leaf's key, and refuses it presented as Ed25519 (`UnsupportedSignatureAlgorithmForPublicKeyContext`). |
| 7 | What a second algorithm costs the fleet's verifiers | Measured by grep: **37** non-test functions in `src/`, `mycelium-core/src/` and `mycelium-*/src/` take a verifying key as `&[u8; 32]` or `&[[u8; 32]]`. That covers audit, consensus, revocation, `NodeTls::verify_bytes`, knowledge, federation, agentfacts and commitment. `ed25519_key_from_cert_der` (`mycelium-core/src/tls.rs:960`) returns `None` for a P-256 leaf. `verify_member_cert`'s chain check is a `WebPkiClientVerifier` built on `ring`'s default algorithms, which include ECDSA P-256, so the chain alone would likely pass; the removal and standing wrapper in front of it (`RemovalAwareClientVerifier`) was not run against a P-256 leaf. A P-256 member certificate would at best have chain standing and no key in the fleet's model. |
| 8 | Can the release pipeline produce a team-signed bundle? | **No, as it stands.** `release.yml`'s `macos` job runs `cargo build --release` and tars the binaries. It has no `codesign`, no notarization, no `.app` wrapper (a provisioning profile is embedded in a bundle, `Contents/embedded.provisionprofile`), and no signing secret. Adding one needs an Apple Developer Program membership, a signing certificate and profile with the entitlement, and those as CI secrets. |

## What it could not do

- **The positive path:** "a signature produced only after the prompt and verified", the row's *done when*. A prompt
  needs a person at the sensor, and a spike agent must not raise one. `blob.swift` has a `sign-interactive` mode for
  the maintainer: `swiftc -O blob.swift -o blob && ./blob create k.blob && ./blob sign-interactive k.blob > out.txt`,
  then `cargo run --offline --manifest-path verify/Cargo.toml -- out.txt` (from `spikes/s-presence/` on the spike
  branch). It uses a fresh `LAContext` per approval.
- **Anything needing a team identity:** a keychain-stored key bound to an access group, an `.app` bundle with an
  embedded profile, notarization, and whether Gatekeeper passes a quarantined (browser-downloaded) ad-hoc tarball.
  This machine has none of those, and the spike did not obtain any.
- **`biometryCurrentSet` re-enrolment:** that adding a finger invalidates the key. It needs a person changing the
  enrolled set.
- **Windows**, whose grade D7 sets at `tty` until shown otherwise. Not run.

## What it means for I7 (the row is written from this, not by it)

1. **The premise changes.** D7 says a Secure Enclave key is "reachable only from a team-signed bundle with a
   provisioning profile". That holds for a key **stored in the keychain**. It does not hold for a key whose
   Enclave-wrapped blob is kept in a file: an ad-hoc `cargo build` binary creates one, restores it in a later process,
   and the Enclave still demands presence to sign.
2. **The blob path is presence-bound but not process-bound.** Any process running as the same user can read the blob
   and ask the Enclave for a signature. It cannot produce one without the user's finger, but it chooses what the prompt
   says (`localizedReason`, under its own process name). A keychain key in a team access group is usable only by the
   team-signed app. So on the blob path, D7's rule ("a check no co-resident process can satisfy") holds for the
   *presence* and not for *what was presented*. The row must grade the record accordingly, for example
   `os_presence(macos, unbound)` against `os_presence(macos)` once a team-signed `dev` exists.
3. **The signer question.** Wiring a P-256 fleet certificate through the verifiers means changing the 37 `[u8; 32]`
   sites and `ed25519_key_from_cert_der`. The narrower choice is a P-256 key the fleet CA certifies (finding 6), read
   by **one** P-256 verifier at the one place a consent record is checked. `ring`, already in the tree, is that
   verifier. The spike supports the narrower choice. It does not decide it.
4. **The pipeline question, answered:** **no**, a team-signed bundle cannot be produced today, and it would need the
   maintainer's Apple Developer team. **Yes**, an unbound grade can ship with no pipeline change. So I7 does not have to
   fall back to `tty` on macOS. It can grade `os_presence(macos, unbound)` now and lift to `os_presence(macos)` when a
   team identity exists.
