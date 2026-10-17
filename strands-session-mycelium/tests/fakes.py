"""Test doubles shared by the node-free suites: a gateway's KV in memory, and a scripted model."""

from __future__ import annotations

import json
from typing import Any, AsyncIterator

import httpx
from mycelium import KvReceipt
from mycelium.agent import SupersededError
from strands.models.model import Model


class FakeGateway:
    """The KV and consensus verbs the store uses, over dicts — ``MyceliumAgent``'s shapes, no node.

    ``decided`` is the fleet's consensus decisions: pass one dict to several fakes and they are nodes of
    one fleet, each with its own ``rows`` (its own view, which a test can hold back) and one set of claims
    that no node can decide twice — what a quorum gives the real store.
    """

    def __init__(self, durability: str = "on_disk", decided: dict[str, bytes] | None = None) -> None:
        self.rows: dict[str, bytes] = {}
        self.decided: dict[str, bytes] = {} if decided is None else decided
        # Decisions this node's view holds. Like the real committed record (`consensus/life/…`), a
        # decision stays visible when its raw copy under the key is deleted.
        self.seen: set[str] = set()
        self.durability = durability
        self.fail_with: int | None = None
        self.writes: list[str] = []
        # Keys whose next write fails once with a 503 — a transient gateway error.
        self.fail_once: set[str] = set()

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
        if key in self.fail_once:
            self.fail_once.discard(key)
            req = httpx.Request("POST", "http://gw/gateway/kv")
            raise httpx.HTTPStatusError("503", request=req, response=httpx.Response(503, request=req))
        self.rows[key] = value
        self.writes.append(key)
        return KvReceipt(operation_id=f"op-{len(self.writes)}", local_durability=self.durability)

    def consistent_set(self, key: str, value: bytes) -> None:
        self._maybe_fail()
        if key in self.decided:
            req = httpx.Request("POST", "http://gw/gateway/overlay/consistent/set")
            raise SupersededError(httpx.Response(409, request=req, json={"error": "superseded", "ok": False}))
        self.decided[key] = value
        self.seen.add(key)
        self.rows[key] = value  # the raw copy the gateway writes beside the decision

    def consistent_get(self, key: str) -> bytes | None:
        """Local, as the real read is: a decision this node's view does not hold yet reads as none."""
        self._maybe_fail()
        return self.decided.get(key) if key in self.rows or key in self.seen else None

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


def largest_value(key: str) -> int:
    """The longest ``"y" * n`` a ``{"x": …}`` row at ``key`` can hold under the gateway's body limit."""
    from strands_session_mycelium.store import GATEWAY_BODY_LIMIT, request_bytes

    lo, hi = 0, GATEWAY_BODY_LIMIT
    while lo < hi:
        mid = (lo + hi + 1) // 2
        body = json.dumps({"x": "y" * mid}, separators=(",", ":")).encode()
        lo, hi = (mid, hi) if request_bytes(key, body) <= GATEWAY_BODY_LIMIT else (lo, mid - 1)
    return lo
