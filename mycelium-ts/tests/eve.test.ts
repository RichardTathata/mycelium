/**
 * The Eve loader (adopter-surface Y6): the README's slot table is the loader's, every slot the loader does not load
 * is refused by name, and the tools are served as MCP answers. The directories are written per test under the OS
 * temp directory; tool and `agent.ts` modules are imported with jest's own `require` (ts-jest compiles them), so no
 * `eve` package is needed here — the real one runs in the `eve-fleet` CI job.
 */
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

import {
  EVE_PINNED_VERSION,
  EVE_SLOTS,
  EveRefusedError,
  answer,
  classifyEveDirectory,
  declareEveAgent,
  eveUnitToml,
  installedEveVersion,
  loadEveAgent,
  serveEveTools,
  type EveAgent,
  type LoadOptions,
} from "../src/eve";

const README = path.join(__dirname, "..", "..", "examples", "eve_weather", "README.md");

const dirs: string[] = [];
afterAll(() => {
  for (const d of dirs) fs.rmSync(d, { recursive: true, force: true });
});

/** A fresh `<tmp>/<project>/agent/` holding `files` (a path ending in `/` is a directory). */
function agentDir(files: Record<string, string>): string {
  const project = fs.mkdtempSync(path.join(os.tmpdir(), "eve-loader-"));
  dirs.push(project);
  const dir = path.join(project, "agent");
  fs.mkdirSync(dir);
  for (const [rel, body] of Object.entries(files)) {
    const at = path.join(dir, rel);
    if (rel.endsWith("/")) {
      fs.mkdirSync(at, { recursive: true });
    } else {
      fs.mkdirSync(path.dirname(at), { recursive: true });
      fs.writeFileSync(at, body);
    }
  }
  return dir;
}

/** Load with jest's `require` and the pinned version, unless a test overrides either. */
const opts = (more: LoadOptions = {}): LoadOptions => ({
  name: "weather",
  importModule: async (file) => require(file),
  resolveEveVersion: () => EVE_PINNED_VERSION,
  ...more,
});

/** A Standard Schema that states itself as JSON Schema: one required string field. */
const SCHEMA = `{
  "~standard": {
    version: 1,
    vendor: "test",
    validate: (v: any) => (v && typeof v.city === "string" ? { value: v } : { issues: [{ message: "expected string", path: ["city"] }] }),
    jsonSchema: { input: () => ({ type: "object", properties: { city: { type: "string" } }, required: ["city"] }) },
  },
}`;
const WEATHER_TOOL = `export default {
  description: "Get the current weather for a city.",
  inputSchema: ${SCHEMA},
  async execute(input: any) { return { city: input.city, condition: "Sunny", temperatureF: 72 }; },
};
`;
const WEATHER = {
  "instructions.md": "You are a weather-focused assistant.\n",
  "agent.ts": 'export default { model: "openai/gpt-5.4-mini" };\n',
  "tools/get_weather.ts": WEATHER_TOOL,
};

async function refusal(dir: string, more: LoadOptions = {}): Promise<EveRefusedError> {
  try {
    await loadEveAgent(dir, opts(more));
  } catch (e) {
    expect(e).toBeInstanceOf(EveRefusedError);
    return e as EveRefusedError;
  }
  throw new Error(`${dir} was loaded, not refused`);
}

test("the README's table is the loader's: every slot, its status and what it is on the fleet", () => {
  const rows = [...fs.readFileSync(README, "utf8").matchAll(/^\| `([^`]+)` \| ([a-z ]+) \| (.+) \|$/gm)].map((m) => ({
    slot: m[1],
    status: m[2],
    onTheFleet: m[3],
  }));
  expect(rows).toEqual(EVE_SLOTS.map((s) => ({ slot: s.slot, status: s.status, onTheFleet: s.onTheFleet })));
});

test("a directory holding every slot is named slot by slot, an unknown entry as unknown", () => {
  const files: Record<string, string> = { "notes/": "" };
  for (const s of EVE_SLOTS) files[s.slot] = s.slot.endsWith("/") ? "" : "x";
  const entries = classifyEveDirectory(agentDir(files));
  for (const s of EVE_SLOTS) {
    expect(entries.find((e) => e.entry === s.slot)?.status).toBe(s.status);
  }
  expect(entries.find((e) => e.entry === "notes/")?.status).toBe("unknown");
  expect(entries).toHaveLength(EVE_SLOTS.length + 1);
});

test("every slot the loader does not load is refused by name, never skipped", async () => {
  const refusedSlots = EVE_SLOTS.filter((s) => s.status !== "loaded");
  expect(refusedSlots.map((s) => s.slot)).toEqual(expect.arrayContaining(["schedules/", "sandbox/", "sandbox.ts", "hooks/", "subagents/"]));
  for (const s of refusedSlots) {
    const e = await refusal(agentDir({ ...WEATHER, [s.slot]: s.slot.endsWith("/") ? "" : "x" }));
    expect(e.refused.map((r) => r.entry)).toEqual([s.slot]);
    expect(e.refused[0].status).toBe(s.status);
    expect(e.message).toContain(s.slot);
  }
});

test("schedules/ is refused by name, and the refusal names Y12", async () => {
  const e = await refusal(agentDir({ ...WEATHER, "schedules/daily.md": "---\ncron: 0 9 * * *\n---\nSummarise.\n" }));
  expect(e.message).toContain("schedules/ (not built: decided (Y6, option B): refused; an SDK route for running schedules is row Y12)");
});

test("an entry no slot names is refused as unknown, and every refusal is named in one error", async () => {
  const e = await refusal(agentDir({ ...WEATHER, "notes.md": "x", "hooks/": "" }));
  expect(e.refused.map((r) => [r.entry, r.status])).toEqual([
    ["hooks/", "does not map"],
    ["notes.md", "unknown"],
  ]);
});

test("another eve version, or none, is refused", async () => {
  const dir = agentDir(WEATHER);
  expect((await refusal(dir, { resolveEveVersion: () => "0.81.0" })).message).toContain(`reads eve ${EVE_PINNED_VERSION} only; 0.81.0 is installed`);
  expect((await refusal(dir, { resolveEveVersion: () => null })).message).toContain("none is installed");
});

test("the installed version is read from node_modules, upward from the directory", () => {
  const dir = agentDir(WEATHER);
  const manifest = path.join(dir, "..", "node_modules", "eve", "package.json");
  fs.mkdirSync(path.dirname(manifest), { recursive: true });
  fs.writeFileSync(manifest, JSON.stringify({ name: "eve", version: "0.80.0" }));
  expect(installedEveVersion(path.join(dir, "tools"))).toBe("0.80.0");
});

test("instructions.md is required on the root", async () => {
  const { "instructions.md": _, ...rest } = WEATHER;
  expect((await refusal(agentDir(rest))).refused[0].entry).toBe("instructions.md");
});

test("agent.ts: the model string is read; any other key is refused by name", async () => {
  expect((await loadEveAgent(agentDir(WEATHER), opts())).model).toBe("openai/gpt-5.4-mini");
  const e = await refusal(agentDir({ ...WEATHER, "agent.ts": 'export default { model: "m", reasoning: "high" };\n' }));
  expect(e.refused.map((r) => r.entry)).toEqual(["agent.ts: reasoning"]);
});

test("a tool that does not map, or carries what the fleet does not translate, is refused naming its file", async () => {
  const cases: [string, string, string][] = [
    ["workflow", `export default { description: "d", inputSchema: ${SCHEMA}, async task() { return 1; } };`, "does not map"],
    ["approve", `export default { description: "d", inputSchema: ${SCHEMA}, approval: "always", async execute() { return 1; } };`, "requires_grant"],
    ["disabled", `export default { kind: "eve:disabled-tool" };`, "removes a framework default"],
    ["no_execute", `export default { description: "d", inputSchema: ${SCHEMA} };`, "no `execute`"],
    ["plain_schema", `export default { description: "d", inputSchema: { type: "object" }, async execute() { return 1; } };`, "not a Standard Schema"],
    [
      "no_json",
      `export default { description: "d", inputSchema: { "~standard": { version: 1, vendor: "v", validate: (v: any) => ({ value: v }) } }, async execute() { return 1; } };`,
      "cannot state itself as JSON Schema",
    ],
  ];
  for (const [name, body, says] of cases) {
    const e = await refusal(agentDir({ ...WEATHER, [`tools/${name}.ts`]: body }));
    expect(e.refused.map((r) => r.entry)).toEqual([`tools/${name}.ts`]);
    expect(e.message).toContain(says);
  }
});

test("a nested directory or a module-backed skill is refused by its path; colocated tests are skipped, as Eve skips them", async () => {
  const e = await refusal(agentDir({ ...WEATHER, "tools/nested/x.ts": "", "skills/packed.ts": "" }));
  expect(e.refused.map((r) => r.entry)).toEqual(["skills/packed.ts", "tools/nested/"]);
  const agent = await loadEveAgent(
    agentDir({ ...WEATHER, "tools/get_weather.test.ts": "throw new Error('imported');", "tools/__tests__/f.json": "{}" }),
    opts(),
  );
  expect(agent.tools.map((t) => t.name)).toEqual(["get_weather"]);
});

test("the weather example loads: its name, skills and tool, and the unit it is declared as", async () => {
  const dir = agentDir({ ...WEATHER, "skills/forecast.md": "Read the forecast aloud.\n", "lib/": "" });
  fs.writeFileSync(path.join(dir, "..", "package.json"), JSON.stringify({ name: "@acme/weather" }));
  const agent = await loadEveAgent(dir, opts({ name: undefined })); // the name from package.json, its scope dropped
  expect(agent.name).toBe("weather");
  expect(agent.skills).toEqual([
    { name: "instructions", text: "You are a weather-focused assistant.\n" },
    { name: "forecast", text: "Read the forecast aloud.\n" },
  ]);
  expect(agent.tools.map((t) => [t.name, t.description, t.inputSchema])).toEqual([
    ["get_weather", "Get the current weather for a city.", { type: "object", properties: { city: { type: "string" } }, required: ["city"] }],
  ]);
  const unit = eveUnitToml(agent, { principal: "weather" });
  expect(unit).toContain('principal = "weather"');
  expect(unit).toContain('runtime = "typescript"');
  expect(unit).toContain('name = "weather"');
  expect(unit).toContain('tools       = "get_weather"');
});

describe("the MCP answers", () => {
  let agent: EveAgent;
  beforeAll(async () => {
    agent = await loadEveAgent(agentDir(WEATHER), opts());
  });
  const rpc = (method: string, params?: unknown, id: unknown = 1) => answer(agent, JSON.stringify({ jsonrpc: "2.0", id, method, params }));

  test("initialize, tools/list", async () => {
    expect((await rpc("initialize"))?.result).toMatchObject({ protocolVersion: "2024-11-05", capabilities: { tools: {} } });
    expect((await rpc("tools/list"))?.result).toEqual({
      tools: [{ name: "get_weather", description: "Get the current weather for a city.", inputSchema: agent.tools[0].inputSchema }],
    });
  });

  test("tools/call runs execute on validated arguments; a schema refusal is a result with isError", async () => {
    expect((await rpc("tools/call", { name: "get_weather", arguments: { city: "Leeds" } }))?.result).toEqual({
      content: [{ type: "text", text: '{"city":"Leeds","condition":"Sunny","temperatureF":72}' }],
      structuredContent: { city: "Leeds", condition: "Sunny", temperatureF: 72 },
    });
    const bad = (await rpc("tools/call", { name: "get_weather", arguments: { city: 7 } }))?.result as { isError: boolean; content: { text: string }[] };
    expect(bad.isError).toBe(true);
    expect(bad.content[0].text).toBe("get_weather: the arguments do not match its schema: city: expected string");
  });

  test("an unknown tool, an unknown method, a notification and a body that is not JSON", async () => {
    expect((await rpc("tools/call", { name: "nope" }))?.error).toEqual({ code: -32602, message: "unknown tool: nope" });
    expect((await rpc("resources/list"))?.error).toMatchObject({ code: -32601 });
    expect(await answer(agent, JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }))).toBeNull();
    expect((await answer(agent, "{"))?.error).toEqual({ code: -32700, message: "parse error" });
  });

  test("served over HTTP on loopback, as a node's bridge reaches it", async () => {
    const server = await serveEveTools(agent);
    try {
      expect(server.url).toMatch(/^http:\/\/127\.0\.0\.1:\d+\/$/);
      const resp = await fetch(server.url, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ jsonrpc: "2.0", id: 9, method: "tools/call", params: { name: "get_weather", arguments: { city: "Hull" } } }),
      });
      expect(resp.status).toBe(200);
      expect(((await resp.json()) as { result: { structuredContent: unknown } }).result.structuredContent).toEqual({
        city: "Hull",
        condition: "Sunny",
        temperatureF: 72,
      });
      expect((await fetch(server.url)).status).toBe(405);
    } finally {
      await server.close();
    }
  });

  test("declareEveAgent declares the unit and writes each skill, the text as the system prompt", async () => {
    const units: string[] = [];
    const prompts: [string, string, unknown][] = [];
    const mycelium = { declareUnits: async (toml: string) => (units.push(toml), { handleId: "h" }) };
    const promptClient = { updatePrompt: async (ns: string, name: string, t: unknown) => void prompts.push([ns, name, t]) };
    const declared = await declareEveAgent(agent, { mycelium: mycelium as never, prompts: promptClient as never });
    expect(units).toEqual([eveUnitToml(agent)]);
    expect(declared.skills).toEqual([[agent.name, "instructions"]]);
    expect(prompts[0][2]).toMatchObject({ system: "You are a weather-focused assistant.\n", userTemplate: "{{input}}" });
  });
});
