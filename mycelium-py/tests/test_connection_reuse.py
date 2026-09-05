"""Connection-reuse regression gate (no Mycelium node needed).

The bridge once opened a fresh TCP connection per call, which exhausts macOS
ephemeral ports at Group-scale write rates (~16k rapid KV calls, found by a
downstream project 2026-08-18). These tests drive the public client surface
against a local keep-alive stub server that counts *distinct TCP connections*:
with the pooled persistent client, hundreds of calls must ride a handful of
connections. Before the fix this count equalled the call count.
"""

import asyncio
import base64
import json
import threading
import time

# How long the stub parks a `stage == "park"` take. Long enough that the parked-takes
# test can *poll* the connection count to its plateau on a slow runner before any take
# returns (a fixed 1 s sample read 88/120 on a hosted runner, 2026-09-04).
PARK_SECS = 5.0
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

from mycelium import MyceliumAgent
from mycelium.prompt_skill import PromptSkillClient
from mycelium.tuple import TupleSpace
from mycelium.wiki import Wiki

_PAYLOAD_B64 = base64.b64encode(b"x").decode()


class _CountingHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"  # keep-alive

    def _reply(self, body: dict) -> None:
        data = json.dumps(body).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self) -> None:  # noqa: N802
        self._reply({"found": False, "keys": []})

    def do_POST(self) -> None:  # noqa: N802
        length = int(self.headers.get("Content-Length", 0))
        raw = self.rfile.read(length)
        if self.path.startswith("/gateway/tuple/take"):
            try:
                stage = json.loads(raw or b"{}").get("stage")
            except ValueError:
                stage = None
            if stage == "park":
                time.sleep(PARK_SECS)  # emulate a server-side parked long-poll
            self._reply({"id": 1, "payload_b64": _PAYLOAD_B64})
        elif self.path == "/gateway/kv/quorum":
            self._reply({"ok": True, "acks_received": 1})
        else:
            self._reply({"ok": True, "page": None})

    def log_message(self, *_: object) -> None:  # quiet
        pass


class _CountingServer(ThreadingHTTPServer):
    daemon_threads = True
    # `socketserver.TCPServer` listens with a backlog of 5. The parked-takes test opens
    # 120 connections at once; on a loaded CI runner the accept loop falls behind, the
    # kernel resets connections past the backlog, and httpx surfaces `ReadError` /
    # the stub logs `BrokenPipeError` — a flake of the *stub*, not the pool (seen
    # 2026-09-05 on #183 and once on main). The real gateway listens with 1024.
    request_queue_size = 256

    def __init__(self, *a, **kw):
        super().__init__(*a, **kw)
        self.connections = 0
        self._lock = threading.Lock()

    def get_request(self):
        sock, addr = super().get_request()
        with self._lock:
            self.connections += 1
        return sock, addr


@pytest.fixture
def stub():
    server = _CountingServer(("127.0.0.1", 0), _CountingHandler)
    t = threading.Thread(target=server.serve_forever, daemon=True)
    t.start()
    try:
        yield server
    finally:
        server.shutdown()


def test_sync_kv_calls_reuse_connections(stub):
    port = stub.server_address[1]
    with MyceliumAgent("127.0.0.1", port) as agent:
        for i in range(200):
            agent.get(f"k/{i}")
            agent.set(f"k/{i}", b"v")
        # The quorum write carries a per-call timeout — it must still ride the
        # shared client, not build one per call (the 360-review finding).
        for _ in range(50):
            agent.set_with_min_acks("k/q", b"v", min_acks=1, timeout_secs=5.0)
    assert stub.connections <= 5, (
        f"450 KV calls used {stub.connections} TCP connections — "
        "the per-call-client regression is back (ephemeral-port exhaustion)"
    )


def test_tuple_take_loop_reuses_connections(stub):
    """take() is the worker hot loop: with items available it returns
    immediately, so a fresh client per take is one connection per item —
    the exact regression at the rates the fix shipped for."""
    port = stub.server_address[1]
    ts = TupleSpace("127.0.0.1", port)

    async def worker():
        for _ in range(100):
            item_id, payload = await ts.take("stage-a", timeout_secs=5.0)
            assert (item_id, payload) == (1, b"x")
            await ts.take_by_key("stage-b", "k", timeout_secs=5.0)

    asyncio.run(worker())
    assert stub.connections <= 5, (
        f"200 take calls used {stub.connections} TCP connections"
    )


def test_parked_takes_are_not_capped_by_the_pool(stub):
    """A worker fleet with >100 tasks parked on take() against ONE handle must
    have every take parked at the server, not queued behind the client pool
    (httpx's default cap is 100 connections; the per-call clients the pool
    replaced never capped concurrency). While the takes are parked, the
    connection count must reach 120 — under the default limits it plateaus at
    exactly 100 until the first take returns. Structural: poll the count until
    it reaches 120 or the park window is nearly over, never a fixed sample."""
    port = stub.server_address[1]
    ts = TupleSpace("127.0.0.1", port)

    async def fleet():
        takes = [
            asyncio.create_task(ts.take("park", timeout_secs=10.0)) for _ in range(120)
        ]
        deadline = time.monotonic() + PARK_SECS - 0.5  # before any parked take returns
        peak = 0
        while time.monotonic() < deadline:
            peak = max(peak, stub.connections)
            if peak >= 120:
                break
            await asyncio.sleep(0.05)
        await asyncio.gather(*takes)
        return peak

    parked = asyncio.run(fleet())
    assert parked >= 120, (
        f"only {parked} of 120 parked takes held a connection during the park window — "
        "the pool is capping long-poll concurrency"
    )


def test_async_calls_reuse_connections_across_event_loops(stub):
    port = stub.server_address[1]
    wiki = Wiki("127.0.0.1", port)

    async def burst():
        for _ in range(50):
            await wiki.read("page")

    # Two separate asyncio.run() loops against one handle: the pool must both
    # reuse connections within a loop AND survive the loop change.
    asyncio.run(burst())
    asyncio.run(burst())
    assert stub.connections <= 6, (
        f"100 async reads across two loops used {stub.connections} connections"
    )


def test_prompt_skill_handle_survives_separate_asyncio_runs(stub):
    """PromptSkillClient (and ReasonClient, same shape) used to build one eager
    AsyncClient in __init__, bound to whichever loop first used it — a second
    asyncio.run() against the same handle then failed on the dead loop's
    connections. Pooled, the handle works across loops."""
    port = stub.server_address[1]
    client = PromptSkillClient("127.0.0.1", port)
    asyncio.run(client.list())
    asyncio.run(client.list())  # pre-fix: RuntimeError("Event loop is closed")


def test_pool_evicts_only_closed_loops_never_live_siblings(stub):
    """Deterministic pin of the pool's eviction rule: a miss may evict entries
    whose loop has CLOSED, and must never evict a live sibling loop's client.
    (The eviction rule that popped everything-that-isn't-me degraded every
    borrow to a fresh client whenever two threads ran loops concurrently.)"""
    from mycelium._pool import ClientPool

    port = stub.server_address[1]
    pool = ClientPool(f"http://127.0.0.1:{port}", 5.0)

    async def borrow():
        async with pool.asy() as c:
            await c.get("/gateway/kv/get", params={"key": "k"})

    loop_a = asyncio.new_event_loop()
    loop_b = asyncio.new_event_loop()
    try:
        loop_a.run_until_complete(borrow())  # registers A
        loop_b.run_until_complete(borrow())  # miss on B: A is live — keep it
        assert len(pool._async_clients) == 2, (
            "a live sibling loop's client was evicted on a pool miss"
        )
        loop_a.close()  # A is now genuinely dead
        loop_c = asyncio.new_event_loop()
        try:
            loop_c.run_until_complete(borrow())  # miss on C: evicts A, keeps B
            keys = set(pool._async_clients)
            assert id(loop_a) not in keys, "closed loop's client not evicted"
            assert id(loop_b) in keys, "live loop B's client evicted"
            assert id(loop_c) in keys
        finally:
            loop_c.run_until_complete(pool.aclose())
            loop_c.close()
    finally:
        if not loop_b.is_closed():
            loop_b.run_until_complete(pool.aclose())
            loop_b.close()
        if not loop_a.is_closed():
            loop_a.close()


def test_concurrent_threads_each_with_a_live_loop_keep_their_clients(stub):
    """Concurrency smoke: two threads, each running its own event loop, against
    ONE handle at the same time — no errors, bounded connections. This is NOT
    the gate for the eviction rule (the buggy evict-anything-that-isn't-me
    version self-damped under real scheduling and passed this test); the
    deterministic gate is test_pool_evicts_only_closed_loops_never_live_siblings."""
    port = stub.server_address[1]
    wiki = Wiki("127.0.0.1", port)
    errors: list[BaseException] = []
    # Stagger the starts: the second loop's first borrow (a pool miss) must
    # land while the first loop already holds a live client — that is the
    # sequence where evict-anything-that-isn't-me destroys a live sibling.
    first_client_ready = threading.Event()

    def run_loop(leader: bool):
        async def burst():
            if leader:
                await wiki.read("page")
                first_client_ready.set()
            else:
                first_client_ready.wait()
            for _ in range(50):
                await wiki.read("page")

        try:
            asyncio.run(burst())
        except BaseException as e:  # surface into the test thread
            errors.append(e)

    threads = [
        threading.Thread(target=run_loop, args=(leader,)) for leader in (True, False)
    ]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert not errors, f"worker thread failed: {errors[0]!r}"
    assert stub.connections <= 8, (
        f"100 reads on two concurrent loops used {stub.connections} connections — "
        "the pool is evicting live sibling loops' clients"
    )
