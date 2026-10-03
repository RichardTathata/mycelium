## [2026-10-03] ingest | the CA key off the node — a pre-issued node certificate

**What:** `tls::load_or_generate` honours `cert_pem` (+ `key_pem`): loads the CA certificate only
(`load_ca_cert_only`, never mints) and the issued node cert; `tls::issue_node_cert` + `mycelium tls issue`
sign one where the CA key is. Guarantee `id.ca_key_off_node` resolves `enforced` only with `cert_pem`.

**Durable knowledge:** a config field that is declared and never read is a guarantee that can be
reported and never met — `cert_pem` sat in `TlsConfig` since the TLS feature landed, so an operator who
set it got a node that silently re-signed its own cert with the CA key anyway (and, with no key on the
node, *minted a new CA over the fleet's*). The doc-coverage rule "an instruction must work if followed
literally" applies to config fields too: the I1 audit's method (feature · setting · enforcement point ·
what checks it at start) is what found it. Profile rev 2 is announced, not shipped — G12's one-release
notice for a requirement an existing deployment can meet by configuration.
