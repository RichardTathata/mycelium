## [2026-11-22] ingest | The Python companions' dependency floors, and what the checkpointer's floor does not fix

A dependency audit of the Python companions found advisories reachable through their declared floors. Raised:
`langgraph-checkpoint>=4.1.1` (`langgraph-checkpoint-mycelium` 0.3.5), `cryptography>=50.0.0` and `pydantic>=2.4`
(`mycelium-py` 0.2.16), `strands-agents==1.65.0` (`strands-session-mycelium` 0.1.1: 1.61.1 capped `mcp<2.2`), and
`pytest>=9.0.3` with `pytest-asyncio>=1.3.0` in the dev extras (pytest-asyncio below 1.3.0 does not install beside
pytest 9). `mycelium-ts`'s lockfile was refreshed by `npm audit fix` (dev-only).

The durable finding: **the checkpointer loads member-writable rows, and the floor does not make that safe.** A
`ckpt/` row names the type tag its blob is decoded with (`saver.py:538-544`). LangGraph 4.x refuses the `"json"` mode's
constructors, but its default `"msgpack"` mode still imports and calls any type a payload names (a warning only);
`os.system` ran on 4.1.1 and 4.3.2 and was refused under `LANGGRAPH_STRICT_MSGPACK=true`. LangGraph's advisories call
store write access post-exploitation; on a fleet it is membership. So 0.3.5 also changes the saver's default serializer
to `JsonPlusSerializer(allowed_msgpack_modules=None)`, LangGraph's strict mode regardless of the variable
(`saver.py::default_serde`, `tests/test_untrusted_rows.py`, seen failing on the floor alone). What stays open is the
content: the rows are unsigned, so a member can still rewrite a thread's state with allowed types.

Pages: `owner-signed-records.md` (the `ckpt/…` row now says the forgery runs code); `companions/companions.md` (the
Strands pin). Outside the wiki: the gap row `what-is-proven.md#checkpoint-rows-deserialized-unsigned`, the
checkpointer README's *What the serializer trusts*, the root `CHANGELOG.md`.
