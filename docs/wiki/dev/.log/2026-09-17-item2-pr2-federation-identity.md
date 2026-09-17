## [2026-09-17] ingest | item 2 PR 2 — federation identity, and a test vector I got wrong

Up: [dev](../dev.md) · record `docs/design/federated-domains.md` §4, §6, §7 · code `src/federation.rs`.

`DomainId`, the signed `DomainDescriptor`, the revisioned `DomainPolicy`, the bilateral
`TrustBundle`, and the canonical bytes a signature is taken over. Types and bytes only — no
transport, no discovery, no `federation/` prefix, no wire change.

### Why not canonical JSON

A signature is meaningful only if two implementations agree byte for byte on what was signed.
Canonical JSON *can* do that and is a known foot-gun doing it: key ordering, non-ASCII escaping,
surrogate pairs above the BMP, number formatting. This repository already owns a canonical-JSON
encoder — written because a consumer required one — and getting it right meant reimplementing
another language's escaping rules character by character.

Here we own both ends, so the encoding is chosen to have no freedom left in it: every field
length-prefixed, every integer fixed-width little-endian, one encoding per value and no way to
encode two values identically. `length_prefixes_make_the_encoding_unambiguous` pins the reason —
without the prefixes, `["ab","c"]` and `["a","bc"]` would be the same bytes, and an attacker
choosing export names would be choosing which signature to forge.

**Domain separation is a security property, not tidiness.** Each object's bytes start with a
distinct tag, so a policy signature can never authenticate a descriptor. That is D6's *"reuse the
code, never the trust"* turned on our own objects rather than only on the OIDC verifier.

**The bundle decides which key, not the descriptor.** A descriptor carries a `public_key`, and
trusting *that* would make every self-signed document self-authorising — the signature would prove
only that whoever wrote it had a key. Two tests state it: an unknown domain is refused however
well-formed its document is, and a descriptor whose key disagrees with the bundle's is refused
rather than preferred.

### The mistake worth recording

`descriptor_canonical_bytes_are_pinned` failed on its first run. I had **hand-written** the expected
hex and got the little-endian bytes of `issued_at_ms` wrong; the implementation was right.

The tempting fix — paste the produced bytes in — would have left a test that asserts the
implementation against itself. **A vector generated from the code and checked against the code
proves only determinism.** It would pin a wrong encoding just as happily as a right one, and here it
nearly did.

So the bytes were first decoded field by field with an independent reader, confirming they are
exactly the documented encoding with nothing left over, and only then pinned. The structural check
went into the suite too, as `canonical_bytes_decode_back_to_exactly_the_fields_that_went_in`:

- the **structural** test says *this is the right encoding*;
- the **hex pin** says *it has not silently changed*.

Neither says both. This is the same shape as the flaky-vs-reliable detector question in item 6 PR 4
earlier today, from the other direction: there the weaker gate was replaced, here a second gate was
added because the first could not carry the claim alone.

### Gates

`make check` clean · core **191** · mycelium **473** (`tls,metrics,a2a,llm`) / **535**
(`compliance,a2a`) · KV namespace sweep clean.
