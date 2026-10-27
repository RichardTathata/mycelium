## [2026-10-27] ingest | Y8: the two-domain scaffold

Row Y8 (`docs/plans/adopter-surface.md` §8), the second PR of the Y8 track after #691.

- **What landed:** `mycelium init --template two-domains` — a food bank and a kitchen, two nodes each, every node
  under `secure-multi-domain`, each gateway running an edge and holding a client for the other; `fleet.sh` the
  operator's steps. New in the library and binary: `mycelium tls ca` (`mint_ca`), `mycelium tls pin`, and
  `doctor::run_attached` (the doctor over a node as a program builds it). CI: the `Scaffold` job's two-domain step.
  Pages updated: `dev/security.md` (the two-mesh paragraph), `dev/operations.md` (the templates).
- **Found:** a federated credential presented twice with the same body inside its window is served twice — the edge
  keeps no seen-set; body binding stops a rewrite, not a replay. Stated in the runbook and the README, left to the
  maintainer.
- **Learned:** a gateway serving its node identity's Ed25519 key cannot be read by macOS's system curl
  (LibreSSL/SecureTransport has no Ed25519), so the scaffold's local caller is its own rustls client; `reqwest` with
  both `native-tls` and `rustls` features picks `native-tls` unless told `use_rustls_tls()`. A gateway dispatches to no
  provider whose `sys/caller-context/` marker it has not seen, so "started" is not "ready": the scaffold waits for the
  marker and the exports' resolution before it calls.
- **Not built, stated:** nodes from the published binary (it attaches no evaluator or federation from configuration);
  a published tag with `secure-multi-domain` (the next release); separate hosts.
