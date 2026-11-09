"""A ``tools/call`` client for a Mycelium gateway's ``/mcp`` door, and a client's own grants (identity-and-lifecycle I1).

A service with a client certificate signs each call with its key (:class:`~mycelium.ClientSigner`, sent as
``params._meta.cs``/``cn``); the provider that runs the call checks it. A tool whose policy requires a grant is called
under one the service holds: it asks with :meth:`McpClient.request_grant`, a supervisor issues it at the gateway's
decision door, it reads the answer with :meth:`McpClient.grant_status`, and presents it with
:meth:`McpClient.call_tool` ``(…, grant=…)``, which proves possession with the same key.

Usage::

    from mycelium import ClientSigner, McpClient

    signer = ClientSigner(open("forecaster.key.pem", "rb").read(), "token:fleet/forecaster")
    mcp = McpClient("https://gw:8300", token=..., signer=signer)
    asked = mcp.request_grant("weather.forecast", {"city": "Lyon"})
    answer = mcp.wait_for_grant(asked["request"])          # or poll grant_status
    result = mcp.call_tool("weather.forecast", {"city": "Lyon"}, grant=answer["grant"])
"""

from __future__ import annotations

import time
from typing import Any, Dict, Optional

from ._pool import ClientPool, path_segment
from .a2a import A2aError, ActionRefusedError
from .client_sig import ClientSigner

__all__ = ["McpClient", "McpError", "GrantNotIssuedError"]


class McpError(A2aError):
    """A JSON-RPC error from ``/mcp``: the numeric ``code``, ``message`` and ``data``. The action evaluator's three
    refusals raise :class:`~mycelium.a2a.ActionRefusedError` instead, so they stay told apart."""

    def __init__(self, code: int, message: str, data: Optional[Dict] = None) -> None:
        super().__init__(code, message, data)
        self.args = (f"MCP error {code}: {message}",)


class GrantNotIssuedError(Exception):
    """A grant request ended without a grant: ``state`` is ``refused``, ``revoked`` or ``expired`` (or ``pending``
    when :meth:`McpClient.wait_for_grant` timed out); ``answer`` is the door's whole answer."""

    def __init__(self, state: str, answer: Dict[str, Any]) -> None:
        super().__init__(f"the grant request ended {state}")
        self.state = state
        self.answer = answer


def _raise(error: Dict[str, Any]) -> None:
    code, message, data = error.get("code", 0), error.get("message", ""), error.get("data")
    if code in ActionRefusedError.CODES:
        raise ActionRefusedError(code, message, data)
    raise McpError(code, message, data)


class McpClient:
    """HTTP client for a gateway's ``/mcp`` door (``mcp:invoke``).

    Parameters
    ----------
    base_url:
        The gateway's base URL, e.g. ``"http://localhost:8300"``.
    timeout_secs:
        Default request timeout in seconds.
    token:
        Gateway bearer; defaults to ``MYCELIUM_GATEWAY_TOKEN`` when unset.
    ca_file:
        PEM bundle of a private fleet CA to trust for an ``https://`` base URL.
    signer:
        The service's :class:`~mycelium.ClientSigner`: every ``tools/call`` is then signed (``cs``/``cn``), and a
        grant passed to :meth:`call_tool` is presented with a possession proof by the same key. Its principal must be
        the one the bearer authenticates as, or the provider refuses the call.
    """

    def __init__(
        self,
        base_url: str,
        *,
        timeout_secs: float = 30.0,
        token: Optional[str] = None,
        ca_file: Optional[str] = None,
        signer: Optional[ClientSigner] = None,
    ) -> None:
        self._base = base_url.rstrip("/")
        self._timeout = timeout_secs
        self._pool = ClientPool(self._base, timeout_secs, token=token, ca_file=ca_file)
        self.signer = signer
        self._id = 0

    def _rpc(self, method: str, params: Dict[str, Any], timeout_secs: Optional[float]) -> Any:
        self._id += 1
        payload = {"jsonrpc": "2.0", "id": self._id, "method": method, "params": params}
        timeout = timeout_secs or self._timeout
        with self._pool.sync(timeout=timeout + 5.0) as c:
            resp = c.post("/mcp", json=payload)
        resp.raise_for_status()
        body = resp.json()
        if "error" in body:
            _raise(body["error"])
        return body.get("result")

    def list_tools(self, *, timeout_secs: Optional[float] = None) -> list:
        """``tools/list``: the tools this gateway can call."""
        result = self._rpc("tools/list", {}, timeout_secs) or {}
        return list(result.get("tools", []))

    def call_tool(
        self,
        name: str,
        arguments: Optional[Dict[str, Any]] = None,
        *,
        grant: Optional[Dict[str, Any]] = None,
        mandate: Optional[Dict[str, Any]] = None,
        correlation: Optional[str] = None,
        parent: Optional[str] = None,
        timeout_secs: Optional[float] = None,
    ) -> Any:
        """``tools/call`` of ``name`` with ``arguments``; returns the JSON-RPC ``result``.

        With a ``signer`` the call is signed (``params._meta.cs``/``cn``); arguments holding an integer outside
        ±(2**53 - 1) raise ``ValueError`` before anything is sent. ``grant`` is a grant this signer's principal holds
        (``grant_status(...)["grant"]``): it is presented as ``params._meta.mandate`` with a possession proof for
        this call. ``mandate`` presents an already-proven ``{"grant", "possession"}`` instead. ``correlation`` and
        ``parent`` are the causal fields (adopter-surface X4).

        Raises :class:`~mycelium.a2a.ActionRefusedError` for the evaluator's refusals and :class:`McpError` for any
        other JSON-RPC error.
        """
        args: Dict[str, Any] = {} if arguments is None else arguments
        meta: Dict[str, Any] = {}
        if grant is not None:
            if self.signer is None:
                raise ValueError("presenting a grant needs the holder's signer: McpClient(..., signer=...)")
            if mandate is not None:
                raise ValueError("pass a grant or a mandate, not both")
            mandate = self.signer.prove_possession(grant, "tools/call", f"tool:{name}", args)
        if mandate is not None:
            meta["mandate"] = mandate
        if self.signer is not None:
            meta.update(self.signer.sign_tools_call(name, args))
        if correlation is not None:
            meta["correlation"] = correlation
        if parent is not None:
            meta["parent"] = parent
        params: Dict[str, Any] = {"name": name, "arguments": args}
        if meta:
            params["_meta"] = meta
        return self._rpc("tools/call", params, timeout_secs)

    # ── A client's own grants ────────────────────────────────────────────────

    def request_grant(
        self,
        tool: str,
        arguments: Optional[Dict[str, Any]] = None,
        *,
        arguments_class: Optional[str] = None,
        window_secs: Optional[int] = None,
    ) -> Dict[str, Any]:
        """``POST /gateway/grants/request`` (``mcp:invoke``): ask for a grant to call ``tool`` with ``arguments``.

        The caller must hold a client certificate on record at the gateway (``409 not_a_client_holder``). A request
        standing for the same tool and arguments is answered again (``"reused": True``); past 16 standing requests the
        door answers ``429 too_many_pending``. ``arguments_class`` names the arguments' data class (a classified set is
        withheld from the record the supervisor reads; the grant binds the digest either way); ``window_secs`` is how
        long the request stands unanswered (default 60, at most 600). Returns ``request``, ``key``, ``resource``,
        ``arguments_digest``, ``expires_at_ms`` and ``reused``; an HTTP refusal raises
        :class:`httpx.HTTPStatusError`.
        """
        body: Dict[str, Any] = {"tool": tool, "arguments": {} if arguments is None else arguments}
        if arguments_class is not None:
            body["arguments_class"] = arguments_class
        if window_secs is not None:
            body["window_secs"] = window_secs
        with self._pool.sync() as c:
            resp = c.post("/gateway/grants/request", json=body)
            resp.raise_for_status()
            data = resp.json()
        return data if isinstance(data, dict) else {}

    def grant_status(self, request: str, *, timeout_secs: Optional[float] = None) -> Dict[str, Any]:
        """``GET /gateway/grants/request/{request}`` (``mcp:invoke``): the caller's own request and its answer —
        ``state`` is ``pending``, ``issued`` (with ``grant``, to pass to :meth:`call_tool`), ``refused`` (with
        ``refusal``), ``revoked`` or ``expired``. Another principal's request is ``404 unknown_request``."""
        kwargs = {} if timeout_secs is None else {"timeout": timeout_secs}
        with self._pool.sync(**kwargs) as c:
            resp = c.get(f"/gateway/grants/request/{path_segment(request)}")
            resp.raise_for_status()
            data = resp.json()
        return data if isinstance(data, dict) else {}

    def wait_for_grant(self, request: str, *, timeout_secs: float = 60.0, poll_secs: float = 0.5) -> Dict[str, Any]:
        """Poll :meth:`grant_status` until the request is answered; return the ``issued`` answer, or raise
        :class:`GrantNotIssuedError` naming the state it ended in (``pending`` when ``timeout_secs`` passed first).
        Each read and each pause is bounded by the time left, so the wait ends within ``timeout_secs``; a read the
        deadline cuts short is ``pending`` with the last answer seen."""
        import httpx

        deadline = time.monotonic() + timeout_secs
        answer: Dict[str, Any] = {}
        while True:
            left = deadline - time.monotonic()
            if left <= 0:
                raise GrantNotIssuedError("pending", answer)
            bound = min(left, self._timeout)
            try:
                answer = self.grant_status(request, timeout_secs=bound)
            except httpx.TimeoutException:
                if bound < self._timeout:  # cut short by the deadline, not the client's own timeout
                    raise GrantNotIssuedError("pending", answer) from None
                raise
            state = answer.get("state", "")
            if state == "issued":
                return answer
            if state != "pending":
                raise GrantNotIssuedError(state, answer)
            time.sleep(max(0.0, min(poll_secs, deadline - time.monotonic())))

    def close(self) -> None:
        """Close the underlying HTTP client."""
        self._pool.close()

    def __enter__(self) -> "McpClient":
        return self

    def __exit__(self, *_: object) -> None:
        self.close()
