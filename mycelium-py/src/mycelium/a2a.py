"""A2A (Agent-to-Agent) protocol client for Mycelium.

Allows calling external A2A-speaking agents (AutoGen, LangChain, etc.) from
Python without knowing they are on Mycelium, and discovering their skills.

Usage::

    from mycelium.a2a import A2aClient

    client = A2aClient("http://host:8300")
    card   = client.fetch_card()          # discover available skills
    reply  = client.send("compute/gpu", "hello")
    for event in client.stream("compute/gpu", "hello"):
        print(event)
"""

from __future__ import annotations

import hashlib
import json
import struct
import uuid
from typing import Dict, Iterator, Optional

import httpx

from ._pool import ClientPool

__all__ = ["A2aClient", "A2aError", "ActionRefusedError"]


class A2aError(Exception):
    """A JSON-RPC error from the A2A endpoint.

    Carries the numeric ``code`` and ``message`` rather than folding them into a string, so a
    caller can branch on them. Previously this raised a bare :class:`KeyError` with the code
    inside the message — which is the wrong type for a protocol error and forced callers to parse
    English to find out what happened.
    """

    def __init__(self, code: int, message: str, data: Optional[Dict] = None) -> None:
        super().__init__(f"A2A error {code}: {message}")
        self.code = code
        self.message = message
        self.data = data or {}


class ActionRefusedError(A2aError):
    """The gateway's action evaluator refused the call.

    **Three refusals, and they must not be collapsed.** This is the distinction the whole
    authorisation slice exists to keep, and it has to survive the trip to a caller:

    ``action_denied`` (-32030)
        An authority decided **no**. A prohibition matched. Do not retry; the answer will not
        change until the policy does.

    ``authority_not_established`` (-32031)
        **Nobody decided.** No clause covered the action, a fact could not be established, the
        policy revision was not the expected one, or the operation is outside the reviewed
        catalogue. This is *not* evidence of drift or of a violation — treating it as one reports
        a breach that never happened.

    ``evidence_not_recorded`` (-32032)
        The policy **permitted** the action, but the decision could not be recorded, so it was
        refused anyway. An action allowed to proceed with no record of why is an unlogged gate,
        not governance. Retrying is reasonable once the recording path is healthy.

    ``policy_revision`` in :attr:`data` is what tells a *stale policy* apart from a real denial:
    if the refusal names a revision you did not deploy, redeploy and retry rather than stop.
    """

    DENIED = "action_denied"
    NOT_ESTABLISHED = "authority_not_established"
    NOT_RECORDED = "evidence_not_recorded"

    #: The three JSON-RPC codes the evaluator seam uses.
    CODES = {-32030: DENIED, -32031: NOT_ESTABLISHED, -32032: NOT_RECORDED}

    @property
    def reason(self) -> str:
        """The machine-readable reason, from ``data.reason`` or the code."""
        return self.data.get("reason") or self.CODES.get(self.code, "unknown")

    @property
    def denied(self) -> bool:
        """An authority decided no."""
        return self.reason == self.DENIED

    @property
    def authority_not_established(self) -> bool:
        """Nobody decided — **never** the same as a denial."""
        return self.reason == self.NOT_ESTABLISHED

    @property
    def evidence_not_recorded(self) -> bool:
        """Permitted, but unrecordable, so refused."""
        return self.reason == self.NOT_RECORDED

    @property
    def policy_revision(self) -> Optional[str]:
        """Which policy artifact decided, when the gateway said."""
        return self.data.get("policy_revision")

    @property
    def checked(self) -> list:
        """What the evaluator reports it checked."""
        return self.data.get("checked", [])


def _raise_for_error(error: Dict) -> None:
    """Raise the most specific error type this JSON-RPC error warrants."""
    code = error.get("code", 0)
    message = error.get("message", "")
    data = error.get("data")
    if code in ActionRefusedError.CODES:
        raise ActionRefusedError(code, message, data)
    raise A2aError(code, message, data)


def arguments_digest(arguments: Dict) -> bytes:
    """SHA-256 of ``arguments`` as the gateway canonicalises them: sorted keys, no whitespace, UTF-8.

    For ``/a2a`` the arguments are ``{"text": message}``. This is the digest a mandate holder's
    possession proof binds to (Boundary H, A1 at the gateway).
    """
    canonical = json.dumps(arguments, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    return hashlib.sha256(canonical.encode("utf-8")).digest()


def mandate_request_bytes(operation: str, resource: str, digest: bytes) -> bytes:
    """The exact request bytes a presented mandate's possession proof binds to.

    ``resource`` may include the resolved provider (``skill:ns/name@node``); only the part before
    ``@`` is bound. The holder signs ``possession_message(grant, these bytes)`` with its own key and
    passes the grant and base64 signature as ``mandate=`` — this SDK does not sign. The format is
    pinned by a golden vector shared with the gateway and the TypeScript SDK.
    """
    if len(digest) != 32:
        raise ValueError("the arguments digest is 32 bytes")
    out = bytearray(b"mycelium.gateway/mandate-request/1")
    for part in (operation.encode("utf-8"), resource.split("@", 1)[0].encode("utf-8")):
        out += struct.pack("<I", len(part)) + part
    out += digest
    return bytes(out)


def _task_params(task_id: str, skill_id: str, message: str, mandate: Optional[Dict]) -> Dict:
    params: Dict = {
        "id":       task_id,
        "skillId":  skill_id,
        "message":  {"role": "user", "parts": [{"type": "text", "text": message}]},
    }
    if mandate is not None:
        # {"grant": <SignedMandateGrant>, "possession": <base64>} — see mandate_request_bytes.
        params["_meta"] = {"mandate": mandate}
    return params


class A2aClient:
    """HTTP client for A2A-protocol nodes (requires the ``a2a`` cargo feature).

    Parameters
    ----------
    base_url:
        Base URL of the target node's HTTP gateway, e.g. ``"http://localhost:8300"``.
        The ``/.well-known/agent.json`` and ``/a2a`` paths are derived from this.
    timeout_secs:
        Default RPC timeout in seconds (can be overridden per call).
    """

    def __init__(self, base_url: str, *, timeout_secs: float = 30.0, token: Optional[str] = None) -> None:
        self._base   = base_url.rstrip("/")
        self._timeout = timeout_secs
        self._pool   = ClientPool(self._base, timeout_secs, token=token)

    # ── Discovery ─────────────────────────────────────────────────────────────

    def fetch_card(self) -> Dict:
        """Fetch the AgentCard from ``/.well-known/agent.json``.

        Returns
        -------
        dict
            Parsed AgentCard JSON with at minimum ``name``, ``url``, and ``skills``.

        Raises
        ------
        httpx.HTTPStatusError
            If the server returns a non-2xx status.
        """
        with self._pool.sync() as c:
            resp = c.get("/.well-known/agent.json")
        resp.raise_for_status()
        return resp.json()

    # ── Synchronous task dispatch ─────────────────────────────────────────────

    def send(
        self,
        skill_id: str,
        message:  str,
        *,
        timeout_secs: Optional[float] = None,
        task_id: Optional[str] = None,
        mandate: Optional[Dict] = None,
    ) -> str:
        """Send a ``tasks/send`` request and return the reply text.

        Parameters
        ----------
        skill_id:
            Skill identifier in ``"ns/name"`` format, e.g. ``"compute/gpu"``.
        message:
            Plain-text message to the skill.
        timeout_secs:
            Override the per-client default timeout.
        task_id:
            Optional explicit task ID; auto-generated if omitted.
        mandate:
            Optional presented mandate, ``{"grant": ..., "possession": ...}``, sent as
            ``params._meta.mandate``. A gateway with an execution authority establishes it for this
            call (Boundary H A1); see :func:`mandate_request_bytes` for what the holder signs.

        Returns
        -------
        str
            The first text part of the completed task's first artifact.

        Raises
        ------
        KeyError
            If the server responds with a JSON-RPC error.
        """
        tid      = task_id or str(uuid.uuid4())
        timeout  = timeout_secs or self._timeout
        payload  = {
            "jsonrpc": "2.0",
            "id":      1,
            "method":  "tasks/send",
            "params":  _task_params(tid, skill_id, message, mandate),
        }
        with self._pool.sync(timeout=timeout + 5.0) as c:  # network headroom
            resp = c.post("/a2a", json=payload)
        resp.raise_for_status()
        body = resp.json()
        if "error" in body:
            _raise_for_error(body["error"])
        task = body.get("result", {})
        return _extract_text(task)

    # ── SSE streaming ─────────────────────────────────────────────────────────

    def stream(
        self,
        skill_id: str,
        message:  str,
        *,
        timeout_secs: Optional[float] = None,
        task_id: Optional[str] = None,
        mandate: Optional[Dict] = None,
    ) -> Iterator[Dict]:
        """Send a ``tasks/sendSubscribe`` request and yield SSE status events.

        Each yielded dict is a deserialized ``task_status_update`` SSE event,
        e.g. ``{"id": "…", "status": {"state": "working"}}``.

        The iterator terminates when the stream closes (completed or failed).

        Parameters
        ----------
        skill_id:
            Skill identifier, e.g. ``"llm/chat"``.
        message:
            Plain-text input.
        timeout_secs:
            Override the streaming read timeout.
        task_id:
            Optional explicit task ID.

        Yields
        ------
        dict
            Task status update events.
        """
        tid     = task_id or str(uuid.uuid4())
        timeout = timeout_secs or self._timeout
        payload = {
            "jsonrpc": "2.0",
            "id":      1,
            "method":  "tasks/sendSubscribe",
            "params":  _task_params(tid, skill_id, message, mandate),
        }
        with httpx.stream(
            "POST",
            f"{self._base}/a2a",
            json=payload,
            headers=self._pool.headers,
            timeout=timeout + 5.0,
        ) as resp:
            resp.raise_for_status()
            for line in resp.iter_lines():
                if line.startswith("data:"):
                    data = line[len("data:"):].strip()
                    if data:
                        try:
                            yield json.loads(data)
                        except json.JSONDecodeError:
                            pass

    def close(self) -> None:
        """Close the underlying HTTP client."""
        self._pool.close()

    def __enter__(self) -> "A2aClient":
        return self

    def __exit__(self, *_: object) -> None:
        self.close()


# ── Helpers ───────────────────────────────────────────────────────────────────

def _extract_text(task: Dict) -> str:
    """Return the first text part from a completed task artifact, or empty string."""
    artifacts = task.get("artifacts", [])
    if not artifacts:
        return ""
    parts = artifacts[0].get("parts", [])
    if not parts:
        return ""
    return parts[0].get("text", "")
