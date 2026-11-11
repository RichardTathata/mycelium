"""S-exchange probe, phase 4 (dynamic-scopes instance, after probe3): the subject token issued WITHOUT the dynamic
scope, from a client that has it as an optional scope — does the exchange bind a per-call value? And two
values in one request: is the token bound to exactly the one asked for?"""
import json, sys
exec(open(__file__.replace("probe4.py", "probe.py")).read().split("admin = call(")[0])
T = f"{BASE}/realms/{REALM}/protocol/openid-connect/token"
s, u = call("POST", T, form={"grant_type": "password", "client_id": "user-app", "username": "alice", "password": "alice"})
subject = u["access_token"]
out("subject_without_dynamic_scope", status=s, scope=claims(subject).get("scope"))
X = {"grant_type": "urn:ietf:params:oauth:grant-type:token-exchange", "subject_token": subject,
     "subject_token_type": "urn:ietf:params:oauth:token-type:access_token",
     "requested_token_type": "urn:ietf:params:oauth:token-type:access_token", "audience": "mycelium:fleet"}
for name, extra in [("exchange_binds_call_value", {"scope": "cn:call-0003"}),
                    ("exchange_binds_another_call_value", {"scope": "cn:call-0004"}),
                    ("exchange_two_values", {"scope": "cn:call-0005 cn:call-0006"})]:
    s, r = call("POST", T, form={**X, **extra}, auth=("mycelium-gateway", "gw-secret"))
    tok = r.get("access_token") if s == 200 else None
    out(name, status=s, scope=claims(tok).get("scope") if tok else r, bytes=len(tok) if tok else None, jti=claims(tok).get("jti") if tok else None)
