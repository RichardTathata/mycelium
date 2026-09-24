"""Shared test helpers for overlay integration scenarios."""

from __future__ import annotations

import base64
import os
import time
import uuid
import httpx


NODE_A_HOST    = os.environ.get("NODE_A_HOST",    "overlay-a")
NODE_B_HOST    = os.environ.get("NODE_B_HOST",    "overlay-b")
NODE_C_HOST    = os.environ.get("NODE_C_HOST",    "overlay-c")
NODE_HTTP_PORT = int(os.environ.get("NODE_HTTP_PORT", "8300"))

ALL_HOSTS = [NODE_A_HOST, NODE_B_HOST, NODE_C_HOST]


def node_url(host: str) -> str:
    return f"http://{host}:{NODE_HTTP_PORT}"


def wait_for_health(host: str, timeout: int = 60) -> None:
    url = f"{node_url(host)}/health"
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            r = httpx.get(url, timeout=3.0)
            if r.status_code == 200:
                return
        except Exception:
            pass
        time.sleep(1.0)
    raise RuntimeError(f"Node {host} did not become healthy within {timeout}s")


def wait_for_cluster_ready(hosts: list[str] | None = None, timeout: int = 60) -> None:
    """Wait until gossip has *currently* converged across all overlay nodes.

    Writes a **freshly-nonced** sentinel from each node, checks the write was
    accepted, then polls until every node reads every sentinel back with the
    exact expected value.

    The nonce is the point. This helper used to write the same fixed keys
    (``test/cluster-ready/{host}``) on every call and then check only that
    *enough keys existed* under the prefix. Once those keys had propagated,
    every later call passed — on evidence from the first one — even if
    connectivity had since degraded. A readiness check that cannot fail after
    its first success is not a precondition, it is a decoration.

    Note this proves *fresh gossip propagation*, which is a much better
    precondition than the old version and is still **not** proof of consensus
    safety — see ``docs/wiki/dev/.log/2026-09-24-consensus-vote-binding.md``.
    """
    if hosts is None:
        hosts = ALL_HOSTS

    nonce = f"{time.time_ns():x}-{uuid.uuid4().hex[:8]}"
    sentinel_prefix = f"test/cluster-ready/{nonce}/"
    expected_values = {host: f"{host}:{nonce}" for host in hosts}

    # Step 1: write one freshly-nonced sentinel per node, and check it was accepted.
    for host in hosts:
        sentinel_key = f"{sentinel_prefix}{host}"
        with httpx.Client(base_url=node_url(host), timeout=5.0) as c:
            # NOTE the field name: the gateway reads `value_b64`, and if it is missing it
            # writes an EMPTY value and still answers `{"ok": true}`. The previous version of
            # this helper sent `value`, so every sentinel it ever wrote was empty — and the
            # check passed anyway, because it only counted keys. See
            # `docs/wiki/dev/.log/2026-09-24-consensus-vote-binding.md` §the gateway write.
            payload = base64.b64encode(expected_values[host].encode()).decode()
            resp = c.post("/gateway/kv", json={"key": sentinel_key, "value_b64": payload})
            if resp.status_code >= 400:
                raise RuntimeError(
                    f"readiness sentinel write rejected by {host}: "
                    f"HTTP {resp.status_code} {resp.text[:200]}"
                )

    # Step 2: wait until every node reads back every sentinel with the EXACT expected
    # value — not merely "enough keys exist under the prefix". A count can be satisfied
    # by the wrong keys, or by keys from an earlier run; identity and value cannot.
    def _reads_all(host: str) -> bool:
        for owner, want in expected_values.items():
            key = f"{sentinel_prefix}{owner}".replace("/", "%2F")
            # `GET /gateway/kv?key=` — not `/gateway/kv/get`, which does not exist and
            # answers 404 (a shape that reads as "no value" if you do not check the status).
            r = httpx.get(f"{node_url(host)}/gateway/kv?key={key}", timeout=3.0)
            if r.status_code >= 400:
                return False
            body = r.json()
            if not body.get("found"):
                return False
            got = base64.b64decode(body.get("value_b64", "")).decode(errors="replace")
            if got != want:
                return False
        return True

    deadline = time.monotonic() + timeout
    last_error: str | None = None
    while time.monotonic() < deadline:
        try:
            if all(_reads_all(h) for h in hosts):
                return
            last_error = "sentinels not yet visible with expected values on every node"
        except Exception as exc:  # transient transport errors are expected while converging
            last_error = f"{type(exc).__name__}: {exc}"
        time.sleep(1.0)
    raise RuntimeError(
        f"Cluster did not converge (fresh sentinel {nonce}) within {timeout}s: {last_error}"
    )


def wait_for_ready(host: str, timeout: int = 60) -> None:
    """Wait for a single node to be healthy (alias kept for callsite compatibility)."""
    wait_for_health(host, timeout)


def poll_until(condition_fn, timeout: int = 30, interval: float = 0.5) -> bool:
    """Return True if condition_fn() returns True before timeout, else False."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if condition_fn():
                return True
        except Exception:
            pass
        time.sleep(interval)
    return False


def assert_eq(actual, expected, msg: str = "") -> None:
    if actual != expected:
        prefix = f"{msg}: " if msg else ""
        raise AssertionError(f"{prefix}expected {expected!r}, got {actual!r}")


def assert_ge(actual: int, minimum: int, msg: str = "") -> None:
    if actual < minimum:
        prefix = f"{msg}: " if msg else ""
        raise AssertionError(f"{prefix}expected >= {minimum}, got {actual}")
