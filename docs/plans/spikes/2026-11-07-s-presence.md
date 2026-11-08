# Spike S-presence — 2026-11-07

The spike row of [`identity-and-lifecycle.md`](../identity-and-lifecycle.md) wave 1b, for I7 and decision D7. The
throwaway code is on branch `spike-s-presence` under `spikes/s-presence/`, and no spike ships. The code:

- `probe.swift`, on the Security framework;
- `blob.swift`, on CryptoKit;
- `verify/`, a Rust crate pinned to the lockfile's `ring` 0.17.14, `rcgen` 0.13.2 and `rustls-webpki` 0.103.16, built
  offline;
- `run.sh`, which rebuilds all of it and records every command with its output;
- `keys.sh`, the key-surface counts.

**Every result below is quoted from `run.sh.out`.** That log was made by `run.sh` from commit `13b574ff`'s sources on
a clean tree (the log's first line says so), and it is committed beside them at `0aaa51f6`, with `keys.txt` from
`64fb5096` (its header; the scanned sources there are identical to `main` at `6a64e8bc`).

**This spike's *done when* is not met.** The row asks for "a signature produced only after the prompt and verified".
No prompt was raised, so no such signature exists. See *What it could not do*.

## The machine, as recorded

From `run.sh.out` lines 3–63:

- **`sw_vers`:** macOS 26.6.2, build 25G83.
- **`uname -m`:** arm64.
- **`sysctl -n machdep.cpu.brand_string`:** Apple M2 Max.
- **`security find-identity -v -p codesigning`:** `0 valid identities found`.
- **`ls -la ~/Library/MobileDevice/Provisioning Profiles`:** no such directory.
- **`xcode-select -p`:** `/Library/Developer/CommandLineTools`, the Command Line Tools rather than an Xcode install.
- **`swiftc --version`:** Swift 6.3.3.
- **`codesign -dv` on both probe binaries:** `flags=0x20002(adhoc,linker-signed)`, `Signature=adhoc`,
  `TeamIdentifier=not set`.

Touch ID is enrolled (`bioutil -r`). That was run once by hand and is not in the log.

**What ran is `swiftc` binaries only.** No `cargo build` binary touched the Secure Enclave. The Rust crate only verified
signatures the Swift probes printed. Both toolchains link through Apple's linker, which ad-hoc signs every arm64
binary, so a `cargo build` binary is expected to carry the same `adhoc,linker-signed` signature, and the results are
expected to carry over to a Rust `dev`. That is an expectation. No `cargo build` binary's signature was inspected here,
and "a `cargo build` binary reaches the Enclave" was **not run**.

**No mode the runner calls can raise a prompt.**
- Every signature over a presence-gated key is made under `LAContext.interactionNotAllowed = true`. That includes
  `tamper`, fixed after #729's review (M2).
- The other signatures are over keys with no presence flag.
- `run.sh` never calls `blob sign-interactive`.

## What ran and what it measured

| # | Question | Result, with its `run.sh.out` lines |
|---|---|---|
| 1 | Does an ad-hoc binary reach the Secure Enclave? | **Yes, for a key not stored in the keychain** (`./probe`, :66–80). `SecKeyCreateRandomKey` with `kSecAttrTokenIDSecureEnclave` and `kSecAttrIsPermanent = false` creates a P-256 key. `kSecAttrTokenID` reads back `com.apple.setoken`, and the key signs (DER signature, 72 bytes in this run; ECDSA DER varies 70–72). |
| 2 | The negative probe on an ephemeral presence-gated key | **Signing fails.** The key has `[.privateKeyUsage, .biometryCurrentSet]` and is bound at creation to an `interactionNotAllowed` context. Signing returns `com.apple.LocalAuthentication -1004 "User interaction required."` (:66–80, lines `2.*`). |
| 3 | Storing the key in the keychain, as D7 assumed (data-protection keychain, and the legacy file keychain) | **Refused, both** (lines `3.*`, `4.*`): `OSStatus -34018`, "failed to add key to keychain". That is `errSecMissingEntitlement`. **Inferred, not run:** storing a Secure Enclave key needs a `keychain-access-groups` entitlement, which needs a provisioning profile, which needs an Apple Developer team. Whether a free personal team suffices is not known. Because no key was stored, **the keychain-side negative probe never ran**: `probe.swift`'s lookup-and-sign section had nothing to look up. |
| 4 | Persisting without the keychain: CryptoKit `SecureEnclave.P256.Signing.PrivateKey(accessControl:)` with `[.privateKeyUsage, .biometryCurrentSet]`, its `dataRepresentation` written to a file | **The blob restores in a new process, and signing there still needs presence.** `./blob create` writes a 569-byte blob (:83–87), file mode `-rw-r--r--`, owner `serenity` (:89–91). A later `./blob sign-noninteractive` process restores it and fails `-1004` (:93–98). The error's `UserInfo` carries a `BiometryDatabaseHash` field; reading that as "bound to the enrolled set" is an **inference** from the field's name. A blob with one byte flipped is accepted at construction and refused at signing (`CryptoTokenKit -3`, "unable to sign digest") (:100–103, :113–116). |
| 5 | One P-256-aware verifier | **Yes, with nothing new in the tree** (:119–140). The signatures of both no-presence keys (the probe's key 1, and `./blob nopresence`) verify under `ring::signature::ECDSA_P256_SHA256_ASN1`, on the 65-byte X9.63 point. A flipped message byte is refused. `ring` is already a direct dependency of `mycelium-core` under `tls`. |
| 6 | Can the fleet CA certify a Secure Enclave key? | **Yes** (same lines). `rcgen` 0.13's public `PublicKeyData` trait, implemented over the bare point with `PKCS_ECDSA_P256_SHA256`, lets an **Ed25519** CA issue a client-auth leaf over the Enclave key without its private half. `webpki` verifies the chain Ed25519 CA → P-256 leaf and verifies the Enclave signature against the leaf's key. It refuses that signature presented as Ed25519 (`UnsupportedSignatureAlgorithmForPublicKeyContext`). The repository already does the same at `src/agent/bulk.rs:1395–1405`, where a test's Ed25519 CA issues an operator's ECDSA P-256 leaf. |
| 7 | Would the mesh accept a P-256 member certificate? | **No, read from the code, not run.** `refuse_if_removed` fails closed on any certificate without an Ed25519 key (`mycelium-core/src/tls.rs:1119–1121`: "a mesh certificate must carry an Ed25519 identity key"). The client verifier calls it after the chain check (`:1197`). A P-256 leaf is therefore refused at the handshake. A Secure Enclave key can be a fleet-certified *consent* key, never a mesh identity. |
| 8 | What a second algorithm costs the fleet's verifiers | From `keys.sh` / `keys.txt`, tree `64fb5096`, non-test code in `src/`, `mycelium-core/src/` and `mycelium-*/src/` (each root once; each `file:line` counted once):<br>**66** fn parameters typed `&[u8; 32]` or `&[[u8; 32]]` (keys and hashes together);<br>**33** of those named `key`, `keys`, `verifying_key(s)` or `pub_key_bytes`;<br>**1** fn parameter typed `[u8; 64]`, the Ed25519 signature width;<br>**18** `[u8; 64]` mentions in all;<br>**11** `VerifyingKey::from_bytes` decodes;<br>**11** `Signature::from_bytes` / `from_slice`.<br>The fn patterns match one line at a time, so a parameter list that spans lines is not counted: 66 and 33 undercount by that much. The note's first version said "37", from an uncommitted grep, and its second said 72 and 39, from a `keys.sh` whose roots named `mycelium-core/src` twice. Both are replaced by these. |
| 9 | Can the release pipeline produce a team-signed bundle? | **No, as it stands** (read from `.github/workflows/release.yml`). The `macos` job runs `cargo build --release` and tars the binaries. It has no `codesign`, no notarization, no `.app` wrapper (a provisioning profile is embedded in a bundle, `Contents/embedded.provisionprofile`), and no signing secret. |

## The main input to I7: presence is not attested to the verifier

**A verifier holding only the public key cannot tell what kind of key signed.** It sees the same thing from:

- a `biometryCurrentSet` Enclave key;
- an Enclave key with no presence flag;
- a software P-256 key.

Rows 5 and 6 verify the no-presence keys exactly as a presence key would verify. So an `os_presence(...)` grade on a
record is **self-asserted by whatever enrolled the key**. Two cases make that concrete:

- A same-user process that controls enrolment can enrol a silent key.
- A developer who wants unattended approvals can do the same.

No attestation of a key's access control is available to an unmanaged ad-hoc binary. App Attest needs a team-signed
app and Apple's servers; Managed Device Attestation needs MDM; and neither attests the presence flag. A team-signed `dev` would change **who can use** a
keychain-stored key, restricting it to the team's access group. It would **not**, by itself, let the verifier check
that the key requires presence. The grade would still rest on trusting the enrolling binary.

The blob path adds a second, smaller gap.
- Any process running as the same user can read the blob (mode `-rw-r--r--` as written here) and ask the Enclave for a
  signature.
- It cannot produce one without the user's finger.
- But it chooses the prompt text (`localizedReason`, under its own process name), so the presence proves a person was
  there, not what they were shown.

I7's row has to grade the record by what a verifier can actually check. That makes `os_presence` an enrolment claim,
not a verified property, unless the row adds something that binds the key's ACL to the record. This spike found no such
mechanism.

## What it could not do

**Not met, and needing a person:**
- **The positive path:** a signature produced only after the prompt. `blob.swift`'s `sign-interactive` mode is for the
  maintainer. It makes a fresh `LAContext` per approval. From `spikes/s-presence/`:
  `swiftc -O blob.swift -o blob && ./blob create k.blob && ./blob sign-interactive k.blob > out.txt`, then
  `cargo run --offline --manifest-path verify/Cargo.toml -- out.txt`. `create` and `sign-interactive` are unchanged by
  #729's M2 fix.
- **A fresh `LAContext` per approval:** written into `sign-interactive`, never exercised.
- **`biometryCurrentSet` invalidating the key when a finger is added**, and its passcode leg. The access control has no
  `.devicePasscode` flag, so a typed password should not satisfy it. That is untested.

**Not built:**
- **The Rust↔CryptoKit bridge.** `dataRepresentation` exists only in CryptoKit, which is Swift-only. The public C Security API
  has no equivalent wrapped-key export. A Rust `dev` therefore needs a Swift shim or a helper binary, and that puts Swift
  into `dev`'s build and into the release's `macos` job.
- **D7's "the enrolled public key compared on every use".** No probe compared a stored public key on use.

**Needing a team identity**, none of which this machine has and none of which the spike obtained:
- a keychain-stored key in an access group;
- an `.app` bundle with an embedded profile;
- notarization;
- whether Gatekeeper passes a quarantined (browser-downloaded) ad-hoc tarball.

**Untested or inferred:**
- **Where the blob would live, and its file mode.** Here it was a scratch path at mode 0644 (`-rw-r--r--`, the
  umask's default), which every local user can read. Whether another macOS user's process can make the Enclave sign
  with it is the untested user binding below. I7 should write the blob 0600 in a 0700 directory either way.
- **Device binding** (the blob is useless on another Mac): inferred from Apple's documentation, not tested.
- **User binding:** not tested with a second macOS user.
- **The lifecycle consequence**, inferred from the device binding: a lost blob, a new Mac, or an invalidated enrolment
  loses the key, so re-enrolment is a required flow for I7.

**Not run:**
- **Windows**, whose grade D7 sets at `tty` until shown otherwise.

## What it means for I7

The row is written from this note, not by it.

1. **D7's premise holds only for keychain keys.** D7 says a Secure Enclave key is "reachable only from a team-signed
   bundle with a provisioning profile". That holds for a key **stored in the keychain** (row 3). An ad-hoc `swiftc`
   binary reached the Enclave and its presence gate through a CryptoKit blob (rows 1, 2, 4).
2. **Presence is self-asserted** (the section above), on the blob path and largely on a team-signed path too. That is
   the main thing the row must state and grade.
3. **The signer.** A P-256 key cannot be a mesh identity (row 7). Wiring P-256 through every verifier means the surface
   in row 8. The narrower choice is a fleet-certified P-256 *consent* key read by one verifier where a consent record is
   checked, and `ring` is already in the tree. The spike supports that choice; it does not decide it.
4. **The pipeline question, answered: no.** A team-signed bundle cannot be produced today, and producing one would need
   the maintainer's Apple Developer team. A blob-path `dev` needs no signing change, but it does need the Swift bridge,
   which is not built.
5. **A proposed revision of D7, for the maintainer.** This is a proposal, not a spike result. D7 says that without a
   bundle, I7 falls back to `tty`. One alternative is a blob-path grade such as `os_presence(macos, unbound)`. Given
   point 2, the row should first decide whether any `os_presence` grade is worth having when a verifier cannot check it.
