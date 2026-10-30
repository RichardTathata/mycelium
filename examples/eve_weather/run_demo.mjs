// Eve's tiny example, the weather assistant, run unchanged on a three-node fleet (adopter-surface Y6).
//
//   npm ci --prefix examples/eve_weather            # eve 0.80.0, pinned, beside the agent directory
//   npm ci --prefix mycelium-ts && npm run build --prefix mycelium-ts
//   cargo build --example eve_fleet_node --features compliance,llm
//   cargo build --bin mycelium --features cli,tls,compliance,llm
//   node examples/eve_weather/run_demo.mjs           # Node 24 or later — Eve 0.80.0's own floor
//
// What it does, in order, each step printing one line the CI job greps:
//   1. checks the three files of `agent/` are byte for byte the pinned package's README example;
//   2. loads the directory with mycelium-ts's loader and serves its tool as an MCP server;
//   3. starts three nodes; the third bridges that server, so the tool is served from it;
//   4. declares the agent through the first node's gateway — the unit and the instructions as a prompt skill;
//   5. calls `get_weather` through the first node's `/mcp` as the forecaster (permitted) and as a visitor (refused);
//   6. explains the refusal from the deciding gateway's evidence journal (`mycelium explain --correlation`);
//   7. runs `mycelium test --faults grant` over the agent's unit with a grant-required rule on the tool: default-deny,
//      the declared issuer's grant admitting one call, and the grant holding across a restart.
//
// Binaries: EVE_FLEET_NODE (default target/debug/examples/eve_fleet_node) and MYCELIUM_BIN (default
// target/debug/mycelium), both relative to the repository root unless absolute; CARGO_TARGET_DIR is honoured.

import { spawn, execFileSync } from "node:child_process";
import { createRequire } from "node:module";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "..", "..");
const require = createRequire(import.meta.url);
const sdk = require(path.join(repo, "mycelium-ts", "dist", "index.js"));

const target = process.env.CARGO_TARGET_DIR ?? path.join(repo, "target");
const nodeBin = path.resolve(repo, process.env.EVE_FLEET_NODE ?? path.join(target, "debug", "examples", "eve_fleet_node"));
const myceliumBin = path.resolve(repo, process.env.MYCELIUM_BIN ?? path.join(target, "debug", "mycelium"));
const base = Number(process.env.EVE_FLEET_BASE_PORT ?? 9450);
const work = fs.mkdtempSync(path.join(os.tmpdir(), "eve-fleet-"));
const children = [];

function fail(message) {
  console.error(`FAILED: ${message}`);
  for (const c of children) c.kill("SIGTERM");
  process.exit(1);
}
process.on("unhandledRejection", (e) => fail(e instanceof Error ? e.stack : String(e)));
process.on("uncaughtException", (e) => fail(e instanceof Error ? e.stack : String(e)));
process.on("exit", () => { for (const c of children) c.kill("SIGTERM"); });

// ── 1. The directory is Eve's own example, unmodified ───────────────────────
const agentDir = path.join(here, "agent");
const readme = fs.readFileSync(path.join(here, "node_modules", "eve", "README.md"), "utf8");
const tiny = readme.split("## Tiny Example")[1].split("## Quick Start")[0];
const blocks = [...tiny.matchAll(/`(agent\/[^`]+)`\s*\n\s*```[a-z]*\n([\s\S]*?)```/g)];
if (blocks.length !== 3) fail(`the pinned README's tiny example has ${blocks.length} files, expected 3`);
for (const [, rel, body] of blocks) {
  if (fs.readFileSync(path.join(here, rel), "utf8") !== body) fail(`${rel} differs from eve ${sdk.EVE_PINNED_VERSION}'s README`);
}
const onDisk = fs.readdirSync(agentDir, { recursive: true, withFileTypes: true }).filter((d) => d.isFile()).length;
if (onDisk !== blocks.length) fail(`agent/ holds ${onDisk} files; the README's example has ${blocks.length}`);
console.log(`unchanged: agent/ is eve ${sdk.EVE_PINNED_VERSION}'s README example, ${blocks.map((b) => b[1]).join(", ")}`);

// ── 2. Load it, serve its tool ───────────────────────────────────────────────
const agent = await sdk.loadEveAgent(agentDir);
console.log(`loaded: ${agent.name} (eve ${agent.eveVersion}) — tools ${agent.tools.map((t) => t.name).join(",")}; skills ${agent.skills.map((s) => s.name).join(",")}; model ${agent.model} read, not routed`);
const server = await sdk.serveEveTools(agent);

// ── 3. Three nodes; the third serves the tool ────────────────────────────────
const node = (i) => ({ gossip: base + i, http: base + 10 + i });
const tokens = [
  "weather:weather-token:cap:write+llm:write+llm:read",
  "forecaster:forecaster-token:mcp:invoke",
  "visitor:visitor-token:mcp:invoke",
].join(",");
// The fleet CA: EVE_FLEET_CA names one made beforehand (`eve_fleet_node ca-init <dir>`, as the CI job does); else one
// is made here.
const ca = process.env.EVE_FLEET_CA ?? path.join(work, "ca");
if (!process.env.EVE_FLEET_CA) execFileSync(nodeBin, ["ca-init", ca], { stdio: "inherit" });
const provider = `127.0.0.1:${node(3).gossip}`;
const ready = [];
for (const i of [1, 2, 3]) {
  const env = {
    ...process.env,
    NAME: `eve-${i}`,
    DATA_DIR: path.join(work, `node-${i}`),
    CA_DIR: ca,
    PORT: String(node(i).gossip),
    HTTP_PORT: String(node(i).http),
    PEERS: i === 1 ? "" : `127.0.0.1:${node(1).gossip}`,
    TOKENS: tokens,
    PROVIDER: provider,
    ...(i === 3 ? { EVE_MCP_URL: server.url } : {}),
    RUST_LOG: process.env.RUST_LOG ?? "warn",
  };
  const child = spawn(nodeBin, [], { env, stdio: ["ignore", "pipe", "pipe"] });
  children.push(child);
  const log = fs.createWriteStream(path.join(work, `node-${i}.log`));
  child.stderr.pipe(log);
  ready.push(
    new Promise((resolve, reject) => {
      let out = "";
      child.stdout.on("data", (d) => {
        out += d;
        if (out.includes(`ready eve-${i}`)) resolve();
      });
      child.on("exit", (code) => reject(new Error(`node ${i} exited ${code}; its log: ${path.join(work, `node-${i}.log`)}`)));
    }),
  );
}
let readyTimer;
await Promise.race([
  Promise.all(ready),
  new Promise((_, r) => (readyTimer = setTimeout(() => r(new Error("the fleet was not ready in 120 s")), 120_000))),
]).finally(() => clearTimeout(readyTimer)); // cleared, so the run does not wait out the bound after the fleet is up
console.log(`fleet: three nodes ready; get_weather served from ${provider}`);

// ── 4. Declare the agent: its unit, its instructions as a prompt skill ──────
const gw = node(1).http;
const declared = await sdk.declareEveAgent(agent, {
  mycelium: new sdk.MyceliumAgent("127.0.0.1", gw, 30_000, { token: "weather-token" }),
  prompts: new sdk.PromptSkillClient("127.0.0.1", gw, 30_000, { token: "weather-token" }),
});
const stored = await new sdk.PromptSkillClient("127.0.0.1", gw, 30_000, { token: "weather-token" }).get(agent.name, "instructions");
if (stored?.system !== agent.skills[0].text) fail("the instructions did not land as the prompt skill's system prompt");
console.log(`declared: unit ${declared.unit.handleId ?? "(handle)"} — not enforced by this node: ${declared.unit.notEnforced?.join(", ") || "nothing"}; prompt skills ${declared.skills.map((s) => s.join("/")).join(",")}`);

// ── 5. The tool through /mcp: permitted, then refused ────────────────────────
async function mcp(token, method, params) {
  const resp = await fetch(`http://127.0.0.1:${gw}/mcp`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
  });
  return { status: resp.status, body: await resp.json().catch(() => null) };
}
const listed = await mcp("forecaster-token", "tools/list", {});
if (!listed.body?.result?.tools?.some((t) => t.name === "get_weather")) fail(`tools/list did not list get_weather: ${JSON.stringify(listed)}`);
const call = (token, correlation) =>
  mcp(token, "tools/call", { name: "get_weather", arguments: { city: "Leeds" }, _meta: { correlation } });

const permitted = await call("forecaster-token", "eve-permitted");
const text = permitted.body?.result?.content?.[0]?.text ?? "";
if (permitted.status !== 200 || !text.includes("Sunny")) fail(`the forecaster's call: ${JSON.stringify(permitted)}`);
console.log(`forecaster get_weather: PERMITTED — ${text}`);

const refused = await call("visitor-token", "eve-refused");
const error = refused.body?.error;
if (!error || (refused.body?.result && !refused.body.result.isError)) fail(`the visitor's call was not refused: ${JSON.stringify(refused)}`);
console.log(`visitor get_weather: REFUSED — ${error.message}`);

// ── 6. The refusal, from the records ─────────────────────────────────────────
const journal = path.join(work, "node-1", "evidence.log");
const explained = execFileSync(myceliumBin, ["explain", "--correlation", "eve-refused", journal], { encoding: "utf8" });
process.stdout.write(explained.split("\n").map((l) => `  ${l}`).join("\n") + "\n");
// The policy names no allowance for the visitor, so authority is not established: the reference evaluator answers
// *indeterminate* — never a permit for a call it has no rule for — and the gateway refuses it; nothing ran.
if (!/token:fleet\/visitor tools\/call tool:get_weather@\S+ → indeterminate, execution none/.test(explained)) {
  fail("the evidence journal does not explain the refusal");
}
const permittedRecord = execFileSync(myceliumBin, ["explain", "--correlation", "eve-permitted", journal], { encoding: "utf8" });
if (!/token:fleet\/forecaster tools\/call tool:get_weather@\S+ → permit/.test(permittedRecord)) fail(`the evidence journal has no record of the permitted call:\n${permittedRecord}`);
console.log("explained: the visitor's call refused at the gateway — authority not established, execution none — and the forecaster's permitted, each on the deciding gateway's record");

for (const c of children) c.kill("SIGTERM");
await server.close();

// ── 7. A grant on the tool, across a restart: X3's fault class on this agent's unit ──
const project = path.join(work, "project");
fs.mkdirSync(path.join(project, "units"), { recursive: true });
fs.writeFileSync(
  path.join(project, "units", `${agent.name}.toml`),
  sdk.eveUnitToml(agent, { principal: agent.name }) + "\n" + fs.readFileSync(path.join(here, "grant-rules.toml"), "utf8"),
);
fs.writeFileSync(
  path.join(project, "grant.toml"),
  [
    'profile = "dev"',
    'bind_address = "127.0.0.1"',
    `bind_port = ${base + 20}`,
    "",
    "[persistence]",
    'base_path = "data/grant-persist"',
    "",
    "[tls]",
    'auto_cert_dir = "data/grant-tls"',
    "",
  ].join("\n"),
);
const grant = (() => {
  try {
    return execFileSync(myceliumBin, ["test", "--faults", "grant", "units", "--config", "grant.toml"], { cwd: project, encoding: "utf8", stdio: ["ignore", "pipe", "inherit"] });
  } catch (e) {
    return String(e.stdout ?? "") + `\n(exit ${e.status})`;
  }
})();
process.stdout.write(grant.split("\n").map((l) => `  ${l}`).join("\n") + "\n");
for (const want of ["grant: passed", "✓ a grant-required tool is default-deny", "✓ the grant holds across a restart"]) {
  if (!grant.includes(want)) fail(`the grant class on ${agent.name}'s unit did not report ${JSON.stringify(want)}`);
}
console.log(`grant: get_weather's grant holds across a restart on ${agent.name}'s unit (mycelium test --faults grant)`);
console.log("EVE FLEET OK");
fs.rmSync(work, { recursive: true, force: true });
