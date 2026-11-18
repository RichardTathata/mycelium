# `tests/fixtures/decision/`

`vector.json` pins the bytes a decider signs to answer a grant request (`grant_decision_message` and
`grant_decision_message_graded`, `src/agent/grant_decision.rs`) and signatures over them. The Rust, Python and
TypeScript suites each read this one file.

**The keys in it are test keys.** `seed_hex` is an Ed25519 seed of thirty-two `0x07` bytes. `graded.consent.pkcs8_hex`
is the private half of a P-256 key generated once for this fixture (2026-11-17), so the pinned consent signatures can
be regenerated. Neither key is, or ever was, a credential for any fleet, and nothing outside the test suites reads
them. A secret scanner that flags the file is flagging these.

ECDSA signatures are randomised: the consent cases' signatures are verified by each suite, never reproduced.
