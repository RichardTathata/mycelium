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
call("POST", f"{BASE}/admin/realms", {"realm": REALM, "enabled": True}, admin)
def client(body):
    s, _ = call("POST", f"{A}/clients", body, admin)
    cid = call("GET", f"{A}/clients?clientId={urllib.parse.quote(body['clientId'])}", token=admin)[1][0]["id"]
    return cid
aud_mapper = lambda name, aud: {"name": name, "protocol": "openid-connect", "protocolMapper": "oidc-audience-mapper",
    "config": {"included.client.audience": aud, "access.token.claim": "true", "id.token.claim": "false"}}
client({"clientId": "mycelium:fleet", "publicClient": False, "secret": "unused", "standardFlowEnabled": False, "directAccessGrantsEnabled": False})
client({"clientId": "user-app", "publicClient": True, "directAccessGrantsEnabled": True, "standardFlowEnabled": False,
        "protocolMappers": [aud_mapper("aud-gateway", "mycelium-gateway")]})
gw = client({"clientId": "mycelium-gateway", "publicClient": False, "secret": "gw-secret", "serviceAccountsEnabled": True,
        "standardFlowEnabled": False, "directAccessGrantsEnabled": False,
        "attributes": {"standard.token.exchange.enabled": "true"},
        "protocolMappers": [aud_mapper("aud-fleet", "mycelium:fleet")]})
call("POST", f"{A}/users", {"username": "alice", "enabled": True, "email": "alice@example.org", "emailVerified": True, "firstName": "Alice", "lastName": "Example",
     "credentials": [{"type": "password", "value": "alice", "temporary": False}]}, admin)

if DYNAMIC:
    s, _ = call("POST", f"{A}/client-scopes", {"name": "cn", "protocol": "openid-connect",
        "attributes": {"is.dynamic.scope": "true", "dynamic.scope.regexp": "cn:*", "include.in.token.scope": "true", "display.on.consent.screen": "false"}}, admin)
    sid = [c for c in call("GET", f"{A}/client-scopes", token=admin)[1] if c["name"] == "cn"][0]["id"]
    call("PUT", f"{A}/clients/{gw}/optional-client-scopes/{sid}", token=admin)

T = f"{BASE}/realms/{REALM}/protocol/openid-connect/token"
s, user = call("POST", T, form={"grant_type": "password", "client_id": "user-app", "username": "alice", "password": "alice"})
subject = user["access_token"]
out("subject_token", status=s, aud=claims(subject).get("aud"), azp=claims(subject).get("azp"), bytes=len(subject))
X = {"grant_type": "urn:ietf:params:oauth:grant-type:token-exchange", "subject_token": subject,
     "subject_token_type": "urn:ietf:params:oauth:token-type:access_token",
     "requested_token_type": "urn:ietf:params:oauth:token-type:access_token", "audience": "mycelium:fleet"}
CN = "0194f2a07c1e0000-3f9a5c1d2e4b6a70-17"

def exchange(name, extra=None):
    s, r = call("POST", T, form={**X, **(extra or {})}, auth=("mycelium-gateway", "gw-secret"))
    if s == 200:
        tok = r["access_token"]
        c = claims(tok)
        out(name, status=s, bytes=len(tok), aud=c.get("aud"), azp=c.get("azp"), scope=c.get("scope"), jti=c.get("jti"),
            lifetime_s=c["exp"] - c["iat"], nonce_anywhere=CN in json.dumps(c), claim_names=sorted(c))
        return tok
    out(name, status=s, error=r)
    return None

base = exchange("exchange_baseline")
again = exchange("exchange_again")
if base and again:
    out("two_exchanges_differ", jti_differs=claims(base)["jti"] != claims(again)["jti"])
exchange("requested_claim_param", {"cn": CN, "nonce": CN})
exchange("requested_claim_claims_param", {"claims": json.dumps({"access_token": {"cn": {"value": CN}}})})
gw_token = call("POST", T, form={"grant_type": "client_credentials"}, auth=("mycelium-gateway", "gw-secret"))[1]["access_token"]
exchange("actor_token", {"actor_token": gw_token, "actor_token_type": "urn:ietf:params:oauth:token-type:access_token"})
exchange("dynamic_scope" if DYNAMIC else "dynamic_scope_feature_off", {"scope": f"cn:{CN}"})
# Mapper types the server offers: does any read a value from the token request?
info = call("GET", f"{BASE}/admin/serverinfo", token=admin)[1]
types = info["protocolMapperTypes"]["openid-connect"]
out("mapper_types", ids=sorted(t["id"] for t in types))
if base:
    print("TOKEN " + base)
