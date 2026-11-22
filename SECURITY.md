# Security Policy

## Reporting a Vulnerability

**Do not open a public GitHub issue for security vulnerabilities.** Public disclosure before
a fix is available puts all Mycelium users at risk.

Instead, send a private report to:

**security@mycelium.dev** (or richard.nicholson64@protonmail.com until a dedicated address
is configured)

Include:
- A description of the vulnerability and its potential impact
- Steps to reproduce or a proof-of-concept (even partial)
- The version or commit where you observed the issue

## Response commitments

| Milestone | Target |
|-----------|--------|
| Acknowledge receipt | 48 hours |
| Confirm scope and severity | 5 business days |
| Release patch or mitigation | 30 days (critical), 90 days (medium/low) |
| Public CVE / advisory | Coordinated with reporter |

If a timeline cannot be met we will notify you promptly and explain why.

## Scope

**In scope:**
- The `mycelium` Rust library (`src/`)
- Wire protocol security (framing, TLS handshake, Ed25519 signing)
- Authentication and authorisation in the HTTP gateway
- Cryptographic primitives (key generation, signing, verification)
- The release workflow and its build provenance (`.github/workflows/release.yml`)

**Out of scope:**
- Demo credentials in `examples/` (e.g., `postgresql://pipeline:pipeline@...` in the
  fluid_pipeline Docker Compose — these are intentional local-only demo credentials)
- Vulnerabilities in third-party dependencies (report those upstream; we will pull fixes
  into Mycelium promptly when they are available)
- Denial-of-service via resource exhaustion in a test/dev cluster

## Verifying a release

From the first tag cut after security-operations SO3, every release tarball, `SHA256SUMS` and the container image
carry a GitHub artifact attestation (SLSA v1 build provenance, signed keyless through Sigstore and logged in the public
Rekor log), which the release workflow verified before it attached the file:

```bash
ID="https://github.com/RichardTathata/mycelium/.github/workflows/release.yml@refs/tags/vNEW"
gh attestation verify mycelium-NEW-linux-x86_64.tar.gz --repo RichardTathata/mycelium --cert-identity "$ID" --deny-self-hosted-runners
gh attestation verify oci://ghcr.io/richardtathata/mycelium:NEW --repo RichardTathata/mycelium --cert-identity "$ID" --deny-self-hosted-runners
```

`--cert-identity` is exact (workflow file and tag); gh's `--signer-workflow` is a prefix match and would also admit a
workflow whose path merely starts with `release`.

The offline and `cosign` forms are in [`RELEASING.md` § 8](RELEASING.md#8-publish-the-github-release). A release asset
that fails verification, or that verifies as built by any workflow other than `release.yml` or from any ref other than
its `refs/tags/v…` tag, is a vulnerability: report it as above.

## Credit

We are happy to acknowledge security reporters in the release notes and CHANGELOG unless
you prefer to remain anonymous. Please let us know your preference when you report.
