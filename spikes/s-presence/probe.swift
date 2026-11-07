// Spike S-presence probe (throwaway). Never triggers a user prompt: every signing call that
// would need presence runs with interactionNotAllowed on its LAContext.
import Foundation
import Security
import LocalAuthentication

func out(_ k: String, _ v: Any) { print("\(k)\t\(v)") }
func errStr(_ e: Unmanaged<CFError>?) -> String {
    guard let e = e?.takeRetainedValue() else { return "nil" }
    let ns = e as Error as NSError
    return "\(ns.domain) \(ns.code) \(ns.localizedDescription)"
}

func makeKey(flags: SecAccessControlCreateFlags, permanent: Bool, dataProtection: Bool, tag: String, ctx: LAContext? = nil) -> (SecKey?, String) {
    var e: Unmanaged<CFError>?
    guard let acl = SecAccessControlCreateWithFlags(nil, kSecAttrAccessibleWhenUnlockedThisDeviceOnly, flags, &e) else {
        return (nil, "acl: " + errStr(e))
    }
    var priv: [String: Any] = [kSecAttrIsPermanent as String: permanent, kSecAttrAccessControl as String: acl]
    if permanent { priv[kSecAttrApplicationTag as String] = tag.data(using: .utf8)! }
    if let ctx = ctx { priv[kSecUseAuthenticationContext as String] = ctx }
    var attrs: [String: Any] = [
        kSecAttrKeyType as String: kSecAttrKeyTypeECSECPrimeRandom,
        kSecAttrKeySizeInBits as String: 256,
        kSecAttrTokenID as String: kSecAttrTokenIDSecureEnclave,
        kSecPrivateKeyAttrs as String: priv,
    ]
    if dataProtection { attrs[kSecUseDataProtectionKeychain as String] = true }
    var e2: Unmanaged<CFError>?
    let k = SecKeyCreateRandomKey(attrs as CFDictionary, &e2)
    return (k, k == nil ? errStr(e2) : "ok")
}

func tokenID(_ k: SecKey) -> String {
    let a = SecKeyCopyAttributes(k) as? [String: Any] ?? [:]
    return (a[kSecAttrTokenID as String] as? String) ?? "absent"
}

let msg = "mycelium spike s-presence".data(using: .utf8)!
let alg = SecKeyAlgorithm.ecdsaSignatureMessageX962SHA256

// 1. Ephemeral SE key, no presence flag: does a plain/ad-hoc binary reach the Secure Enclave at all?
let (k1, r1) = makeKey(flags: [.privateKeyUsage], permanent: false, dataProtection: false, tag: "")
out("1.create_ephemeral_no_presence", r1)
if let k1 = k1 {
    out("1.token_id", tokenID(k1))
    out("1.token_is_secure_enclave", tokenID(k1) == (kSecAttrTokenIDSecureEnclave as String))
    var e: Unmanaged<CFError>?
    if let sig = SecKeyCreateSignature(k1, alg, msg as CFData, &e) as Data? {
        let pub = SecKeyCopyPublicKey(k1)!
        var e3: Unmanaged<CFError>?
        let raw = SecKeyCopyExternalRepresentation(pub, &e3)! as Data   // 65-byte X9.63 uncompressed point
        out("1.sign", "ok len=\(sig.count)")
        out("1.pub_x963_hex", raw.map { String(format: "%02x", $0) }.joined())
        out("1.sig_der_hex", sig.map { String(format: "%02x", $0) }.joined())
        out("1.msg_hex", msg.map { String(format: "%02x", $0) }.joined())
    } else { out("1.sign", errStr(e)) }
}

// 2. Ephemeral SE key gated on biometryCurrentSet, bound at creation to a context whose
//    interactionNotAllowed is true. The negative probe: signing must FAIL, and no prompt is shown.
let ctx2 = LAContext(); ctx2.interactionNotAllowed = true
let (k2, r2) = makeKey(flags: [.privateKeyUsage, .biometryCurrentSet], permanent: false, dataProtection: false, tag: "", ctx: ctx2)
out("2.create_ephemeral_biometryCurrentSet", r2)
if let k2 = k2 {
    out("2.token_id", tokenID(k2))
    var e: Unmanaged<CFError>?
    let sig = SecKeyCreateSignature(k2, alg, msg as CFData, &e)
    out("2.sign_noninteractive", sig == nil ? "FAILED (expected): " + errStr(e) : "SIGNED (defect: presence not required)")
}
out("2.canEvaluate_biometry", LAContext().canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: nil))

// 3. Permanent SE key in the data-protection keychain: what a fleet-enrolled identity needs.
let tag = "dev.mycelium.spike.s-presence.\(UUID().uuidString)"
let (k3, r3) = makeKey(flags: [.privateKeyUsage, .biometryCurrentSet], permanent: true, dataProtection: true, tag: tag)
out("3.create_permanent_dataprotection_biometryCurrentSet", r3)

// 4. Permanent SE key in the file-based (legacy) keychain, no data-protection flag.
let tag4 = "dev.mycelium.spike.s-presence.legacy.\(UUID().uuidString)"
let (k4, r4) = makeKey(flags: [.privateKeyUsage, .biometryCurrentSet], permanent: true, dataProtection: false, tag: tag4)
out("4.create_permanent_legacy_biometryCurrentSet", r4)
for (k, t) in [(k3, tag), (k4, tag4)] {
    guard k != nil else { continue }
    // Negative probe on a stored key: look it up with a non-interactive context, then sign.
    let ctx = LAContext(); ctx.interactionNotAllowed = true
    var q: [String: Any] = [
        kSecClass as String: kSecClassKey,
        kSecAttrApplicationTag as String: t.data(using: .utf8)!,
        kSecAttrKeyType as String: kSecAttrKeyTypeECSECPrimeRandom,
        kSecReturnRef as String: true,
        kSecUseAuthenticationContext as String: ctx,
    ]
    if t == tag { q[kSecUseDataProtectionKeychain as String] = true }
    var item: CFTypeRef?
    let st = SecItemCopyMatching(q as CFDictionary, &item)
    out("probe[\(t == tag ? 3 : 4)].lookup", st)
    if st == errSecSuccess {
        let key = item as! SecKey
        var e: Unmanaged<CFError>?
        let sig = SecKeyCreateSignature(key, alg, msg as CFData, &e)
        out("probe[\(t == tag ? 3 : 4)].sign_noninteractive", sig == nil ? "FAILED (expected): " + errStr(e) : "SIGNED (defect: no prompt needed)")
    }
    // Clean up.
    var dq: [String: Any] = [kSecClass as String: kSecClassKey, kSecAttrApplicationTag as String: t.data(using: .utf8)!]
    if t == tag { dq[kSecUseDataProtectionKeychain as String] = true }
    out("cleanup", SecItemDelete(dq as CFDictionary))
}
