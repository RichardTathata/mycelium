"""
The Strands interface this package implements, pinned. A Strands release that adds, removes or
re-signs a ``SessionRepository`` method fails here — re-check the store against it, then update
both this record and the pin in ``pyproject.toml`` (and CI's install line).
"""

from __future__ import annotations

import inspect
from importlib.metadata import version

from strands.session.session_repository import SessionRepository

from strands_session_mycelium import MyceliumSessionRepository

PINNED = "1.61.1"

# Each method's parameters — name and default — in order. Annotations are left out: their text varies
# with the Python version, and a change of type alone shows up in the suites that use the method.
RECORDED = {
    "create_session": ["self", "session", "**kwargs"],
    "read_session": ["self", "session_id", "**kwargs"],
    "create_agent": ["self", "session_id", "session_agent", "**kwargs"],
    "read_agent": ["self", "session_id", "agent_id", "**kwargs"],
    "update_agent": ["self", "session_id", "session_agent", "**kwargs"],
    "create_message": ["self", "session_id", "agent_id", "session_message", "**kwargs"],
    "read_message": ["self", "session_id", "agent_id", "message_id", "**kwargs"],
    "update_message": ["self", "session_id", "agent_id", "session_message", "**kwargs"],
    "list_messages": ["self", "session_id", "agent_id", "limit=None", "offset=0", "**kwargs"],
    "create_multi_agent": ["self", "session_id", "multi_agent", "**kwargs"],
    "read_multi_agent": ["self", "session_id", "multi_agent_id", "**kwargs"],
    "update_multi_agent": ["self", "session_id", "multi_agent", "**kwargs"],
}


def shape(p: inspect.Parameter) -> str:
    if p.kind is p.VAR_KEYWORD:
        return f"**{p.name}"
    if p.kind is p.VAR_POSITIONAL:
        return f"*{p.name}"
    return p.name if p.default is p.empty else f"{p.name}={p.default!r}"


def test_the_installed_strands_is_the_pinned_release():
    assert version("strands-agents") == PINNED


def test_session_repository_is_the_interface_this_package_implements():
    methods = {
        name: [shape(p) for p in inspect.signature(fn).parameters.values()]
        for name, fn in inspect.getmembers(SessionRepository, inspect.isfunction)
        if not name.startswith("_")
    }
    assert methods == RECORDED


def test_every_method_is_implemented_here():
    for name in RECORDED:
        assert getattr(MyceliumSessionRepository, name) is not getattr(SessionRepository, name), name
    assert not MyceliumSessionRepository.__abstractmethods__
