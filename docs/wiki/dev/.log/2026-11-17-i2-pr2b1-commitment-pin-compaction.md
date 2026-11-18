## [2026-11-17] ingest | CA succession, PR 2b-1: the commitment, the pin, the anchor journal's compaction (I2)

PR 2a's row named PR 2b as one PR; it is four (plan §8, "I2 (PR 2b-1 of 7)"): 2b-1, the old CA's commitment and a
node's pin of it, and the compaction; 2b-2, the successor-signed record, its door, Widened; 2b-3, cut over and narrow,
the window floor, `fleet_ca_path`; 2b-4, the acceptance record, the gateway hold, `id.ca_succession_committed` and the
profile revisions (held on a decision: no shipped build can make a commitment while the generation OID is provisional).

- The commitment (`ca-commit.pem`, `mycelium tls commit`): the **old CA's** Ed25519 signature over its own
  certificate's digest and the successor's, the successor a distinct CA of the next generation. A CA commits once.
- The pin: a node pins only from the file its operator placed (`{auto_cert_dir}/ca-commit.pem`), assessed against a
  root it trusts, persisted at `{auto_cert_dir}/succession/commit.pin.pem` before it goes on. A second commitment is
  refused and counted (`ca_commitment_conflict`). No `[persistence]` with a commitment refuses the start.
- `sys/ca-commit/{node}`: what each node pinned, republished **for detection only** — verified at read against the
  reader's roots, compared with its pin, never adopted. The owner-signed inventory has its row.
- The identity journal is compacted (the maintainer's decision of 2026-11-17: the kept-anchor budget counts what a
  node keeps now). `Journal::rewrite`: new file, fsync, rename, directory fsync; the choice made inside the writer; a
  record leaves the journaled set before its frame is left out. Crash before the rename: the journal as it was. After:
  the compacted journal, and a poisoned writer. Only a journal folded as a set may be rewritten — never the evidence
  journal.
- No new lock (the lock-order table is unchanged); no wire change; sim-seam baseline `tls.rs` and `journal.rs` raised
  in the open.
- Pages touched: `dev/owner-signed-records.md` (the `sys/ca-commit/` row), `dev/architecture/runtime-invariants.md`
  (§Persistence: the identity journal's compaction). Outside the wiki: the plan's I2 row and §8 row, `deprecations.md`
  §120, `CHANGELOG.md`, `features/node-identity.md`, `operations/cert-rotation.md`, `operations/what-is-proven.md`,
  `reference/cli-json.md`, `src/lib.rs`'s namespace table.
