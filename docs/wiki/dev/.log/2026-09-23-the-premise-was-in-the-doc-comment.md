## [2026-09-23] ingest | the premise was in the doc comment — TLS on the federation edge, anchored on a pin

Up: [dev](../dev.md) · pages touched: [security](../security.md) (federated-domains section) ·
record `docs/design/federated-domains.md` (row 11) · code `src/federation/pinning.rs`,
`src/federation.rs`, `src/federation/client.rs` · guide 17, `docs/operations/federation.md`.

v2.12.0 stopped an on-path attacker altering a federated call and said plainly what it had not done:
*"integrity, not confidentiality — an on-path observer still reads every federated call. TLS on the
edge needs a trust anchor that does not exist."* This closes that, and the interesting part is not
the TLS.

## The anchor was already the answer

Partner trust here is one Ed25519 key per partner, chosen bilaterally by an operator — no registry,
no trust-registry service (D25). There is no X.509 material anywhere in it. So "add TLS" is not a
configuration task; it is a question about what to trust, with three answers:

| Anchor | Cost |
|---|---|
| The public Web PKI | Installs a trust root **larger than the relationship** underneath a design whose whole point is that the relationship is bilateral. Anyone a public CA will issue for becomes a possible endpoint. Also demands a public name and a renewed public certificate from every partner gateway. |
| Exchange private CAs | Works. A CA per domain, a second rotation story, and a document whose compromise is total, because a CA signs **any** name. What it buys is delegation — the partner re-issues endpoint certs freely — which is the one property a two-party link does not need. |
| Pin the key in the bundle | The anchor is the decision the operator already made. Nothing new to distribute; a compromise costs one endpoint, not a namespace. |

The third, and not as a shortcut: it is the module's own existing rule, applied one layer down.
`verify_descriptor`'s doc already says **the bundle decides which key, never the document** — a
descriptor carrying its own `public_key` is not authorised by being internally consistent. A
certificate vouching for itself is the same claim in a different encoding.

## The part worth keeping

`mycelium_core::tls::ed25519_key_from_cert_der` already extracts an Ed25519 public key from a
certificate. Reusing it was the obvious move, and it would have been the vulnerability.

It works by **scanning the DER for the Ed25519 SPKI byte prefix** and taking the 32 bytes after it.
Its doc states the safety argument in full, and the argument is correct:

> The trust anchor for peer identity is the **CA-signed cert**: rustls validates the peer's cert
> against the cluster CA during the handshake, so by the time this runs the DER is a well-formed,
> CA-issued Ed25519 cert produced by `generate_node_cert`. That makes a targeted, length-checked
> scan […] both safe (the input is validated) and dependency-free.

Every clause of that is true where it is used. **None of it is true at a pinning verifier**, which
runs *before* any validation, on bytes an attacker chose, with no CA in the picture at all. The scan
finds the first occurrence of the prefix anywhere in the encoding — and a certificate has several
attacker-controlled fields that *precede* the real `SubjectPublicKeyInfo`. So an attacker can carry
the victim's public key as **data** inside a certificate signed by their own key, and a pin computed
by scanning would match a key that is not signing the handshake.

That is a test, not a worry. `a_planted_key_pattern_fools_the_byte_scan_and_not_the_structural_read`
puts the victim's SPKI prefix and key into the **serial number** of an attacker-signed certificate,
and asserts both halves: the scan really does return the victim's key (or the plant proves nothing),
and the structural read returns the attacker's, so the pin does not match. The SPKI is read from its
position in the structure by the X.509 parser rustls already uses — the same code that validates
chains, so there is no second parser at a trust edge.

The lesson generalises past this function: **a function's safety argument is part of its signature,
and it is not in the signature.** It lives in prose, it does not travel with the call, and the
compiler will not mention it. Reuse across a trust boundary means re-reading the argument, not the
code. The direction matters too — the same module *does* hand-write the RFC 8410 SPKI encoding in
`ed25519_spki_sha256`, because that one **writes** bytes for a key already trusted, which is the
safe direction, and it is gated by a test asserting it agrees with the structural read.

## Two smaller things the build turned up

**A refusal must not borrow a worse refusal's meaning.** The first implementation let a pin failure
fall through the silent-gateway path, which reports `DeliveryUnknown`. That is not merely imprecise:
`DeliveryUnknown` means *it may have run*, which permanently bars an at-most-once caller from
retrying. A failed handshake delivered nothing. `ClientError::Tls` exists for that distinction, and
the end-to-end test asserts the variant is *not* `DeliveryUnknown` as well as asserting what it is.

**Recovering a typed error through three libraries is not a `source()` walk.** The refusal is raised
inside rustls and arrives through hyper and reqwest. The chain is
`reqwest::Error → hyper_util → io::Error(Custom) → io::Error(Custom) → rustls::Error`, and
`source()` **stops at the first `io::Error`** — a custom payload is reachable only through
`get_ref()`, and here it is nested twice. The first version walked `source()` alone, found nothing,
and reported the mismatch as a silent gateway; the end-to-end test failed with exactly that, which
is how it was found. String-matching the message would have "worked" and would have been a gate that
quietly stops working the next time any layer rewords itself.

**A setting that lives only in the built object is a setting the next builder call drops.**
`with_timeouts` and `with_tls_pins` both rebuild the HTTP client. Each building from its own
arguments makes the result depend on call order — and the order that loses is the one that silently
turns pinning off. Both now write to fields and call one `rebuild_http`, and the gate is a test that
runs both orders, because an order-dependence bug is invisible in whichever order the author happened
to write first.

## What is not claimed

Confidentiality and endpoint authentication, not caller authentication — that stays the credential,
deliberately, because a second authentication model at one edge is the drift v2.4.1 and v2.4.2 were
spent removing. The verifier checks no name, no chain and no expiry: there is no authority here to
check them against, and a name checked against nothing reads like validation while proving nothing.
Nothing here helps against an attacker holding the partner's private key.
