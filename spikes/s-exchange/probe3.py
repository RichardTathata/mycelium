"""S-exchange probe, phase 3 (dynamic-scopes instance only): is the dynamic scope configured right — does a
client_credentials grant carry it — and does an exchange carry one the subject token already holds?"""
import json, sys
exec(open(__file__.replace("probe3.py", "probe.py")).read().split("admin = call(")[0])
admin = call("POST", f"{BASE}/realms/master/protocol/openid-connect/token", form={"grant_type": "password", "client_id": "admin-cli", "username": "admin", "password": "admin"})[1]["access_token"]
A = f"{BASE}/admin/realms/{REALM}"
T = f"{BASE}/realms/{REALM}/protocol/openid-connect/token"
CN = "0194f2a07c1e0000-3f9a5c1d2e4b6a70-17"
s, r = call("POST", T, form={"grant_type": "client_credentials", "scope": f"cn:{CN}"}, auth=("mycelium-gateway", "gw-secret"))
out("client_credentials_dynamic_scope", status=s, scope=claims(r["access_token"]).get("scope") if s == 200 else r)
# The dynamic scope also optional on the user's client: the subject token is issued holding it.
ua = call("GET", f"{A}/clients?clientId=user-app", token=admin)[1][0]["id"]
sid = [c for c in call("GET", f"{A}/client-scopes", token=admin)[1] if c["name"] == "cn"][0]["id"]
call("PUT", f"{A}/clients/{ua}/optional-client-scopes/{sid}", token=admin)
s, u = call("POST", T, form={"grant_type": "password", "client_id": "user-app", "username": "alice", "password": "alice", "scope": f"openid cn:{CN}"})
subject = u["access_token"]
out("subject_with_dynamic_scope", status=s, scope=claims(subject).get("scope"))
X = {"grant_type": "urn:ietf:params:oauth:grant-type:token-exchange", "subject_token": subject,
     "subject_token_type": "urn:ietf:params:oauth:token-type:access_token",
     "requested_token_type": "urn:ietf:params:oauth:token-type:access_token", "audience": "mycelium:fleet"}
for name, extra in [("exchange_keeps_subjects_dynamic_scope", {"scope": f"cn:{CN}"}),
                    ("exchange_new_dynamic_scope_value", {"scope": "cn:another-call-0002"}),
                    ("exchange_no_scope_param", {})]:
    s, r = call("POST", T, form={**X, **extra}, auth=("mycelium-gateway", "gw-secret"))
    out(name, status=s, scope=claims(r["access_token"]).get("scope") if s == 200 else r)
