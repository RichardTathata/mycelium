# Data-at-rest encryption

## What it is

A node with [persistence](persistence.md) can encrypt what it writes to disk — every WAL record and every snapshot —
through a cipher the operator supplies over their own KMS or keyring. The substrate hands the cipher opaque bytes and
stores what it returns; it does not encrypt the store in memory or the gossip wire, which is the
[mesh's mTLS](mesh-mtls.md). A record that does not decrypt is unreadable state, so a wrong or missing key refuses the
start rather than replaying nothing.

## When to use it, and when not

- **Use it when** the disk a node persists to may be read by someone who should not read the data: a shared volume, a
  backup, a decommissioned drive.
- **Not when** you need key rotation in place: re-encrypting a store under a new key is an offline migration, and
  there is no rotation hook ([crown jewel § 1](../operations/crown-jewel.md#1-data-at-rest-encryption)).

## Turn it on

This feature is turned on in code.

- [`persistence`](../reference/configuration.md#persistence): there is nothing to encrypt without it.
- [`DataAtRestCipher`](../../mycelium-core/src/persistence.rs): implement `encrypt` and `decrypt` over an
  authenticated cipher, so `decrypt` returns `None` on tamper.
- [`with_data_at_rest_cipher`](../../src/agent/mod.rs): attach it before `start()`; an attach after `start()` is
  rejected and counted, because the WAL writer already runs without it.
- The snippet in [crown jewel § Attach a cipher](../operations/crown-jewel.md#attach-a-cipher) is not run as written;
  the test below runs the same calls with a test cipher.
- `test_ws3_data_at_rest_cipher_encrypts_wal_and_round_trips`, in [`src/lib_tests.rs`](../../src/lib_tests.rs):
  plaintext never appears in `wal.bin`, the same cipher recovers the data on restart, and a wrong key refuses the
  start.

## See it work

No runnable example exists: the [example index](../../examples/README.md#the-capability-matrix) has no row for it. The
end-to-end run is the test in [`src/lib_tests.rs`](../../src/lib_tests.rs):
`test_ws3_data_at_rest_cipher_encrypts_wal_and_round_trips`. The late attach is
`a_cipher_attached_after_start_is_rejected_and_the_report_says_so`, in the same file.

## What it guarantees, and what it does not

- **Guarantee:** [`at_rest.cipher`](../reference/guarantee-catalogue.md#at_rest.cipher) — the WAL and snapshot are
  encrypted at rest.
- **Not built:** in-place key rotation; encryption of the store in memory; only the KV WAL and snapshot pass through it —
  the node-local journals (the evidence journal among them), the companion WALs and the node's identity key do not
  (protect `tls` key material separately); the strength is the operator's cipher's — the
  substrate's own test cipher is XOR, illustrative only.
- **Evidence:** the [what-is-proven](../operations/what-is-proven.md) ledger has no row specific to the cipher.

## Go deeper

- **Guide:** [09, security](../guide/09-security.md).
- **Runbook:** [crown jewel § 1, Data-at-rest encryption](../operations/crown-jewel.md#1-data-at-rest-encryption).
- **Design:** no design record; the SOC 2 audit-gap work is the record.
- **Code:** [`mycelium-core/src/persistence.rs`](../../mycelium-core/src/persistence.rs) (the cipher hook, decrypting
  on replay) and `with_data_at_rest_cipher` in [`src/agent/mod.rs`](../../src/agent/mod.rs).
