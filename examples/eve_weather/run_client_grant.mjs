// Identity-and-lifecycle I1's exit: Eve's weather tool (adopter-surface Y6) called through `/mcp` under a grant held by
// a service, `token:fleet/forecaster`, on a three-node fleet whose gateway is the `mycelium` binary.
//
//   npm ci --prefix examples/eve_weather
//   npm ci --prefix mycelium-ts && npm run build --prefix mycelium-ts
//   cargo build --release --bin mycelium --features cli,tls,compliance,a2a,llm,metrics   # the release's NODE_FEATURES
//   cargo build --release --example eve_fleet_node --features compliance,llm
//   MYCELIUM_BIN=target/release/mycelium EVE_FLEET_NODE=target/release/examples/eve_fleet_node \
//     node examples/eve_weather/run_client_grant.mjs                                       # ends with CLIENT GRANT OK
//
// The fleet: node 1 is the `mycelium` binary — the gateway — with `[authority]` over a unit that makes `get_weather`
// grant-required for the forecaster and lets the treasurer issue and revoke; `gateway_identity_issuer = "fleet"`;
// `[persistence]`. Node 2 is the binary as a plain member. Node 3 serves the tool: the binary hosts no tool of its own,
// so the provider is `eve_fleet_node`, which bridges the loader's MCP server (as `run_demo.mjs` runs it).
//
// The service: a client certificate from `mycelium tls issue --client forecaster --issuer fleet`, published at
// `POST /gateway/identity/client-cert`; `mycelium-ts`'s `McpClient` with a `ClientSigner` over its key, so every call
// carries `cs`/`cn` and a grant is presented with a possession proof.
//
// Each step prints one line; the run ends with CLIENT GRANT OK and a JSON summary of the measured steps.

import { spawn, execFileSync } from "node:child_process";
import { createRequire } from "node:module";
import { generateKeyPairSync } from "node:crypto";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "..", "..");
const require = createRequire(import.meta.url);
const sdk = require(path.join(repo, "mycelium-ts", "dist", "index.js"));

const target = process.env.CARGO_TARGET_DIR ?? path.join(repo, "target");
const profile = process.env.EVE_PROFILE ?? "debug";
const nodeBin = path.resolve(repo, process.env.EVE_FLEET_NODE ?? path.join(target, profile, "examples", "eve_fleet_node"));
const myceliumBin = path.resolve(repo, process.env.MYCELIUM_BIN ?? path.join(target, profile, "mycelium"));
const base = Number(process.env.CLIENT_GRANT_BASE_PORT ?? 9550);
const work = fs.mkdtempSync(path.join(os.tmpdir(), "client-grant-"));
const children = new Map();
const measured = { date: new Date().toISOString().slice(0, 10), mycelium: myceliumBin, version: "", steps: {} };

function fail(message) {
  console.error(`FAILED: ${message}\n(work directory: ${work})`);
  for (const c of children.values()) c.kill("SIGTERM");
  process.exit(1);
}
process.on("unhandledRejection", (e) => fail(e instanceof Error ? e.stack : String(e)));
process.on("exit", () => { for (const c of children.values()) c.kill("SIGTERM"); });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function until(what, cond, ms = 60_000) {
  const deadline = Date.now() + ms;
  for (;;) {
    try { if (await cond()) return; } catch { /* not yet */ }
    if (Date.now() > deadline) fail(`${what} within ${ms} ms`);
    await sleep(100);
  }
}
async function timed(name, fn) {
  const t = Date.now();
  const out = await fn();
  measured.steps[name] = Date.now() - t;
  return out;
}

measured.version = execFileSync(myceliumBin, ["--version"], { encoding: "utf8" }).trim();
const node = (i) => ({ gossip: base + i, http: base + 10 + i });
const gw = `http://127.0.0.1:${node(1).http}`;
const provider = `127.0.0.1:${node(3).gossip}`;
const resource = `tool:get_weather@${provider}`;

// ── The fleet CA, the unit, the configurations ──────────────────────────────
const ca = path.join(work, "ca");
execFileSync(nodeBin, ["ca-init", ca], { stdio: "inherit" });
const units = path.join(work, "units");
fs.mkdirSync(units, { recursive: true });
fs.writeFileSync(path.join(units, "weather.toml"), [
  'principal = "weather"',
  "",
  "# The forecaster may call the tool only under a grant; the treasurer issues and revokes. The tool is served by",
  "# node 3, so the resource names it (a bare `tool:` resource in a unit means this node).",
  "[[rule]]", 'actor = "token:fleet/forecaster"', 'operation = "tools/call"', `resource = "${resource}"`, "requires_grant = true", "",
  "[[rule]]", 'actor = "token:fleet/treasurer"', 'operation = "grant.issue"', `resource = "${resource}"`, "",
  "[[rule]]", 'actor = "token:fleet/treasurer"', 'operation = "grant.revoke"', `resource = "${resource}"`, "",
].join("\n"));
function nodeToml(i, { authority }) {
  const dir = path.join(work, `node-${i}`);
  fs.mkdirSync(path.join(dir, "tls"), { recursive: true });
  for (const f of ["ca-cert.pem", "ca-key.pem"]) fs.copyFileSync(path.join(ca, f), path.join(dir, "tls", f));
  const lines = [
    'profile = "dev"',
    'bind_address = "127.0.0.1"',
    `bind_port = ${node(i).gossip}`,
    `http_port = ${node(i).http}`,
    'gateway_identity_issuer = "fleet"',
    "reconnect_backoff_secs = 1",
    i === 1 ? "bootstrap_peers = []" : `bootstrap_peers = ["127.0.0.1:${node(1).gossip}"]`,
    "",
  ];
  for (const [name, scopes] of [["forecaster", ["mcp:invoke"]], ["treasurer", ["grant:read", "grant:issue"]], ["publisher", ["identity:write"]], ["prober", ["kv:read", "kv:write"]]]) {
    lines.push("[[gateway_named_tokens]]", `name = "${name}"`, `token = "${name}-token"`, `scopes = [${scopes.map((s) => `"${s}"`).join(", ")}]`, "");
  }
  lines.push("[tls]", `auto_cert_dir = "${path.join(dir, "tls")}"`, "", "[persistence]", `base_path = "${path.join(dir, "kv")}"`, "");
  if (authority) lines.push("[authority]", `declarations = "${units}"`, 'unit = "weather"', `journal = "${path.join(dir, "evidence.log")}"`, "");
  const file = path.join(dir, "node.toml");
  fs.writeFileSync(file, lines.join("\n"));
  return file;
}
const configs = { 1: nodeToml(1, { authority: true }), 2: nodeToml(2, { authority: false }) };

function startBinary(i) {
  const child = spawn(myceliumBin, ["-c", configs[i]], { env: { ...process.env, RUST_LOG: process.env.RUST_LOG ?? "warn" }, stdio: ["ignore", "pipe", "pipe"] });
  const log = fs.createWriteStream(path.join(work, `node-${i}.log`), { flags: "a" });
  child.stdout.pipe(log);
  child.stderr.pipe(log);
  children.set(i, child);
  return child;
}
const readyAt = (i) => until(`node ${i} ready`, async () => (await fetch(`http://127.0.0.1:${node(i).http}/ready`)).ok);
// The gateway dispatches to the provider over the mesh, so a call is sent only once a write at the gateway reaches the
// provider. `/stats`'s peer count is not enough: after the gateway's restart it reads 2 while the gateway's writers to
// its peers are still being re-established, and a call sent then times out (measured: the restart's window below).
let probes = 0;
async function meshed() {
  const key = `probe/${++probes}`;
  const at1 = new sdk.MyceliumAgent("127.0.0.1", node(1).http, 5_000, { token: "prober-token" });
  const at3 = new sdk.MyceliumAgent("127.0.0.1", node(3).http, 5_000, { token: "prober-token" });
  await until(`a write at node 1 reaching node 3 (${key})`, async () => {
    await at1.set(key, Buffer.from(String(Date.now())));
    return (await at3.get(key)) !== null;
  }, 120_000);
  // And back: the provider's reply travels on its own writer to the gateway.
  await until(`a write at node 3 reaching node 1 (${key}/back)`, async () => {
    await at3.set(`${key}/back`, Buffer.from(String(Date.now())));
    return (await at1.get(`${key}/back`)) !== null;
  }, 120_000);
}

// ── The tool's provider: Eve's agent, loaded and served, bridged by node 3 ───
const agent = await sdk.loadEveAgent(path.join(here, "agent"));
const server = await sdk.serveEveTools(agent);
startBinary(1);
await readyAt(1);
startBinary(2);
const eve = spawn(nodeBin, [], {
  env: {
    ...process.env, NAME: "eve-3", DATA_DIR: path.join(work, "node-3"), CA_DIR: ca, PORT: String(node(3).gossip),
    HTTP_PORT: String(node(3).http), PEERS: `127.0.0.1:${node(1).gossip}`, PROVIDER: provider, EVE_MCP_URL: server.url,
    TOKENS: "forecaster:forecaster-token:mcp:invoke,prober:prober-token:kv:read+kv:write", RUST_LOG: process.env.RUST_LOG ?? "warn",
  },
  stdio: ["ignore", "pipe", "pipe"],
});
children.set(3, eve);
const eveLog = fs.createWriteStream(path.join(work, "node-3.log"));
eve.stderr.pipe(eveLog);
eve.stdout.pipe(eveLog);
await new Promise((resolve, reject) => {
  let out = "";
  eve.stdout.on("data", (d) => { out += d; if (out.includes("ready eve-3")) resolve(); });
  eve.on("exit", (code) => reject(new Error(`node 3 exited ${code}`)));
});
await readyAt(2);
const plain = new sdk.McpClient(gw, { token: "forecaster-token" });
await until("node 1 lists get_weather", async () => (await plain.listTools()).some((t) => t.name === "get_weather"));
await meshed();
console.log(`fleet: ${measured.version} — node 1 the gateway with [authority], node 2 a member, get_weather served from ${provider}`);

// ── The service's certificate, published ─────────────────────────────────────
const clientDir = path.join(work, "client");
function issueClient(force) {
  execFileSync(myceliumBin, ["tls", "issue", "--ca-dir", ca, "--client", "forecaster", "--issuer", "fleet", "--out", clientDir, ...(force ? ["--force"] : [])], { stdio: "ignore" });
  return {
    cert: fs.readFileSync(path.join(clientDir, "client-fleet-forecaster.cert.pem"), "utf8"),
    key: fs.readFileSync(path.join(clientDir, "client-fleet-forecaster.key.pem"), "utf8"),
  };
}
async function publish(cert) {
  const resp = await fetch(`${gw}/gateway/identity/client-cert`, {
    method: "POST", headers: { "content-type": "application/json", authorization: "Bearer publisher-token" }, body: JSON.stringify({ cert_pem: cert }),
  });
  const body = await resp.json();
  if (!resp.ok) fail(`publishing the client certificate: ${resp.status} ${JSON.stringify(body)}`);
  return body;
}
const first = issueClient(false);
const published = await publish(first.cert);
await sleep(2_000); // the record gossips to the provider, which checks `cs` at its receive boundary
const signer = new sdk.ClientSigner(first.key, "token:fleet/forecaster");
const mcp = new sdk.McpClient(gw, { token: "forecaster-token", signer });
const supervisor = new sdk.MyceliumAgent("127.0.0.1", node(1).http, 30_000, { token: "treasurer-token" });
console.log(`published: ${published.principal}, key ${published.key.slice(0, 16)}…`);

async function refusal(call) {
  try { await call(); } catch (e) { return e; }
  fail("the call was not refused");
}
const reason = (e) => e?.data?.reason ?? e?.reason ?? e?.message;
async function issued(args) {
  const asked = await mcp.requestGrant("get_weather", args);
  await until(`request ${asked.request} pending at the supervisor`, async () => (await supervisor.grantsPending()).some((r) => r.request === asked.request));
  const row = (await supervisor.grantsPending()).find((r) => r.request === asked.request);
  await supervisor.grantDecide(row.agent, row.request, "issue");
  const answer = await mcp.waitForGrant(asked.request, { timeoutMs: 30_000 });
  return { asked, row, grant: answer.grant };
}
const text = (result) => result?.content?.[0]?.text ?? JSON.stringify(result);

// ── Without a grant, refused; under the forecaster's grant, it runs ─────────
const leeds = { city: "Leeds" };
const without = await timed("refused_without_a_grant_ms", () => refusal(() => mcp.callTool("get_weather", leeds)));
console.log(`without a grant: REFUSED — ${reason(without)}`);
const g1 = await timed("request_issue_read_ms", () => issued(leeds));
const ran = await timed("granted_call_ms", () => mcp.callTool("get_weather", leeds, { grant: g1.grant }));
if (!text(ran).includes("Sunny")) fail(`the granted call: ${JSON.stringify(ran)}`);
console.log(`under token:fleet/forecaster's grant: PERMITTED — ${text(ran)}`);
const again = await refusal(() => mcp.callTool("get_weather", leeds, { grant: g1.grant }));
console.log(`the same grant again: REFUSED — ${reason(again)}`);

// ── A foreign key's signature, refused at the gateway's own check ───────────
const foreign = new sdk.ClientSigner(generateKeyPairSync("ed25519").privateKey.export({ format: "pem", type: "pkcs8" }), "token:fleet/forecaster");
const foreignRefused = await refusal(() => new sdk.McpClient(gw, { token: "forecaster-token", signer: foreign }).callTool("get_weather", leeds));
if (reason(foreignRefused) !== "client_key_unknown") fail(`a foreign key: ${reason(foreignRefused)}`);
console.log(`a foreign key's cs: REFUSED — ${reason(foreignRefused)}`);

// ── A grant across a restart of the gateway ──────────────────────────────────
const paris = { city: "Paris" };
const g2 = await issued(paris);
const restarted = Date.now();
const gwChild = children.get(1);
const exited = new Promise((r) => gwChild.on("exit", r));
gwChild.kill("SIGTERM");
await exited;
startBinary(1);
await readyAt(1);
await until("node 1 lists get_weather after its restart", async () => (await plain.listTools()).some((t) => t.name === "get_weather"));
await meshed();
const afterRestart = await mcp.callTool("get_weather", paris, { grant: g2.grant });
measured.steps.restart_to_granted_call_ms = Date.now() - restarted;
if (!text(afterRestart).includes("Sunny")) fail(`the grant after the restart: ${JSON.stringify(afterRestart)}`);
console.log(`a grant issued before the gateway's restart, presented after it: PERMITTED — ${text(afterRestart)}`);

// ── Revoked, refused ─────────────────────────────────────────────────────────
const york = { city: "York" };
const g3 = await issued(york);
await supervisor.grantRevoke(g3.row.agent, g3.row.request, { reason: "exit run" });
const revoked = await refusal(() => mcp.callTool("get_weather", york, { grant: g3.grant }));
if (!String(revoked.message).includes("revoked") && !JSON.stringify(revoked.data ?? {}).includes("revoked")) fail(`the revoked grant: ${revoked.message}`);
console.log(`a revoked grant: REFUSED — ${revoked.message.slice(0, 120)}`);

// ── A newer certificate supersedes the older key ─────────────────────────────
await sleep(5);
const second = issueClient(true);
await publish(second.cert);
await sleep(2_000);
const superseded = await refusal(() => mcp.callTool("get_weather", leeds));
if (reason(superseded) !== "client_key_superseded") fail(`the superseded key: ${reason(superseded)}`);
console.log(`the older key after a newer certificate: REFUSED — ${reason(superseded)}`);
const newMcp = new sdk.McpClient(gw, { token: "forecaster-token", signer: new sdk.ClientSigner(second.key, "token:fleet/forecaster") });
const madrid = { city: "Madrid" };
const asked4 = await newMcp.requestGrant("get_weather", madrid);
await until("request 4 pending", async () => (await supervisor.grantsPending()).some((r) => r.request === asked4.request));
const row4 = (await supervisor.grantsPending()).find((r) => r.request === asked4.request);
await supervisor.grantDecide(row4.agent, row4.request, "issue");
const g4 = await newMcp.waitForGrant(asked4.request, { timeoutMs: 30_000 });
const ranNew = await newMcp.callTool("get_weather", madrid, { grant: g4.grant });
if (!text(ranNew).includes("Sunny")) fail(`the current key's call: ${JSON.stringify(ranNew)}`);
console.log(`the current key, under its own grant: PERMITTED — ${text(ranNew)}`);

for (const c of children.values()) c.kill("SIGTERM");
await server.close();
console.log(`measured: ${JSON.stringify(measured)}`);
console.log("CLIENT GRANT OK");
process.exit(0);
