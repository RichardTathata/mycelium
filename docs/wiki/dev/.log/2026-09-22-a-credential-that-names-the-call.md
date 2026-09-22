## [2026-09-22] ingest | a credential that named who and what, but not which — federation's confused deputy

Up: [dev](../dev.md) · record `docs/design/federated-domains.md` (row 11) · code
`src/federation/call.rs`, `src/federation/edge.rs`, `src/agent/a2a.rs` · PR #357.

**The hole.** A federated credential's signature covered the tag, the origin domain, the principal,
the export and the validity window. **Nothing about the payload.**

So an attacker on the path between two domains could rewrite a call's body, leave the credential
header untouched, and the receiving gateway would accept the altered call as authentic — then run
the AE preflight and write an evidence record about *the attacker's text*. The credential said
*this principal may call this export* and remained true the whole time, while the call became a
different call. A confused deputy with a valid badge.

Found by reading row 11's own sentence — *"a hostile network between domains"* — and asking what,
precisely, a hostile network could do. The answer was two things, and the worse one needed no TLS.

### The distinction worth keeping

| | Closes | Needs |
|---|---|---|
| **binding the body into the credential** | silent **modification** | nothing new — the pattern already existed one layer up |
| **TLS on the edge** | **eavesdropping** | a trust anchor that does not exist yet |

Tampering beats reading. The binding went first for that reason, and because it needs no PKI
decision: `ActionEnvelope::arguments_digest` already binds arguments to a *decision*, and this binds
the body to the *caller*. The shape was in the codebase; it had simply not crossed the domain edge.

### Three things that make a binding hold rather than merely exist

- **Digest the bytes as received.** The client serialises once and signs those bytes; `/a2a` now
  takes `Bytes` and parses afterwards. A digest over a re-serialisation compares our encoder against
  theirs — false refusals for honest partners, and a pass for an attacker who matched our encoder.
  The same rule the evidence exporter already followed: *verify the received bytes, then parse.*
- **Make stripping a forgery.** Whether a credential binds is inside the signed bytes, so removing
  it fails as `BadSignature`. Without that, the protection is opt-out by anyone on the path.
- **Say what the compatibility default costs.** `require_body_binding` is `false` so a partner that
  has not upgraded keeps working — and the type says, in those words, that `false` gives **no
  integrity guarantee against an active attacker**, because absence is indistinguishable from
  tampering. A rolling-upgrade window is a fine thing to have and a terrible thing to leave unsaid.

Two refusals, kept apart on purpose: `BodyNotBound` sends an operator to upgrade a partner,
`BodyMismatch` sends them to look for someone on the path. Collapsing them sends them to the wrong
place on the worse day.

### An overclaim of mine, caught by a plant

I wrote that the presence byte in the canonical form stops an attacker stripping `Some([0; 32])`
down to `None`. It does not — the digest is fixed-length and last, so stripping changes the byte
length and the signature fails either way. **A plant that deleted the byte broke no test**, which is
how the note was found to be wrong.

The byte is kept, because that whole argument rests on *being last*, and one byte now is cheaper
than noticing the day a field is appended. But the comment and the test say that now, instead of
claiming a property they did not have. Two of three plants caught; the third is recorded as not
load-bearing rather than quietly dropped.

### What is still open, stated so the record is not read as more than it is

Confidentiality. An on-path observer still reads every federated call. TLS on the edge needs a
**trust anchor that does not exist**: partner trust here is an Ed25519 key in the `TrustBundle`,
with no X.509 material anywhere. Public PKI, pinning the partner's certificate in the bundle beside
its key, or exchanging each domain's CA — a decision, not an implementation, and unmade.

### The sentence to keep

**Authenticating the caller is not authenticating the call.** A signature proves what it covers and
nothing adjacent, and the gap between *who is asking* and *what they asked* is exactly where a
deputy gets confused.
