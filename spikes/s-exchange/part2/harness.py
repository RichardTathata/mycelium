"""Spike S-exchange, part 2 (2026-11-11): sender-constrained exchanged tokens on Keycloak's current release.

Runs, against the image pinned below, every probe the note cites, and prints JSON lines (raw: tokens included —
a throwaway local realm). Needs Docker, openssl 3 and the `cryptography` package. Every container it starts is
stopped and removed on every exit path.

    python3 harness.py OUTDIR            # default features, then (if DPoP is off by default) --features=dpop
"""
import base64, hashlib, json, os, ssl, subprocess, sys, time, urllib.error, urllib.parse, urllib.request, uuid

# The current Keycloak release, pulled 2026-11-11 (`quay.io/keycloak/keycloak:latest`, built 2026-11-09).
IMG = "quay.io/keycloak/keycloak@sha256:723b1d6e254c0fc96d5cffec860c7e0744d6a3277822b865bb538cf891575e41"
NAME, PORT = "sx2-kc", 18443
OUT = sys.argv[1]
CERTS = os.path.join(OUT, "certs")
os.makedirs(CERTS, exist_ok=True)
RESULTS = open(os.path.join(OUT, "results.jsonl"), "a")

def out(probe, **kw):
    line = json.dumps({"probe": probe, **kw}, sort_keys=True)
    print(line); RESULTS.write(line + "\n"); RESULTS.flush()

def sh(*args, **kw):
    return subprocess.run(list(args), check=True, capture_output=True, text=True, **kw)

b64u = lambda b: base64.urlsafe_b64encode(b).rstrip(b"=").decode()

# ── Certificates and keys ────────────────────────────────────────────────────────────────────────────────────────
def certs():
    os.chdir(CERTS)
    if os.path.exists("trust.p12"):
        return
    o = lambda *a: sh("openssl", *a)
    o("req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes", "-keyout", "ca-ec.key", "-out", "ca-ec.crt", "-subj", "/CN=sx2 EC CA", "-days", "2")
    o("req", "-x509", "-newkey", "ed25519", "-nodes", "-keyout", "ca-ed.key", "-out", "ca-ed.crt", "-subj", "/CN=sx2 Ed25519 CA (the fleet CA's algorithm)", "-days", "2")
    o("req", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes", "-keyout", "server.key", "-out", "server.csr", "-subj", "/CN=localhost")
    open("server.ext", "w").write("subjectAltName=DNS:localhost,IP:127.0.0.1\nextendedKeyUsage=serverAuth\n")
    o("x509", "-req", "-in", "server.csr", "-CA", "ca-ec.crt", "-CAkey", "ca-ec.key", "-CAcreateserial", "-out", "server.crt", "-days", "2", "-extfile", "server.ext")
    open("client.ext", "w").write("extendedKeyUsage=clientAuth\n")
    o("req", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes", "-keyout", "gw-ec.key", "-out", "gw-ec.csr", "-subj", "/CN=mycelium-gateway")
    o("x509", "-req", "-in", "gw-ec.csr", "-CA", "ca-ec.crt", "-CAkey", "ca-ec.key", "-CAcreateserial", "-out", "gw-ec.crt", "-days", "2", "-extfile", "client.ext")
    o("req", "-newkey", "ed25519", "-nodes", "-keyout", "gw-ed.key", "-out", "gw-ed.csr", "-subj", "/CN=mycelium-gateway")
    o("x509", "-req", "-in", "gw-ed.csr", "-CA", "ca-ed.crt", "-CAkey", "ca-ed.key", "-CAcreateserial", "-out", "gw-ed.crt", "-days", "2", "-extfile", "client.ext")
    for alias, f in (("ca-ec", "ca-ec.crt"), ("ca-ed", "ca-ed.crt")):
        sh("docker", "run", "--rm", "-v", f"{CERTS}:/certs", "--entrypoint", "keytool", IMG, "-importcert", "-noprompt",
           "-alias", alias, "-file", f"/certs/{f}", "-keystore", "/certs/trust.p12", "-storetype", "PKCS12", "-storepass", "changeit")
    for f in os.listdir("."):
        os.chmod(f, 0o644)

# ── Keycloak ─────────────────────────────────────────────────────────────────────────────────────────────────────
def start(features=None):
    subprocess.run(["docker", "rm", "-f", NAME], capture_output=True)
    args = ["docker", "run", "-d", "--name", NAME, "-p", f"127.0.0.1:{PORT}:8443", "-v", f"{CERTS}:/certs:ro",
            "-e", "KC_BOOTSTRAP_ADMIN_USERNAME=admin", "-e", "KC_BOOTSTRAP_ADMIN_PASSWORD=admin", IMG, "start-dev",
            "--https-certificate-file=/certs/server.crt", "--https-certificate-key-file=/certs/server.key",
            "--https-trust-store-file=/certs/trust.p12", "--https-trust-store-password=changeit", "--https-client-auth=request"]
    if features:
        args.append(f"--features={features}")
    sh(*args)
    for _ in range(120):
        try:
            urllib.request.urlopen(f"https://127.0.0.1:{PORT}/realms/master", context=ctx())
            return
        except Exception:
            time.sleep(2)
    raise SystemExit("keycloak did not start")

def stop():
    logs = subprocess.run(["docker", "logs", NAME], capture_output=True, text=True)
    subprocess.run(["docker", "rm", "-f", NAME], capture_output=True)
    return (logs.stdout + logs.stderr).splitlines()

def ctx(client=None):
    c = ssl.create_default_context(cafile=os.path.join(CERTS, "ca-ec.crt"))
    # Python 3.13 sets VERIFY_X509_STRICT, which refuses these throwaway certificates (no authority key identifier);
    # the chain, the hostname and the expiry are still verified.
    c.verify_flags &= ~ssl.VERIFY_X509_STRICT
    if client:
        c.load_cert_chain(os.path.join(CERTS, f"{client}.crt"), os.path.join(CERTS, f"{client}.key"))
    return c

BASE = f"https://127.0.0.1:{PORT}"
def call(method, url, body=None, token=None, form=None, auth=None, headers=None, client=None):
    h = dict(headers or {})
    data = None
    if form is not None:
        data = urllib.parse.urlencode(form).encode(); h["Content-Type"] = "application/x-www-form-urlencoded"
    elif body is not None:
        data = json.dumps(body).encode(); h["Content-Type"] = "application/json"
    if token: h["Authorization"] = f"Bearer {token}"
    if auth: h["Authorization"] = "Basic " + base64.b64encode(f"{auth[0]}:{auth[1]}".encode()).decode()
    req = urllib.request.Request(url, data=data, method=method, headers=h)
    try:
        with urllib.request.urlopen(req, context=ctx(client)) as r:
            raw = r.read(); return r.status, (json.loads(raw) if raw else None)
    except urllib.error.HTTPError as e:
        raw = e.read()
        try: return e.code, json.loads(raw)
        except Exception: return e.code, raw.decode(errors="replace")

def claims(jwt):
    p = jwt.split(".")[1]
    return json.loads(base64.urlsafe_b64decode(p + "=" * (-len(p) % 4)))

# ── The realm ────────────────────────────────────────────────────────────────────────────────────────────────────
REALM = "fleet"
T = f"{BASE}/realms/{REALM}/protocol/openid-connect/token"
def admin():
    return call("POST", f"{BASE}/realms/master/protocol/openid-connect/token", form={"grant_type": "password", "client_id": "admin-cli", "username": "admin", "password": "admin"})[1]["access_token"]

def realm(adm):
    A = f"{BASE}/admin/realms/{REALM}"
    call("POST", f"{BASE}/admin/realms", {"realm": REALM, "enabled": True}, adm)
    aud = lambda n, a: {"name": n, "protocol": "openid-connect", "protocolMapper": "oidc-audience-mapper", "config": {"included.client.audience": a, "access.token.claim": "true"}}
    call("POST", f"{A}/clients", {"clientId": "mycelium:fleet", "publicClient": False, "secret": "unused", "standardFlowEnabled": False}, adm)
    call("POST", f"{A}/clients", {"clientId": "user-app", "publicClient": True, "directAccessGrantsEnabled": True, "standardFlowEnabled": False, "protocolMappers": [aud("aud-gateway", "mycelium-gateway")]}, adm)
    call("POST", f"{A}/clients", {"clientId": "mycelium-gateway", "publicClient": False, "secret": "gw-secret", "serviceAccountsEnabled": True, "standardFlowEnabled": False,
         "attributes": {"standard.token.exchange.enabled": "true"}, "protocolMappers": [aud("aud-fleet", "mycelium:fleet")]}, adm)
    call("POST", f"{A}/users", {"username": "alice", "enabled": True, "email": "alice@example.org", "emailVerified": True, "firstName": "Alice", "lastName": "Example",
         "credentials": [{"type": "password", "value": "alice", "temporary": False}]}, adm)

def gateway_attrs(adm, **attrs):
    A = f"{BASE}/admin/realms/{REALM}"
    c = call("GET", f"{A}/clients?clientId=mycelium-gateway", token=adm)[1][0]
    c.setdefault("attributes", {}).update({k.replace("__", "."): v for k, v in attrs.items()})
    s, r = call("PUT", f"{A}/clients/{c['id']}", c, adm)
    return s

def subject():
    return call("POST", T, form={"grant_type": "password", "client_id": "user-app", "username": "alice", "password": "alice"})[1]["access_token"]

X = lambda subj: {"grant_type": "urn:ietf:params:oauth:grant-type:token-exchange", "subject_token": subj,
                  "subject_token_type": "urn:ietf:params:oauth:token-type:access_token",
                  "requested_token_type": "urn:ietf:params:oauth:token-type:access_token", "audience": "mycelium:fleet"}

def report(name, s, r, **kw):
    if s == 200:
        tok = r.get("access_token")
        c = claims(tok) if tok and tok.count(".") == 2 else {}
        out(name, status=s, token_type=r.get("token_type"), issued_token_type=r.get("issued_token_type"), cnf=c.get("cnf"),
            bytes=len(tok) if tok else None, jti=c.get("jti"), azp=c.get("azp"), token=tok, **kw)
    else:
        out(name, status=s, error=r, **kw)

# ── DPoP (RFC 9449) ──────────────────────────────────────────────────────────────────────────────────────────────
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec, ed25519
from cryptography.hazmat.primitives.asymmetric.utils import decode_dss_signature
DPOP_EC = ec.generate_private_key(ec.SECP256R1())
DPOP_ED = ed25519.Ed25519PrivateKey.generate()

def jwk_of(key):
    if isinstance(key, ec.EllipticCurvePrivateKey):
        n = key.public_key().public_numbers()
        return {"kty": "EC", "crv": "P-256", "x": b64u(n.x.to_bytes(32, "big")), "y": b64u(n.y.to_bytes(32, "big"))}, "ES256"
    raw = key.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
    return {"kty": "OKP", "crv": "Ed25519", "x": b64u(raw)}, "EdDSA"

def thumbprint(jwk):  # RFC 7638: the required members, lexicographic, no whitespace
    req = {"EC": ("crv", "kty", "x", "y"), "OKP": ("crv", "kty", "x")}[jwk["kty"]]
    return b64u(hashlib.sha256(json.dumps({k: jwk[k] for k in req}, separators=(",", ":"), sort_keys=True).encode()).digest())

def dpop(key, htu, htm="POST"):
    jwk, alg = jwk_of(key)
    head = b64u(json.dumps({"typ": "dpop+jwt", "alg": alg, "jwk": jwk}).encode())
    body = b64u(json.dumps({"jti": str(uuid.uuid4()), "htm": htm, "htu": htu, "iat": int(time.time())}).encode())
    si = f"{head}.{body}".encode()
    if alg == "ES256":
        r, s = decode_dss_signature(key.sign(si, ec.ECDSA(hashes.SHA256())))
        sig = r.to_bytes(32, "big") + s.to_bytes(32, "big")
    else:
        sig = key.sign(si)
    return f"{head}.{body}.{b64u(sig)}"

def dpop_probes(label):
    subj = subject()
    for key in (DPOP_EC, DPOP_ED):
        jwk, alg = jwk_of(key)
        s, r = call("POST", T, form=X(subj), auth=("mycelium-gateway", "gw-secret"), headers={"DPoP": dpop(key, T)})
        expected = thumbprint(jwk)
        kw = {"alg": alg, "expected_jkt": expected}
        if s == 200:
            kw["jkt_matches"] = (claims(r["access_token"]).get("cnf") or {}).get("jkt") == expected
        report(f"{label}_dpop_{alg}", s, r, **kw)

# ── mTLS certificate-bound tokens (RFC 8705) ─────────────────────────────────────────────────────────────────────
def cert_thumb(name):
    der = ssl.PEM_cert_to_DER_cert(open(os.path.join(CERTS, f"{name}.crt")).read())
    return b64u(hashlib.sha256(der).digest())

def mtls_probes(label, adm):
    s = gateway_attrs(adm, tls__client__certificate__bound__access__tokens="true")
    out(f"{label}_mtls_client_setting", status=s, attribute="tls.client.certificate.bound.access.tokens")
    subj = subject()
    for client in ("gw-ec", "gw-ed"):
        try:
            s, r = call("POST", T, form=X(subj), auth=("mycelium-gateway", "gw-secret"), client=client)
        except Exception as e:  # the TLS handshake itself
            out(f"{label}_mtls_{client}", status=None, error=f"{type(e).__name__}: {e}")
            continue
        kw = {"expected_x5t_s256": cert_thumb(client)}
        if s == 200:
            kw["x5t_matches"] = (claims(r["access_token"]).get("cnf") or {}).get("x5t#S256") == kw["expected_x5t_s256"]
        report(f"{label}_mtls_{client}", s, r, **kw)
    s, r = call("POST", T, form=X(subj), auth=("mycelium-gateway", "gw-secret"))
    report(f"{label}_mtls_without_a_client_certificate", s, r)
    gateway_attrs(adm, tls__client__certificate__bound__access__tokens="false")

# ── The routes the first review listed as not run ────────────────────────────────────────────────────────────────
def other_routes(label, adm):
    subj = subject()
    CN = "0194f2a07c1e0000-3f9a5c1d2e4b6a70-17"
    s, r = call("POST", T, form={**X(subj), "resource": f"urn:mycelium:call:{CN}"}, auth=("mycelium-gateway", "gw-secret"))
    report(f"{label}_resource_param", s, r, nonce_anywhere=(CN in json.dumps(claims(r["access_token"])) if s == 200 else None))
    s, r = call("POST", T, form={**X(subj), "requested_token_type": "urn:ietf:params:oauth:token-type:refresh_token"}, auth=("mycelium-gateway", "gw-secret"))
    report(f"{label}_requested_refresh_token", s, r)
    gateway_attrs(adm, client__use__lightweight__access__token__enabled="true")
    s, r = call("POST", T, form=X(subj), auth=("mycelium-gateway", "gw-secret"))
    report(f"{label}_lightweight_access_token", s, r, claim_names=sorted(claims(r["access_token"])) if s == 200 else None)
    gateway_attrs(adm, client__use__lightweight__access__token__enabled="false")

def survey(label, adm):
    info = call("GET", f"{BASE}/admin/serverinfo", token=adm)[1]
    feats = info.get("features") or info.get("profileInfo", {})
    out(f"{label}_serverinfo", version=info["systemInfo"]["version"],
        features=[{k: f.get(k) for k in ("name", "type", "enabled")} for f in feats] if isinstance(feats, list) else feats)
    # A survey, not a run: the mapper types this server offers (none was attached here but in part 1's probes).
    out(f"{label}_mapper_type_survey", ids=sorted(t["id"] for t in info["protocolMapperTypes"]["openid-connect"]))

def phase(label, features=None):
    try:
        start(features)
        adm = admin()
        realm(adm)
        survey(label, adm)
        report(f"{label}_baseline_exchange", *call("POST", T, form=X(subject()), auth=("mycelium-gateway", "gw-secret")))
        dpop_probes(label)
        mtls_probes(label, adm)
        other_routes(label, adm)
    finally:
        logs = stop()
        open(os.path.join(OUT, f"{label}.log"), "w").write("\n".join(logs) + "\n")
        out(f"{label}_feature_log_lines", lines=[l for l in logs if "eature" in l])

try:
    certs()
    out("image", image=IMG)
    phase("default")
    phase("dpop_feature", "dpop")
finally:
    subprocess.run(["docker", "rm", "-f", NAME], capture_output=True)
