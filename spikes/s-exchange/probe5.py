"""S-exchange probe, phase 5: the exchange's latency, 200 sequential exchanges on loopback after 20 warm-up ones,
each binding a fresh per-call value on the dynamic-scopes instance (or none on the default one)."""
import json, statistics, sys, time
exec(open(__file__.replace("probe5.py", "probe.py")).read().split("admin = call(")[0])
T = f"{BASE}/realms/{REALM}/protocol/openid-connect/token"
subject = call("POST", T, form={"grant_type": "password", "client_id": "user-app", "username": "alice", "password": "alice"})[1]["access_token"]
X = {"grant_type": "urn:ietf:params:oauth:grant-type:token-exchange", "subject_token": subject,
     "subject_token_type": "urn:ietf:params:oauth:token-type:access_token",
     "requested_token_type": "urn:ietf:params:oauth:token-type:access_token", "audience": "mycelium:fleet"}
samples = []
for i in range(220):
    extra = {"scope": f"cn:call-{i:06d}"} if DYNAMIC else {}
    t0 = time.perf_counter()
    s, r = call("POST", T, form={**X, **extra}, auth=("mycelium-gateway", "gw-secret"))
    dt = (time.perf_counter() - t0) * 1000
    assert s == 200, r
    if i >= 20:
        samples.append(dt)
samples.sort()
q = lambda p: round(samples[int(p * (len(samples) - 1))], 2)
out("exchange_latency_ms", n=len(samples), p50=q(0.5), p90=q(0.9), p99=q(0.99), max=round(samples[-1], 2))
