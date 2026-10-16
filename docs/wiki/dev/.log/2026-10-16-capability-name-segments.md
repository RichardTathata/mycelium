## [2026-10-16] ingest | a capability name is one key segment, refused at every door

**What changed:** `capability::capability_name_problem(ns, name)` and `capability_group_name_problem(group)`
(`src/capability.rs`, public) decide whether a pair can be a key segment — non-empty, no `/`, not `locality`/`self` —
and every door that writes a `cap/`, `req/` or `gcap/` key consults them: `POST /gateway/capability/advertise` (400
`invalid_capability_name`), `NodeCapabilityConfig::validate` (so `units/declare` and node config), and in process
`advertise_capability` / `declare_requirement` / `define_capability_group` (warn, inert handle). Closes #613's review
finding 6, recorded under E in the post-360 plan §3.6. Upgrade note `docs/guide/deprecations.md` §43.

**The transferable part:** a key built by `format!` from caller-supplied segments needs its segments checked where
the key is built, at every builder — the reader's parser (`parse_cap_key`) already refused the bad shape, so the
writer could produce entries that every reader silently dropped, and the one pair that happened to parse
(`locality`/`self`) collided with a reserved key of a different type. Same family as row E's `prompt_kv_key`.

**Enumeration (rule 1):** `git grep -n 'format!("cap/{}/{}/{}"\|"req/{}/{}/{}"'` (three builders: the two in-process
handles and the HTTP route) and every caller of `.advertise_capability(` / `.declare_requirement(` /
`.define_capability_group(` across `src/` and `mycelium-*/src` — all reach the guarded functions. The data-derived
callers: `mycelium-reason`'s `require_model` / `serve_model` (an `org/model` id was already refused by
`register_prompt_skill`, so its requirement was unsatisfiable), the wasm host's provisioner and stem (names from
manifests and unit files, which `validate()` now checks).

**Test:** `a_capability_name_that_is_not_a_key_segment_is_refused_at_every_door` (`src/agent/http/remainder_tests.rs`),
seen failing first at the route (200 where 400 was expected).
