"""Test doubles shared by the node-free suites: a gateway's KV in memory, and a scripted model."""

from __future__ import annotations

from typing import Any, AsyncIterator

import httpx
from mycelium import KvReceipt
from strands.models.model import Model


class FakeGateway:
    """The four KV verbs the store uses, over a dict — ``MyceliumAgent``'s shapes, no node."""

    def __init__(self, durability: str = "on_disk") -> None:
        self.rows: dict[str, bytes] = {}
        self.durability = durability
        self.fail_with: int | None = None
        self.writes: list[str] = []

    def _maybe_fail(self) -> None:
        if self.fail_with is not None:
            req = httpx.Request("GET", "http://gw/gateway/kv")
            resp = httpx.Response(self.fail_with, request=req)
            raise httpx.HTTPStatusError(f"{self.fail_with}", request=req, response=resp)

    def get(self, key: str) -> bytes | None:
        self._maybe_fail()
        return self.rows.get(key)

    def set(self, key: str, value: bytes) -> KvReceipt:
        self._maybe_fail()
        self.rows[key] = value
        self.writes.append(key)
        return KvReceipt(operation_id=f"op-{len(self.writes)}", local_durability=self.durability)

    def delete(self, key: str) -> None:
        self._maybe_fail()
        self.rows.pop(key, None)

    def keys(self, prefix: str | None = None) -> list[str]:
        self._maybe_fail()
        return sorted(k for k in self.rows if prefix is None or k.startswith(prefix))

    def close(self) -> None:
        pass


class ScriptedModel(Model):
    """Answers every turn with the next reply in ``replies`` — enough for a real ``Agent`` to run."""

    def __init__(self, replies: list[str]) -> None:
        self.replies = list(replies)
        self.config: dict[str, Any] = {}

    def update_config(self, **model_config: Any) -> None:
        self.config.update(model_config)

    def get_config(self) -> Any:
        return self.config

    async def structured_output(self, *args: Any, **kwargs: Any) -> AsyncIterator[Any]:  # pragma: no cover
        raise NotImplementedError
        yield

    async def stream(self, messages: Any, tool_specs: Any = None, system_prompt: Any = None, **kwargs: Any):
        text = self.replies.pop(0) if self.replies else "(no more replies)"
        yield {"messageStart": {"role": "assistant"}}
        yield {"contentBlockStart": {"start": {}}}
        yield {"contentBlockDelta": {"delta": {"text": text}}}
        yield {"contentBlockStop": {}}
        yield {"messageStop": {"stopReason": "end_turn"}}
