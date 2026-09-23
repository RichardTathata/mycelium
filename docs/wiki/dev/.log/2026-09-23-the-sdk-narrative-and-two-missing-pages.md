## [2026-09-23] ingest | §12.2's last two lines — a field is not a narrative, and two pages the wiki owed

Up: [dev](../dev.md) · pages added: [architecture/contracts](../architecture/contracts.md),
[testing/replay](../testing/replay.md) · pages touched: [dev](../dev.md) (routing; the AE heading),
[architecture](../architecture/architecture.md), [testing](../testing/testing.md) (five sections
moved out), [wiki root](../../wiki.md), [AGENTS](../../AGENTS.md) (a routing table) · docs
`mycelium-py/README.md`, `mycelium-ts/README.md`, guide 15,
`langgraph-checkpoint-mycelium/README.md`.

## A carried field is not a delivered contract

Both SDKs have exposed `local_durability` / `localDurability` since 2.8.0, and neither README ever
said what the four states mean. That passes a parity gate — the field is there, on time, in both
languages — and fails the thing the gate exists for: an SDK user who has to read
`mycelium-core/src/receipt.rs` to interpret a field they were handed has been given a value, not a
contract.

So both READMEs now carry the four rungs as a table, each `LocalDurability` state as an operational
fact (`buffered` **survives a process crash and is lost to a power failure** — not a softer
`on_disk`; `failed` is **not** "the record is absent"; `not_configured` is the state `persisted:
true` hides), and the rule that a timeout is not a negative — with federation's `DeliveryUnknown`
named as the same rule one boundary out, so the two do not read as unrelated quirks.

## The unflattering half, which is the useful half

The LangGraph chapter's rung 2 says checkpoints are durable in the mesh.
`MyceliumCheckpointSaver.put()` writes its index row with a plain `POST /gateway/kv`
(`saver.py::_kv_set`), which is **receipt rung 1**: applied to the store of the node you are talking
to. It says nothing about that node's disk and nothing about any peer.

A checkpoint `put()` acknowledged is therefore **not yet a checkpoint that survives losing that
node** — and the flagship demo has known this all along, in its code rather than its prose. It does
not trust the write for replication: it reads from node B in a bounded poll until the checkpoint
appears there, and only then kills A. That is a client-side *observation* of rung 3, paid for in
latency because the write path never asks for it.

Both the chapter and the checkpointer's README now say so, with the deliberate choice spelled out
(accept rung 1 for intermediate super-steps; ask for more before an irreversible effect or a
hand-off). **The chapter's "rungs" are demo steps and guide 18's are receipts** — unrelated ladders
with one word, now stated where the two meet.

## Two pages the wiki owed, and why they are pages

`dev/architecture/contracts.md` and `dev/testing/replay.md` were §12.2's *page per new mechanism*,
and both had the same symptom before: the knowledge existed, scattered across the pages of whatever
had touched it. `testing.md` had grown **five** replay sections and was the longest page in `dev/`;
replay is one subject, and it is the subject a session reaches for when a recorded run does not
reproduce. The sections moved wholesale — same directory, so every relative link survived — leaving
a pointer.

`AGENTS.md` gains a routing table for the axis' mechanisms, because the failure mode these two pages
came from is not *nobody wrote it down*, it is *nobody knew where it went*. And `dev/dev.md`'s AE
heading said **"not implemented"** for nine releases while the paragraphs beneath it were kept
current after every PR — a heading nobody re-reads is exactly where drift hides, which is the
`/wiki-lint` doc-vs-code sweep's whole thesis, found by hand this time.
