// Spike S-presence, probe 2 (throwaway): a CryptoKit Secure Enclave key persisted as its wrapped
// blob in a file — no keychain, so no keychain-access-group entitlement. Never prompts: every
// signing context has interactionNotAllowed = true.
import Foundation
import CryptoKit
import LocalAuthentication
import Security

func out(_ k: String, _ v: Any) { print("\(k)\t\(v)") }
let args = CommandLine.arguments
let path = args.count > 2 ? args[2] : "/tmp/none"
let msg = "mycelium spike s-presence".data(using: .utf8)!

switch args.count > 1 ? args[1] : "" {
case "create":
    out("se.isAvailable", SecureEnclave.isAvailable)
    var e: Unmanaged<CFError>?
    let acl = SecAccessControlCreateWithFlags(nil, kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
                                              [.privateKeyUsage, .biometryCurrentSet], &e)!
    do {
        let k = try SecureEnclave.P256.Signing.PrivateKey(accessControl: acl)
        try k.dataRepresentation.write(to: URL(fileURLWithPath: path))
        out("create", "ok blob_len=\(k.dataRepresentation.count)")
        out("pub_x963_hex", k.publicKey.x963Representation.map { String(format: "%02x", $0) }.joined())
    } catch { out("create", "error: \(error)") }
case "sign-noninteractive":
    do {
        let blob = try Data(contentsOf: URL(fileURLWithPath: path))
        let ctx = LAContext(); ctx.interactionNotAllowed = true
        let k = try SecureEnclave.P256.Signing.PrivateKey(dataRepresentation: blob, authenticationContext: ctx)
        out("restore_in_new_process", "ok")
        do {
            let sig = try k.signature(for: msg)
            out("sign_noninteractive", "SIGNED (defect: presence not required) \(sig.derRepresentation.count)")
        } catch { out("sign_noninteractive", "FAILED (expected): \(error)") }
    } catch { out("restore", "error: \(error)") }
case "sign-interactive":
    // FOR THE MAINTAINER ONLY: shows the Touch ID prompt. A fresh LAContext per approval.
    do {
        let blob = try Data(contentsOf: URL(fileURLWithPath: path))
        let ctx = LAContext()
        ctx.localizedReason = "approve spike S-presence test record"
        let k = try SecureEnclave.P256.Signing.PrivateKey(dataRepresentation: blob, authenticationContext: ctx)
        let sig = try k.signature(for: msg)
        out("sign_interactive", "ok")
        out("pub_x963_hex", k.publicKey.x963Representation.map { String(format: "%02x", $0) }.joined())
        out("sig_der_hex", sig.derRepresentation.map { String(format: "%02x", $0) }.joined())
        out("msg_hex", msg.map { String(format: "%02x", $0) }.joined())
    } catch { out("sign_interactive", "error: \(error)") }
case "tamper":
    do {
        var blob = try Data(contentsOf: URL(fileURLWithPath: path))
        blob[blob.count / 2] ^= 0xff
        let k = try SecureEnclave.P256.Signing.PrivateKey(dataRepresentation: blob)
        out("tampered_blob_construct", "accepted (validation is deferred)")
        do { _ = try k.signature(for: msg); out("tampered_blob_sign", "SIGNED (defect)") }
        catch { out("tampered_blob_sign", "refused: \(error)") }
    } catch { out("tampered_blob", "refused: \(error)") }
case "nopresence":
    // A key with no presence flag, persisted and restored: shows the blob round-trip signs and what
    // the verifier sees, without any prompt.
    do {
        let k = try SecureEnclave.P256.Signing.PrivateKey()
        try k.dataRepresentation.write(to: URL(fileURLWithPath: path))
        let k2 = try SecureEnclave.P256.Signing.PrivateKey(dataRepresentation: try Data(contentsOf: URL(fileURLWithPath: path)))
        let sig = try k2.signature(for: msg)
        out("roundtrip_sign", "ok")
        out("pub_x963_hex", k2.publicKey.x963Representation.map { String(format: "%02x", $0) }.joined())
        out("pub_spki_der_hex", k2.publicKey.derRepresentation.map { String(format: "%02x", $0) }.joined())
        out("sig_der_hex", sig.derRepresentation.map { String(format: "%02x", $0) }.joined())
        out("msg_hex", msg.map { String(format: "%02x", $0) }.joined())
    } catch { out("nopresence", "error: \(error)") }
default: out("usage", "create|sign-noninteractive|tamper|nopresence <path>")
}
