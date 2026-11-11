"""S-exchange probe: Keycloak's standard token exchange (RFC 8693) and binding a per-call value into the
exchanged token. Run against a Keycloak at BASE (the default-feature instance, and one started with
--features=dynamic-scopes). Prints JSON lines: one per probe."""
import base64, json, sys, urllib.parse, urllib.request, urllib.error

BASE = sys.argv[1]
DYNAMIC = len(sys.argv) > 2 and sys.argv[2] == "dynamic"
REALM = "fleet"

def call(method, url, body=None, token=None, form=None, auth=None):
    headers = {}
    data = None
    if form is not None:
        data = urllib.parse.urlencode(form).encode()
        headers["Content-Type"] = "application/x-www-form-urlencoded"
    elif body is not None:
        data = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    if token:
        headers["Authorization"] = f"Bearer {token}"
    if auth:
        headers["Authorization"] = "Basic " + base64.b64encode(f"{auth[0]}:{auth[1]}".encode()).decode()
    req = urllib.request.Request(url, data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(req) as r:
            raw = r.read()
            return r.status, (json.loads(raw) if raw else None)
    except urllib.error.HTTPError as e:
        raw = e.read()
        try:
            return e.code, json.loads(raw)
        except Exception:
            return e.code, raw.decode(errors="replace")

def claims(jwt):
    p = jwt.split(".")[1]
    return json.loads(base64.urlsafe_b64decode(p + "=" * (-len(p) % 4)))

def out(probe, **kw):
    print(json.dumps({"probe": probe, **kw}, sort_keys=True))


admin = call("POST", f"{BASE}/realms/master/protocol/openid-connect/token", form={"grant_type": "password", "client_id": "admin-cli", "username": "admin", "password": "admin"})[1]["access_token"]
A = f"{BASE}/admin/realms/{REALM}"
gw = call("GET", f"{A}/clients?clientId=mycelium-gateway", token=admin)[1][0]["id"]
def mapper(name, kind, config):
    s, r = call("POST", f"{A}/clients/{gw}/protocol-mappers/models", {"name": name, "protocol": "openid-connect", "protocolMapper": kind, "config": config}, admin)
    out("add_mapper", name=name, status=s, error=r if s >= 300 else None)
mapper("claims-param", "oidc-claims-param-token-mapper", {"access.token.claim": "true", "id.token.claim": "true", "userinfo.token.claim": "true"})
mapper("claims-param-idtoken", "oidc-claims-param-value-idtoken-mapper", {"id.token.claim": "true"})
mapper("session-note-cn", "oidc-usersessionmodel-note-mapper", {"user.session.note": "client_request_param_cn", "claim.name": "cn", "jsonType.label": "String", "access.token.claim": "true", "id.token.claim": "true"})
mapper("session-note-cn-bare", "oidc-usersessionmodel-note-mapper", {"user.session.note": "cn", "claim.name": "cn_bare", "jsonType.label": "String", "access.token.claim": "true", "id.token.claim": "true"})
T = f"{BASE}/realms/{REALM}/protocol/openid-connect/token"
subject = call("POST", T, form={"grant_type": "password", "client_id": "user-app", "username": "alice", "password": "alice"})[1]["access_token"]
X = {"grant_type": "urn:ietf:params:oauth:grant-type:token-exchange", "subject_token": subject,
     "subject_token_type": "urn:ietf:params:oauth:token-type:access_token",
     "requested_token_type": "urn:ietf:params:oauth:token-type:access_token", "audience": "mycelium:fleet"}
CN = "0194f2a07c1e0000-3f9a5c1d2e4b6a70-17"
def exchange(name, extra=None):
    s, r = call("POST", T, form={**X, **(extra or {})}, auth=("mycelium-gateway", "gw-secret"))
    if s == 200:
        tok = r.get("access_token") or r.get("id_token")
        c = claims(tok)
        out(name, status=s, issued_token_type=r.get("issued_token_type"), bytes=len(tok), nonce_anywhere=CN in json.dumps(c),
            extra_claims=sorted(set(c) - {"acr","aud","azp","email","email_verified","exp","family_name","given_name","iat","iss","jti","name","preferred_username","realm_access","resource_access","scope","sid","sub","typ"}))
        return tok
    out(name, status=s, error=r)
claims_param = json.dumps({"access_token": {"cn": {"value": CN}}, "id_token": {"cn": {"value": CN}}})
exchange("claims_param_access_token", {"claims": claims_param, "cn": CN})
exchange("claims_param_id_token", {"claims": claims_param, "cn": CN, "requested_token_type": "urn:ietf:params:oauth:token-type:id_token"})
exchange("standard_claim_via_claims_param", {"claims": json.dumps({"access_token": {"nickname": {"value": CN}}})})
