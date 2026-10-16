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
`register_prompt_skill`, so its requirement was unsatisfiable), the wasm host's stem (unit files, which `validate()`
now checks) and provisioner (a catalogue manifest's `provides` is not checked at load — a bad one installs and its
advert is refused in process, with a warning).

**Review (#629):** two doors the first enumeration missed, both fixed — the emergent-group projector builds `gcap/` and
`grp/` keys from *every* `cap-group/` definition in its view, a 2.33 peer's or a persisted one included, so it now
skips a definition whose names fail the check (`the_group_projector_skips_a_definition_whose_names_are_not_key_segments`);
and `[[serve]]` / `register_prompt_skill` accepted `locality`/`self` (a template written, the advert refused, `Ok`
returned) and now refuse it as `InvalidName`. The lesson: a key a node *derives* from gossiped state is a door too.

**Test:** `a_capability_name_that_is_not_a_key_segment_is_refused_at_every_door` (`src/agent/http/remainder_tests.rs`),
seen failing first at the route (200 where 400 was expected).
