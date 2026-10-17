"""
strands_session_mycelium.store — a Strands Agents session store on the Mycelium mesh.

An agent's conversation and state, and a multi-agent orchestrator's (a swarm's or a graph's) shared
state, live in the fleet's gossiped KV, reached through the node's HTTP gateway with the Mycelium
Python SDK (``mycelium.MyceliumAgent``). Any node of the fleet can resume a session another node
wrote, once the rows have gossiped to it.

Rows, one per record (every segment percent-encoded, so an id cannot forge a ``/``)::

    {prefix}/{session}/session                      the Session
    {prefix}/{session}/agent/{agent}                the SessionAgent (state, conversation-manager state)
    {prefix}/{session}/agent/{agent}/count          {"count": n} — messages 0..n-1 were written
    {prefix}/{session}/agent/{agent}/msg/{index}    one SessionMessage, the index zero-padded
    {prefix}/{session}/multi/{id}                   a MultiAgentBase's serialized state

What a write's receipt names. Every write returns the SDK's :class:`mycelium.KvReceipt`, kept on
:attr:`MyceliumSessionRepository.last_receipt`. It names **local application on the gateway node**
(rung 1) and that node's own disk state as ``local_durability`` (rung 2) — **not replication**:
nothing here waits for a peer. A session saved on node A is readable on node B only once it has
gossiped there (``docs/design/contracts-receipts.md``). ``require_on_disk=True`` makes a write whose
receipt is not ``"on_disk"`` raise.

Incomplete is never empty. Rows gossip independently, so a reader on another node can see a
message before the agent that owns it, or message 7 before message 6. Each such case raises
:class:`IncompleteSession` (a ``SessionException``) naming the keys that are missing — retriable once
they arrive — rather than returning ``None`` or a shorter conversation: a resumed agent that saw a
shorter conversation would append at the wrong index and overwrite a message it never read. A row
that cannot be decoded raises ``SessionException``; so does a gateway error (401, 403, 5xx).
``None`` means only that no row of that record exists anywhere under the session.

Concurrent agents. Two agents with different ids in one session write disjoint rows and never lose
each other's writes, on one node or several. **One agent id is one writer**: two processes running
the same agent id in the same session overwrite each other's rows (last writer wins), as the same
agent id in two Strands managers would. A multi-agent record is likewise its orchestrator's alone.
"""

from __future__ import annotations

import json
import logging
from typing import TYPE_CHECKING, Any, Callable, Optional
from urllib.parse import quote, unquote

import httpx
from mycelium import KvReceipt, MyceliumAgent
from strands.session.repository_session_manager import RepositorySessionManager
from strands.session.session_repository import SessionRepository
from strands.types.exceptions import SessionException
from strands.types.session import Session, SessionAgent, SessionMessage

if TYPE_CHECKING:
    from strands.multiagent.base import MultiAgentBase

logger = logging.getLogger(__name__)

# `mycelium_core::framing::MAX_KV_WRITE_BYTES` — the gateway refuses a larger value. Checked here
# so the error names the record rather than arriving as a 500.
MAX_ROW_BYTES = 10 * 1024 * 1024 - 64 * 1024

# Wide enough for any message index Strands produces; fixed width keeps the keys in index order.
_INDEX_WIDTH = 10


class IncompleteSession(SessionException):
    """Part of a session is visible and part of it is not yet.

    ``missing`` lists the KV keys that a complete read needs and could not find. Retry once they
    have gossiped to this node; this is neither an absent session nor a shorter one.
    """

    def __init__(self, what: str, missing: list[str]) -> None:
        self.missing = missing
        super().__init__(f"{what} is incomplete on this node; missing {missing} (retry once they arrive)")


def _seg(value: str, what: str) -> str:
    if not isinstance(value, str) or value == "":
        raise SessionException(f"{what} must be a non-empty string, got {value!r}")
    return quote(value, safe="")


class MyceliumSessionRepository(SessionRepository):
    """Strands' ``SessionRepository`` over a Mycelium gateway's KV routes.

    Args:
        agent: the SDK client for the gateway to read and write through.
        prefix: the first key segment of every row (an application namespace — the gateway's KV
            routes refuse the namespaces the substrate owns).
        require_on_disk: raise when a write's receipt is not ``"on_disk"`` on the gateway node.
        on_receipt: called with every write's key and receipt.
    """

    def __init__(
        self,
        agent: MyceliumAgent,
        *,
        prefix: str = "strands",
        require_on_disk: bool = False,
        on_receipt: Optional[Callable[[str, KvReceipt], None]] = None,
    ) -> None:
        self._agent = agent
        self._prefix = _seg(prefix, "prefix")
        self._require_on_disk = require_on_disk
        self._on_receipt = on_receipt
        self.last_receipt: Optional[KvReceipt] = None
        # (session, agent) -> the count this process last wrote, so a message write rewrites the
        # count row only when it grows.
        self._counts: dict[tuple[str, str], int] = {}

    # ── keys ────────────────────────────────────────────────────────────────

    def _session_root(self, session_id: str) -> str:
        return f"{self._prefix}/{_seg(session_id, 'session_id')}/"

    def _session_key(self, session_id: str) -> str:
        return self._session_root(session_id) + "session"

    def _agent_key(self, session_id: str, agent_id: str) -> str:
        return self._session_root(session_id) + "agent/" + _seg(agent_id, "agent_id")

    def _count_key(self, session_id: str, agent_id: str) -> str:
        return self._agent_key(session_id, agent_id) + "/count"

    def _messages_prefix(self, session_id: str, agent_id: str) -> str:
        return self._agent_key(session_id, agent_id) + "/msg/"

    def _message_key(self, session_id: str, agent_id: str, message_id: int) -> str:
        if not isinstance(message_id, int) or isinstance(message_id, bool) or message_id < 0:
            raise SessionException(f"message_id must be a non-negative integer, got {message_id!r}")
        return self._messages_prefix(session_id, agent_id) + str(message_id).zfill(_INDEX_WIDTH)

    def _multi_key(self, session_id: str, multi_agent_id: str) -> str:
        return self._session_root(session_id) + "multi/" + _seg(multi_agent_id, "multi_agent_id")

    # ── the gateway ─────────────────────────────────────────────────────────

    def _keys(self, prefix: str) -> list[str]:
        try:
            return self._agent.keys(prefix=prefix)
        except httpx.HTTPError as e:
            raise SessionException(f"listing {prefix!r} through the gateway failed: {e}") from e

    def _read(self, key: str) -> Optional[dict[str, Any]]:
        try:
            raw = self._agent.get(key)
        except httpx.HTTPError as e:
            raise SessionException(f"reading {key!r} through the gateway failed: {e}") from e
        if raw is None:
            return None
        try:
            value = json.loads(raw)
        except (UnicodeDecodeError, json.JSONDecodeError) as e:
            raise SessionException(f"the row at {key!r} is unreadable: {e}") from e
        if not isinstance(value, dict):
            raise SessionException(f"the row at {key!r} is unreadable: not a JSON object")
        return value

    def _write(self, key: str, value: dict[str, Any]) -> KvReceipt:
        data = json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
        if len(data) > MAX_ROW_BYTES:
            raise SessionException(
                f"the row at {key!r} is {len(data)} bytes; a KV write is at most {MAX_ROW_BYTES}"
            )
        try:
            receipt = self._agent.set(key, data)
        except httpx.HTTPError as e:
            raise SessionException(f"writing {key!r} through the gateway failed: {e}") from e
        self.last_receipt = receipt
        if self._on_receipt is not None:
            self._on_receipt(key, receipt)
        if receipt.local_durability == "failed":
            logger.warning(
                "key=<%s> | applied on the gateway node, which could not make it durable: %s",
                key, receipt.local_durability_error,
            )
        if self._require_on_disk and not receipt.on_disk:
            raise SessionException(
                f"the write of {key!r} was applied but is not on disk on the gateway node "
                f"(local_durability={receipt.local_durability!r})"
            )
        return receipt

    def _absent_or_incomplete(self, what: str, key: str, under: str) -> None:
        """A record's row is absent: ``None`` if nothing of it exists, incomplete otherwise."""
        if self._keys(under):
            raise IncompleteSession(what, [key])

    # ── sessions ────────────────────────────────────────────────────────────

    def create_session(self, session: Session, **kwargs: Any) -> Session:
        key = self._session_key(session.session_id)
        if self._read(key) is not None:
            raise SessionException(f"Session {session.session_id} already exists")
        self._write(key, session.to_dict())
        return session

    def read_session(self, session_id: str, **kwargs: Any) -> Optional[Session]:
        key = self._session_key(session_id)
        row = self._read(key)
        if row is None:
            self._absent_or_incomplete(f"session {session_id!r}", key, self._session_root(session_id))
            return None
        return Session.from_dict(row)

    def delete_session(self, session_id: str, **kwargs: Any) -> None:
        """Tombstone every row of the session (each tombstone gossips like a write)."""
        keys = self._keys(self._session_root(session_id))
        if not keys:
            raise SessionException(f"Session {session_id} does not exist")
        for key in keys:
            try:
                self._agent.delete(key)
            except httpx.HTTPError as e:
                raise SessionException(f"deleting {key!r} through the gateway failed: {e}") from e
        for k in [k for k in self._counts if k[0] == session_id]:
            del self._counts[k]

    # ── agents ──────────────────────────────────────────────────────────────

    def create_agent(self, session_id: str, session_agent: SessionAgent, **kwargs: Any) -> None:
        agent_id = session_agent.agent_id
        self._write(self._agent_key(session_id, agent_id), session_agent.to_dict())
        self._write(self._count_key(session_id, agent_id), {"count": 0})
        self._counts[(session_id, agent_id)] = 0

    def read_agent(self, session_id: str, agent_id: str, **kwargs: Any) -> Optional[SessionAgent]:
        key = self._agent_key(session_id, agent_id)
        row = self._read(key)
        if row is None:
            self._absent_or_incomplete(f"agent {agent_id!r} of session {session_id!r}", key, key + "/")
            return None
        return SessionAgent.from_dict(row)

    def update_agent(self, session_id: str, session_agent: SessionAgent, **kwargs: Any) -> None:
        agent_id = session_agent.agent_id
        previous = self.read_agent(session_id, agent_id)
        if previous is None:
            raise SessionException(f"Agent {agent_id} in session {session_id} does not exist")
        session_agent.created_at = previous.created_at
        self._write(self._agent_key(session_id, agent_id), session_agent.to_dict())

    # ── messages ────────────────────────────────────────────────────────────

    def _stored_count(self, session_id: str, agent_id: str) -> Optional[int]:
        key = self._count_key(session_id, agent_id)
        row = self._read(key)
        if row is None:
            return None
        count = row.get("count")
        if not isinstance(count, int) or isinstance(count, bool) or count < 0:
            raise SessionException(f"the row at {key!r} is unreadable: count {count!r}")
        return count

    def create_message(
        self, session_id: str, agent_id: str, session_message: SessionMessage, **kwargs: Any
    ) -> None:
        index = session_message.message_id
        # The message first, then the count: on the gateway node the count never names a message
        # that is not there. Across nodes the two gossip independently — `list_messages` says so.
        self._write(self._message_key(session_id, agent_id, index), session_message.to_dict())
        known = self._counts.get((session_id, agent_id))
        if known is None:
            known = self._stored_count(session_id, agent_id) or 0
        if index + 1 > known:
            self._write(self._count_key(session_id, agent_id), {"count": index + 1})
            known = index + 1
        self._counts[(session_id, agent_id)] = known

    def read_message(
        self, session_id: str, agent_id: str, message_id: int, **kwargs: Any
    ) -> Optional[SessionMessage]:
        row = self._read(self._message_key(session_id, agent_id, message_id))
        return None if row is None else SessionMessage.from_dict(row)

    def update_message(
        self, session_id: str, agent_id: str, session_message: SessionMessage, **kwargs: Any
    ) -> None:
        index = session_message.message_id
        previous = self.read_message(session_id, agent_id, index)
        if previous is None:
            raise SessionException(f"Message {index} does not exist")
        session_message.created_at = previous.created_at
        self._write(self._message_key(session_id, agent_id, index), session_message.to_dict())

    def list_messages(
        self, session_id: str, agent_id: str, limit: Optional[int] = None, offset: int = 0, **kwargs: Any
    ) -> list[SessionMessage]:
        """The agent's messages from ``offset``, in order — all of them, or :class:`IncompleteSession`.

        Messages are written 0, 1, 2, … and never removed, so the conversation is every index below
        the larger of the stored count and one past the highest message seen. Any index in that
        range from ``offset`` that is not here yet makes the read incomplete. Returning the rows
        that are here would be worse than an error: Strands resumes by appending after the last
        message it was given, so it would overwrite the messages it was not.
        """
        count_key = self._count_key(session_id, agent_id)
        prefix = self._messages_prefix(session_id, agent_id)
        present: set[int] = set()
        for key in self._keys(prefix):
            tail = unquote(key[len(prefix):])
            if tail.isdigit():
                present.add(int(tail))
        count = self._stored_count(session_id, agent_id)
        if count is None:
            # `create_agent` writes the count with the agent, so its absence is a row in flight —
            # or no such agent, which Strands never asks to list.
            raise IncompleteSession(f"the messages of agent {agent_id!r} in session {session_id!r}", [count_key])
        end = max(count, max(present) + 1 if present else 0)
        start = max(offset, 0)
        wanted = list(range(start, end))
        if limit is not None:
            wanted = wanted[: max(limit, 0)]
        missing = [self._message_key(session_id, agent_id, i) for i in wanted if i not in present]
        if missing:
            raise IncompleteSession(f"the messages of agent {agent_id!r} in session {session_id!r}", missing)
        messages: list[SessionMessage] = []
        for i in wanted:
            key = self._message_key(session_id, agent_id, i)
            row = self._read(key)
            if row is None:  # listed a moment ago, tombstoned since
                raise IncompleteSession(f"the messages of agent {agent_id!r} in session {session_id!r}", [key])
            messages.append(SessionMessage.from_dict(row))
        return messages

    # ── multi-agent state (a swarm's or a graph's) ──────────────────────────

    def create_multi_agent(self, session_id: str, multi_agent: "MultiAgentBase", **kwargs: Any) -> None:
        self._write(self._multi_key(session_id, multi_agent.id), multi_agent.serialize_state())

    def read_multi_agent(self, session_id: str, multi_agent_id: str, **kwargs: Any) -> Optional[dict[str, Any]]:
        return self._read(self._multi_key(session_id, multi_agent_id))

    def update_multi_agent(self, session_id: str, multi_agent: "MultiAgentBase", **kwargs: Any) -> None:
        if self.read_multi_agent(session_id, multi_agent.id) is None:
            raise SessionException(f"MultiAgent state {multi_agent.id} in session {session_id} does not exist")
        self._write(self._multi_key(session_id, multi_agent.id), multi_agent.serialize_state())


class MyceliumSessionManager(RepositorySessionManager):
    """A Strands ``SessionManager`` whose session lives in a Mycelium fleet.

    Pass it as an ``Agent``'s (or a ``Swarm``'s / ``Graph``'s) ``session_manager``. Either hand it
    an SDK client (``agent=``) or the gateway's address; the bearer defaults to
    ``MYCELIUM_GATEWAY_TOKEN`` as the SDK's does.

    Reading the session happens here, at construction, as Strands' own managers do — so an
    incomplete session raises :class:`IncompleteSession` from the constructor.
    """

    def __init__(
        self,
        session_id: str,
        *,
        agent: Optional[MyceliumAgent] = None,
        host: str = "127.0.0.1",
        port: int = 7946,
        token: Optional[str] = None,
        scheme: str = "http",
        ca_file: Optional[str] = None,
        prefix: str = "strands",
        require_on_disk: bool = False,
        on_receipt: Optional[Callable[[str, KvReceipt], None]] = None,
        **kwargs: Any,
    ) -> None:
        self._owns_client = agent is None
        client = agent if agent is not None else MyceliumAgent(
            host, port, token=token, scheme=scheme, ca_file=ca_file
        )
        self.repository = MyceliumSessionRepository(
            client, prefix=prefix, require_on_disk=require_on_disk, on_receipt=on_receipt
        )
        super().__init__(session_id=session_id, session_repository=self.repository, **kwargs)

    @property
    def last_receipt(self) -> Optional[KvReceipt]:
        """The receipt of this manager's most recent write (rung 1 and the gateway node's disk)."""
        return self.repository.last_receipt

    def close(self) -> None:
        """Close the SDK client, if this manager created it."""
        if self._owns_client:
            self.repository._agent.close()
