"""
What a Strands agent needs to work in a Mycelium fleet, for the demonstration in ``run_demo.py``:

* :class:`ScriptedModel` — a Strands ``Model`` that answers from a script, tool calls included, so the run is the
  same every time (the demonstration is about where calls go and who may make them, not about a model's words);
* :class:`Mandate` — an agent's signed grant from the co-op's authority, and the possession proof it makes for
  each call with its own key (the gateway checks both: ``src/agent/gateway_authority.rs``);
* :func:`fleet_tool` — a Strands tool that calls a fleet skill through a node's ``/a2a`` with the A2A 1.0
  ``SendMessage`` request a standard client sends, the skill named in ``metadata.skillId`` and the mandate in
  ``metadata._meta.mandate`` (post-360 row N).
"""

from __future__ import annotations

import base64
import hashlib
import json
import struct
import uuid
from dataclasses import dataclass, field
from typing import Any

import httpx
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from mycelium import arguments_digest, mandate_request_bytes
from strands import tool
from strands.models.model import Model


# ── a model that follows a script ───────────────────────────────────────────


class ScriptedModel(Model):
    """Each turn answers with the next step: ``("text", str)`` or ``("tool", name, input)``."""

    def __init__(self, steps: list[tuple]) -> None:
        self.steps = list(steps)
        self.config: dict[str, Any] = {}

    def update_config(self, **model_config: Any) -> None:
        self.config.update(model_config)

    def get_config(self) -> Any:
        return self.config

    async def structured_output(self, *args: Any, **kwargs: Any):  # pragma: no cover
        raise NotImplementedError
        yield

    async def stream(self, messages: Any, tool_specs: Any = None, system_prompt: Any = None, **kwargs: Any):
        step = self.steps.pop(0) if self.steps else ("text", "(the script is done)")
        yield {"messageStart": {"role": "assistant"}}
        if step[0] == "tool":
            _, name, arguments = step
            yield {"contentBlockStart": {"start": {"toolUse": {"toolUseId": f"t-{uuid.uuid4().hex[:8]}", "name": name}}}}
            yield {"contentBlockDelta": {"delta": {"toolUse": {"input": json.dumps(arguments)}}}}
            yield {"contentBlockStop": {}}
            yield {"messageStop": {"stopReason": "tool_use"}}
        else:
            yield {"contentBlockStart": {"start": {}}}
            yield {"contentBlockDelta": {"delta": {"text": step[1]}}}
            yield {"contentBlockStop": {}}
            yield {"messageStop": {"stopReason": "end_turn"}}


# ── a mandate, and its proof for one call ───────────────────────────────────


def _lp(out: bytearray, b: bytes) -> None:
    out += struct.pack("<I", len(b)) + b


def grant_digest(mandate: dict) -> bytes:
    """SHA-256 of ``Mandate::canonical_bytes`` (``src/mandate/grant.rs``), from the grant's JSON."""
    out = bytearray()
    _lp(out, b"mycelium.mandate/grant/1")
    for f in ("holder", "established_by", "purpose", "scope"):
        _lp(out, mandate[f].encode())
    out += struct.pack("<I", len(mandate["operations"]))
    for op in mandate["operations"]:
        _lp(out, op.encode())
    out += struct.pack("<Q", mandate["epoch"])
    _lp(out, mandate["term"].encode())
    out += struct.pack("<Q", mandate["valid_from_ms"]) + struct.pack("<Q", mandate["valid_until_ms"])
    return hashlib.sha256(bytes(out)).digest()


@dataclass
class Mandate:
    """An agent's grant, and the key only that agent holds."""

    grant: dict
    key: Ed25519PrivateKey

    def present(self, operation: str, resource: str, arguments: dict) -> dict:
        """The grant with a possession proof bound to exactly this call (``possession_message``)."""
        request = mandate_request_bytes(operation, resource, arguments_digest(arguments))
        message = b"mycelium.mandate/possession/1" + grant_digest(self.grant["mandate"]) + struct.pack("<I", len(request)) + request
        return {"grant": self.grant, "possession": base64.b64encode(self.key.sign(message)).decode()}


# ── calling a fleet skill through /a2a ──────────────────────────────────────


@dataclass
class FleetCaller:
    """One agent's way into the fleet: a node's gateway, its bearer (its principal) and its mandate, if it holds one."""

    gateway: str
    token: str
    mandate: Mandate | None = None
    calls: list[dict] = field(default_factory=list)

    def call(self, skill: str, text: str) -> str:
        metadata: dict = {"skillId": skill}
        if self.mandate is not None:
            metadata["_meta"] = {"mandate": self.mandate.present("skill.invoke", f"skill:{skill}", {"text": text})}
        body = {
            "jsonrpc": "2.0",
            "id": uuid.uuid4().hex,
            "method": "SendMessage",
            "params": {
                "message": {"messageId": uuid.uuid4().hex, "role": "ROLE_USER", "parts": [{"text": text}]},
                "metadata": metadata,
            },
        }
        reply = httpx.post(f"{self.gateway}/a2a", json=body, headers={"Authorization": f"Bearer {self.token}"}, timeout=30)
        data = reply.json()
        if "error" in data:
            err = data["error"]
            self.calls.append({"skill": skill, "refused": err})
            return f"refused ({err.get('code')}): {err.get('message')}"
        task = data["result"]["task"]
        answer = " ".join(p.get("text", "") for a in task.get("artifacts", []) for p in a.get("parts", []))
        self.calls.append({"skill": skill, "state": task["status"]["state"], "answer": answer})
        return answer

    def repoint(self, gateway: str) -> None:
        self.gateway = gateway


def fleet_tool(caller: FleetCaller):
    """The Strands tool an agent calls fleet skills with."""

    @tool(name="fleet_skill")
    def fleet_skill(skill: str, text: str) -> str:
        """Ask the fleet: call the skill ``skill`` (``ns/name``) with ``text``, through this agent's node.

        Args:
            skill: the fleet skill to call, ``ns/name``
            text: what to ask it
        """
        return caller.call(skill, text)

    return fleet_skill
