"""The two causal fields (adopter-surface X4) a take, claim or emit door reads from a body's ``_meta``
(adopter-surface Y11) — and only when an exposure rule on the node selects that door."""

from __future__ import annotations

from typing import Any, Optional


def with_causal(body: dict[str, Any], correlation: Optional[str], parent: Optional[str]) -> dict[str, Any]:
    """``body`` with ``_meta`` carrying whichever of ``correlation`` and ``parent`` is set; unchanged when neither is."""
    meta: dict[str, str] = {}
    if correlation is not None:
        meta["correlation"] = correlation
    if parent is not None:
        meta["parent"] = parent
    if meta:
        body["_meta"] = meta
    return body
