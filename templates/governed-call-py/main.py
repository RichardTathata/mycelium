#!/usr/bin/env python3
"""governed-call-py: the published node, a declared unit, a permitted write, and a write the gateway refuses."""
import os, subprocess, sys, time, urllib.request
import httpx
from mycelium import MyceliumAgent

BIN = os.environ.get("MYCELIUM_BIN", "mycelium")   # the published binary, on PATH or named here
PORT = 8300                                         # http_port in node.toml
os.makedirs("data", exist_ok=True)
log = open("data/node.log", "w")
try:
    node = subprocess.Popen([BIN, "-c", "node.toml"], stdout=log, stderr=subprocess.STDOUT)
except FileNotFoundError:
    sys.exit(f"no `{BIN}` found: put the published mycelium binary on PATH, or set MYCELIUM_BIN to its path")
try:
    for _ in range(120):                            # the gateway answers /health once it listens
        try:
            urllib.request.urlopen(f"http://127.0.0.1:{PORT}/health", timeout=1)
            break
        except Exception:
            time.sleep(0.5)
    else:
        sys.exit("the node never became healthy: see data/node.log")

    alice = MyceliumAgent("127.0.0.1", PORT, token="alice-token")
    unit = alice.declare_from("units/purchaser.toml")
    print(f"alice declared `{unit.principal}`: {unit.declared}")
    print(f"  not enforced by this node (it attaches no evaluator): {unit.not_enforced}")
    receipt = alice.set("demo/greeting", b"hello from alice")
    print(f"alice set demo/greeting: PERMITTED — receipt {receipt}")

    reader = MyceliumAgent("127.0.0.1", PORT, token="reader-token")
    print(f"reader get demo/greeting: {reader.get('demo/greeting')!r}")
    try:
        reader.set("demo/greeting", b"overwritten")
        sys.exit("the reader's write was not refused")
    except httpx.HTTPStatusError as e:
        body = e.response.json() if e.response.content else {}
        print(f"reader set demo/greeting: REFUSED {e.response.status_code} — {body.get('error', e.response.text)} "
              f"(the gateway's rule gateway.auth: the route needs {body.get('required_scope', '?')}, which the reader does not hold)")
    print("next: `mycelium doctor node.toml` — what start() enforced here, and what it did not")
finally:
    node.terminate()
    try:
        node.wait(timeout=10)
    except subprocess.TimeoutExpired:
        node.kill()
    log.close()
