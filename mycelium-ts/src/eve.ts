/**
 * mycelium/eve — an Eve agent directory, read as published by the `eve` package, declared as a unit of a fleet
 * (adopter-surface Y6).
 *
 * A **demonstration, not a compatibility promise** (the plan's D24): the loader reads one pinned version of Eve,
 * {@link EVE_PINNED_VERSION}, and refuses a directory whose installed `eve` is another. Eve's runtime is Eve's
 * product; what is carried here is the directory's *contents* — the instructions, the Markdown skills and the tools —
 * onto the fleet's own doors: prompt skills through the gateway's `prompts/` route, the tools as an MCP server a
 * node bridges (`McpHandle::connect_mcp_server`), so `/mcp` reaches them under the fleet's evaluator.
 *
 * Every slot of Eve's agent directory has a status in {@link EVE_SLOTS}, and a slot the loader does not load is
 * **refused by name** — {@link EveRefusedError} names every one — never skipped: an agent that relied on a hook or a
 * sandbox would otherwise run on the fleet without it and look as if it worked.
 *
 * @example
 * ```ts
 * import { loadEveAgent, serveEveTools, declareEveAgent, MyceliumAgent, PromptSkillClient } from "mycelium-ts";
 *
 * const agent = await loadEveAgent("./agent");          // refuses what does not map, by name
 * const server = await serveEveTools(agent, { port: 0 }); // the tools, as an MCP server for a node to bridge
 * await declareEveAgent(agent, {
 *   mycelium: new MyceliumAgent("127.0.0.1", 8300),
 *   prompts: new PromptSkillClient("127.0.0.1", 8300),
 * });
 * ```
 */

import * as fs from "node:fs";
import * as http from "node:http";
import * as path from "node:path";
import { pathToFileURL } from "node:url";

import type { MyceliumAgent } from "./agent";
import type { ScheduleDeclaration, ScheduleRun } from "./types";
import type { PromptSkillClient } from "./prompt_skill";
import type { UnitHandle } from "./types";

/** The one Eve version this loader reads (`eve` on npm, published 2026-10-15). Any other is refused. */
export const EVE_PINNED_VERSION = "0.80.0";

/** What the loader does with a slot of an Eve agent directory. */
export type EveSlotStatus = "loaded" | "not built" | "does not map";

/** One slot of Eve's agent directory (Eve 0.80.0's `docs/reference/agent-files.md`) and how it lands. */
export interface EveSlot {
  /** The path relative to the agent directory: a file (`agent.ts`) or a directory (`tools/`). */
  readonly slot: string;
  readonly status: EveSlotStatus;
  /** What it is on the fleet, or why it is refused. */
  readonly onTheFleet: string;
}

/**
 * Every slot of an Eve 0.80.0 agent directory, in the order of Eve's own reference, and how this loader treats it.
 * `loaded` slots are read; every other one present in a directory is refused by name. The example's README carries
 * this table and a test checks the two agree (`tests/eve.test.ts`).
 */
export const EVE_SLOTS: readonly EveSlot[] = [
  { slot: "agent.ts", status: "loaded", onTheFleet: "the unit; its `model` is read and not routed, because the fleet routes inference through its own router (row R); any other key is refused by name" },
  { slot: "instructions.md", status: "loaded", onTheFleet: "a prompt skill `<agent>/instructions`, the text as its system prompt" },
  { slot: "instructions.ts", status: "not built", onTheFleet: "a module-backed instructions source; the Markdown form is the one loaded" },
  { slot: "instructions/", status: "not built", onTheFleet: "a directory of instruction fragments; the single Markdown file is the one loaded" },
  { slot: "instrumentation/", status: "not built", onTheFleet: "Eve's telemetry providers; the fleet's records are its own (X4)" },
  { slot: "channels/", status: "not built", onTheFleet: "maps to the gateway and the A2A door, by hand; this loader does not translate a channel (Slack does not map)" },
  { slot: "connections/", status: "not built", onTheFleet: "maps to tool discovery (guide 06), by hand; this loader does not translate a connection" },
  { slot: "extensions/", status: "not built", onTheFleet: "mounted packages; the loader reads only the agent's own files" },
  { slot: "hooks/", status: "does not map", onTheFleet: "lifecycle and stream-event subscribers; the fleet has nothing they could subscribe to" },
  { slot: "skills/", status: "loaded", onTheFleet: "each flat `skills/<name>.md` a prompt skill `<agent>/<name>`; a module-backed or packaged skill is refused by name" },
  { slot: "lib/", status: "loaded", onTheFleet: "nothing declared: helper code the tools import, as in Eve" },
  { slot: "memory.ts", status: "not built", onTheFleet: "cross-session memory; the fleet's KV could hold it, and this loader does not translate it" },
  { slot: "memory/", status: "not built", onTheFleet: "cross-session memory; the fleet's KV could hold it, and this loader does not translate it" },
  { slot: "sandbox.ts", status: "does not map", onTheFleet: "the Vercel Sandbox API has no counterpart; confined activations (row M) are a stem's, not a tool's" },
  { slot: "sandbox/", status: "does not map", onTheFleet: "the Vercel Sandbox API has no counterpart; confined activations (row M) are a stem's, not a tool's" },
  { slot: "tools/", status: "loaded", onTheFleet: "each `defineTool` module an MCP tool, its schema the tool's, served for a node to bridge; a workflow tool (`task`, `serve`) does not map, and a tool with `approval` is refused — declare a `[[rule]]` with `requires_grant` (X3) instead" },
  { slot: "schedules/", status: "loaded", onTheFleet: "each `schedules/<name>.md` (frontmatter `cron`, the body its prompt) or `<name>.ts` (`defineSchedule`) a `[[schedule]]` run through the gateway (Y12), when its cron is a period and a phase in UTC; any other cron, a nested schedule or a `.ts` key besides `cron`, `markdown` and `run` is refused by name" },
  { slot: "subagents/", status: "not built", onTheFleet: "maps differently: another unit found by capability, discovery rather than a parent calling a child; this loader does not translate one" },
];

/** A slot found in a directory, with its status — what {@link classifyEveDirectory} returns. */
export interface EveEntry {
  readonly entry: string;
  readonly status: EveSlotStatus | "unknown";
  readonly reason: string;
}

/** A directory, or a part of one, this loader will not run: every refused entry, by name. */
export class EveRefusedError extends Error {
  constructor(public readonly refused: readonly EveEntry[]) {
    super(
      `the Eve agent directory cannot be loaded: ${refused
        .map((r) => `${r.entry} (${r.status}: ${r.reason})`)
        .join("; ")}`,
    );
    this.name = "EveRefusedError";
  }
}

/** A Standard Schema (v1) that can state itself as JSON Schema — what Eve's `inputSchema` is, zod's included. */
interface StandardSchema {
  "~standard": {
    version: number;
    vendor: string;
    validate: (value: unknown) => unknown;
    jsonSchema?: { input: (options: { target: string }) => Record<string, unknown> };
  };
}

/** One Eve tool, as the fleet serves it. */
export interface EveTool {
  /** The file's name without its extension — Eve's own rule (`tools/get_weather.ts` is `get_weather`). */
  readonly name: string;
  readonly description: string;
  /** The tool's input schema as JSON Schema (draft 2020-12), from its Standard Schema. */
  readonly inputSchema: Record<string, unknown>;
  /** Validate the arguments with the tool's own schema, then run its `execute`. */
  call(args: unknown): Promise<unknown>;
}

/** One prompt skill an Eve directory carries: its instructions, or a flat Markdown skill. */
export interface EveSkill {
  readonly name: string;
  readonly text: string;
}

/**
 * One Eve schedule (`schedules/<name>.md` or `.ts`), its cron read as a period and a phase counted from the Unix epoch in
 * UTC — Eve's own clock for a cron ("Vercel evaluates the expression in UTC", Eve 0.80.0's `docs/schedules.mdx`).
 */
export interface EveSchedule {
  /** The file's path under `schedules/` without its extension: Eve's rule for a schedule's name. */
  readonly name: string;
  /** The fleet's schedule id: `<agent>.<name>`, so two agents' schedules of one name are two schedules. */
  readonly id: string;
  readonly cron: string;
  readonly periodSecs: number;
  readonly phaseSecs: number;
  /** `markdown`: a prompt the agent runs on (fire-and-forget); `run`: Eve's handler, called with what the fleet has. */
  readonly form: "markdown" | "run";
  readonly markdown?: string;
  readonly run?: (args: EveScheduleArgs) => Promise<void> | void;
}

/** What an Eve `run` handler is called with on the fleet: `waitUntil` as Eve's, `to` refused (channels are not built). */
export interface EveScheduleArgs {
  readonly to: (...args: unknown[]) => never;
  readonly waitUntil: (task: Promise<unknown>) => void;
  /** Eve's pre-built app principal has no counterpart: an empty object. */
  readonly appAuth: Record<string, never>;
}

/** An Eve agent directory, read. */
export interface EveAgent {
  /** The agent's name: one key segment, the namespace of its prompt skills. */
  readonly name: string;
  readonly dir: string;
  /** The `eve` version installed beside the directory — always {@link EVE_PINNED_VERSION}. */
  readonly eveVersion: string;
  /** `agent.ts`'s `model`, read and not routed. */
  readonly model: string | null;
  /** `instructions.md` first, then `skills/*.md` by name. */
  readonly skills: readonly EveSkill[];
  readonly tools: readonly EveTool[];
  /** `schedules/*`, by name: each a period and a phase, run through the gateway by {@link runEveSchedules}. */
  readonly schedules: readonly EveSchedule[];
  /** Every entry of the directory and how it was read. */
  readonly entries: readonly EveEntry[];
}

/** How {@link loadEveAgent} reads a directory. */
export interface LoadOptions {
  /** The agent's name. Default: the `name` of the project's `package.json` — the one beside `agent/` — without its npm
   *  scope, else the project directory's name: Eve's own rule for a standalone root agent. */
  name?: string;
  /** Import a module. Default: Node's dynamic `import()` of the file's URL, so a `.ts` tool runs under Node's own
   *  type stripping (Eve 0.80.0 needs Node 24). */
  importModule?: (file: string) => Promise<Record<string, unknown>>;
  /** The installed `eve` version. Default: `node_modules/eve/package.json`, searched upward from the directory. */
  resolveEveVersion?: (dir: string) => string | null;
}

const TOOL_EXTENSIONS = [".ts", ".mts", ".js", ".mjs"];
const KEY_SEGMENT = /^[a-z0-9][a-z0-9_-]*$/;
const WORKFLOW_TOOL_BRAND = Symbol.for("eve:workflow-tool-brand");
const AGENT_KEYS = new Set(["model"]);

/** Eve skips colocated tests during discovery (`*.test.*`, `*.spec.*`, `__tests__/`); so does this loader. */
function isColocatedTest(name: string): boolean {
  return name === "__tests__" || /\.(test|spec)\.[cm]?[jt]sx?$/.test(name);
}

/**
 * Entries at the agent root that are never authored content, skipped as Eve 0.80.0's `classifyAgentRootEntry` skips
 * them: the directories its discovery ignores as generated (`GENERATED_AGENT_DIRECTORY_NAMES` in
 * `discover/filesystem.js`: `.devtools`, `.eve`, `.next`, `.output`, `.vercel`, `node_modules`), and every other name
 * starting with `.` (`.DS_Store`, `.gitkeep`), which no root slot has. Inside `tools/` and `skills/` a dot entry is
 * refused instead, as Eve errors on it there (see `classifyEveDirectory`). Eve also ignores any other root *file* no slot names and warns on any other directory; this loader
 * refuses both by name instead — stricter than Eve, on purpose: an entry it would skip is one an author may rely on.
 */
function isNeverAuthored(name: string): boolean {
  return name.startsWith(".") || name === "node_modules";
}

/** The fields of an Eve 0.80.0 `defineTool` the fleet carries; any other is refused by name. */
const TOOL_FIELDS = new Set(["description", "inputSchema", "execute"]);

/**
 * Name every entry of an Eve agent directory with its status, without importing anything. An entry no slot names is
 * `unknown`; inside `skills/` and `tools/` an entry the slot does not take (a module-backed skill, a nested directory)
 * is `not built`, named by its path.
 */
export function classifyEveDirectory(dir: string): EveEntry[] {
  const out: EveEntry[] = [];
  for (const dirent of fs.readdirSync(dir, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
    if (isNeverAuthored(dirent.name)) continue;
    const entry = dirent.isDirectory() ? `${dirent.name}/` : dirent.name;
    const slot = EVE_SLOTS.find((s) => s.slot === entry);
    if (!slot) {
      out.push({ entry, status: "unknown", reason: "no slot of an Eve 0.80.0 agent directory has this name" });
      continue;
    }
    out.push({ entry, status: slot.status, reason: slot.onTheFleet });
    if (slot.slot === "skills/" || slot.slot === "tools/") {
      for (const inner of fs.readdirSync(path.join(dir, dirent.name), { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
        if (isColocatedTest(inner.name)) continue;
        const sub = `${slot.slot}${inner.name}${inner.isDirectory() ? "/" : ""}`;
        // Inside a slot Eve 0.80.0 does not skip a dot entry: its discovery errors on the name (`TOOL_SLUG_PATTERN`,
        // `SKILL_NAME_PATTERN` in `discover/grammar.js` start with a letter or digit). So this loader refuses it too.
        if (inner.name.startsWith(".")) {
          out.push({ entry: sub, status: "not built", reason: "a name starting with `.` is no tool or skill name; Eve refuses it too" });
          continue;
        }
        const ok =
          inner.isFile() &&
          (slot.slot === "skills/" ? inner.name.endsWith(".md") : TOOL_EXTENSIONS.includes(path.extname(inner.name)));
        if (!ok) {
          out.push({
            entry: sub,
            status: "not built",
            reason: slot.slot === "skills/" ? "only a flat Markdown skill is loaded" : "only a flat tool module is loaded",
          });
        }
      }
      if (slot.slot === "tools/") out.push(...duplicateTools(path.join(dir, dirent.name)));
    }
    if (slot.slot === "schedules/") out.push(...classifySchedules(path.join(dir, dirent.name)));
  }
  return out;
}

const SCHEDULE_EXTENSIONS = [".md", ...TOOL_EXTENSIONS];

/** `schedules/`'s entries the loader does not take, by name: a nested schedule (Eve names it `a/b`, the fleet's id is one
 *  key segment), a file that is neither Markdown nor a module, a dot entry (no schedule name starts with `.`), and two
 *  files naming one schedule. */
function classifySchedules(schedulesDir: string): EveEntry[] {
  const out: EveEntry[] = [];
  const byName = new Map<string, string[]>();
  for (const inner of fs.readdirSync(schedulesDir, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
    if (isColocatedTest(inner.name)) continue;
    const sub = `schedules/${inner.name}${inner.isDirectory() ? "/" : ""}`;
    if (inner.name.startsWith(".")) {
      out.push({ entry: sub, status: "not built", reason: "a name starting with `.` is no schedule name" });
    } else if (inner.isDirectory()) {
      out.push({ entry: sub, status: "not built", reason: "a nested schedule (Eve names it by its path); a schedule id on the fleet is one key segment" });
    } else if (!inner.isFile() || !SCHEDULE_EXTENSIONS.includes(path.extname(inner.name))) {
      out.push({ entry: sub, status: "not built", reason: "only a Markdown or module schedule file is loaded" });
    } else {
      const name = inner.name.slice(0, -path.extname(inner.name).length);
      byName.set(name, [...(byName.get(name) ?? []), inner.name]);
    }
  }
  for (const files of byName.values()) {
    if (files.length > 1) {
      for (const f of files) out.push({ entry: `schedules/${f}`, status: "not built", reason: `two files name one schedule: ${files.join(", ")}` });
    }
  }
  return out;
}

/**
 * A 5-field cron (`minute hour day-of-month month day-of-week`, UTC) as a period and a phase from the Unix epoch, or why
 * it is not one. A period and a phase is what a `[[schedule]]` declares (Y2: "no cron-expression language beyond a
 * period and a phase"), so the crons that are one are read and the rest refused:
 *
 * - `* * * * *` — every minute; `*\/N * * * *` with N dividing 60 — every N minutes, on the hour's boundaries;
 * - `M * * * *` — hourly at minute M; `M *\/N * * *` with N dividing 24 — every N hours at minute M;
 * - `M H * * *` — daily at H:M; `M H * * D` — weekly on day D (0 or 7 Sunday) at H:M;
 * - `@hourly`, `@daily` (`@midnight`), `@weekly`.
 *
 * A list, a range, a step anywhere else, a day of the month or a month is refused: none is one period.
 */
export function cronAsPeriod(cron: string): { periodSecs: number; phaseSecs: number } | { refused: string } {
  const alias: Record<string, string> = { "@hourly": "0 * * * *", "@daily": "0 0 * * *", "@midnight": "0 0 * * *", "@weekly": "0 0 * * 0" };
  const fields = (alias[cron.trim()] ?? cron).trim().split(/\s+/);
  if (fields.length !== 5) return { refused: `\`${cron}\` is not a 5-field cron` };
  const [m, h, dom, mon, dow] = fields;
  const num = (f: string, max: number): number | null => (/^\d+$/.test(f) && Number(f) <= max ? Number(f) : null);
  const step = (f: string): number | null => (/^\*\/\d+$/.test(f) ? Number(f.slice(2)) : null);
  const notOne = (why: string) => ({ refused: `\`${cron}\` is not a period and a phase: ${why}` });
  if (dom !== "*" || mon !== "*") return notOne("a day of the month or a month is a calendar");
  const minute = num(m, 59);
  if (h === "*" && dow === "*") {
    if (m === "*") return { periodSecs: 60, phaseSecs: 0 };
    const n = step(m);
    if (n !== null) return n >= 1 && 60 % n === 0 ? { periodSecs: n * 60, phaseSecs: 0 } : notOne(`every ${n} minutes does not divide the hour`);
    if (minute !== null) return { periodSecs: 3600, phaseSecs: minute * 60 };
    return notOne("its minute is a list, a range or a step");
  }
  if (minute === null) return notOne("its minute is not one minute");
  const hs = step(h);
  if (hs !== null && dow === "*") {
    return hs >= 1 && 24 % hs === 0 ? { periodSecs: hs * 3600, phaseSecs: minute * 60 } : notOne(`every ${hs} hours does not divide the day`);
  }
  const hour = num(h, 23);
  if (hour === null) return notOne("its hour is a list, a range or a step");
  const atTime = hour * 3600 + minute * 60;
  if (dow === "*") return { periodSecs: 86_400, phaseSecs: atTime };
  const day = num(dow, 7);
  if (day === null) return notOne("its day of the week is a list, a range or a step");
  // The Unix epoch began on a Thursday (day 4): a week's slot starts there.
  return { periodSecs: 7 * 86_400, phaseSecs: (((day % 7) - 4 + 7) % 7) * 86_400 + atTime };
}

/** A Markdown schedule's frontmatter: `cron` alone, one `key: value` line, the value bare or quoted. */
function scheduleFrontmatter(text: string): { cron: string; body: string } | { refused: string } {
  const m = /^---\r?\n([\s\S]*?)\r?\n---\r?\n?([\s\S]*)$/.exec(text);
  if (!m) return { refused: "it does not start with YAML frontmatter declaring `cron`" };
  let cron: string | null = null;
  for (const line of m[1].split(/\r?\n/)) {
    if (line.trim() === "" || line.trim().startsWith("#")) continue;
    const kv = /^([A-Za-z_][A-Za-z0-9_-]*):\s*(.*?)\s*$/.exec(line);
    if (!kv) return { refused: `its frontmatter line \`${line}\` is not read (one \`key: value\` per line)` };
    if (kv[1] !== "cron") return { refused: `its frontmatter sets \`${kv[1]}\`; only \`cron\` is read` };
    const v = kv[2];
    cron = /^(["']).*\1$/.test(v) ? v.slice(1, -1) : v;
  }
  if (cron === null || cron === "") return { refused: "its frontmatter declares no `cron`" };
  return { cron, body: m[2].replace(/^\r?\n/, "") };
}

/** Two tool modules with one name (`get_weather.ts` and `get_weather.js`): each refused by name, as Eve refuses a slot
 *  collision (`discover/slot-collision`) rather than pick one. */
function duplicateTools(toolsDir: string): EveEntry[] {
  const byName = new Map<string, string[]>();
  for (const f of toolFiles(toolsDir)) {
    const name = f.slice(0, -path.extname(f).length);
    byName.set(name, [...(byName.get(name) ?? []), f]);
  }
  return [...byName.values()]
    .filter((files) => files.length > 1)
    .flatMap((files) =>
      files.map((f) => ({ entry: `tools/${f}`, status: "not built" as const, reason: `two tool modules name one tool: ${files.join(", ")}` })),
    );
}

/** The tool modules of `tools/`, sorted: flat files with a module extension, tests and never-authored names skipped. */
function toolFiles(toolsDir: string): string[] {
  return fs
    .readdirSync(toolsDir, { withFileTypes: true })
    .filter((d) => d.isFile() && TOOL_EXTENSIONS.includes(path.extname(d.name)) && !isColocatedTest(d.name) && !isNeverAuthored(d.name))
    .map((d) => d.name)
    .sort();
}

/** `node_modules/eve/package.json`'s version, searched upward from `dir`, or `null`. */
export function installedEveVersion(dir: string): string | null {
  let at = path.resolve(dir);
  for (;;) {
    const manifest = path.join(at, "node_modules", "eve", "package.json");
    if (fs.existsSync(manifest)) {
      const version = (JSON.parse(fs.readFileSync(manifest, "utf8")) as { version?: unknown }).version;
      return typeof version === "string" ? version : null;
    }
    const up = path.dirname(at);
    if (up === at) return null;
    at = up;
  }
}

function defaultName(dir: string): string {
  const parent = path.dirname(path.resolve(dir));
  const manifest = path.join(parent, "package.json");
  if (fs.existsSync(manifest)) {
    const name = (JSON.parse(fs.readFileSync(manifest, "utf8")) as { name?: unknown }).name;
    if (typeof name === "string" && name !== "") return name.replace(/^@[^/]+\//, "");
  }
  return path.basename(parent);
}

// `import()` kept as a real dynamic import in this CommonJS build: TypeScript would compile it to `require()`, which
// does not load an ES module's `.ts` file the way Node's own loader does.
const nativeImport = new Function("url", "return import(url)") as (url: string) => Promise<Record<string, unknown>>;

function isStandardSchema(value: unknown): value is StandardSchema {
  const std = (value as StandardSchema | null)?.["~standard"];
  return typeof std === "object" && std !== null && typeof std.validate === "function";
}

/** The refusal for a tool module, or `null` when it is a plain `defineTool` the loader serves. */
function toolRefusal(name: string, def: unknown): string | null {
  if (typeof def !== "object" || def === null) return "its default export is not a tool definition";
  const d = def as Record<string | symbol, unknown>;
  if (d.kind === "eve:disabled-tool") return "`disableTool()` removes a framework default; the fleet has none to remove";
  if (d.kind === "eve:dynamic") return "`defineDynamic` resolves per session; not built";
  if (d[WORKFLOW_TOOL_BRAND] === true || "task" in d || "serve" in d) return "a workflow tool (Workflow hooks) does not map";
  if ("approval" in d || "approvalKey" in d) {
    return "an Eve approval policy is not carried; declare a `[[rule]]` with `requires_grant` for the tool (X3)";
  }
  const extra = Object.keys(d).filter((k) => !TOOL_FIELDS.has(k));
  if (extra.length > 0) {
    return `it sets ${extra.map((k) => `\`${k}\``).join(", ")}, which the fleet does not carry; only \`description\`, \`inputSchema\` and \`execute\` are read`;
  }
  if (typeof d.description !== "string" || d.description === "") return "it has no `description`";
  if (typeof d.execute !== "function") return "it has no `execute`";
  if (!isStandardSchema(d.inputSchema)) return "its `inputSchema` is not a Standard Schema";
  if (typeof d.inputSchema["~standard"].jsonSchema?.input !== "function") {
    return "its `inputSchema` cannot state itself as JSON Schema (`~standard.jsonSchema`)";
  }
  if (!KEY_SEGMENT.test(name)) return "its name is not one key segment (lower case, digits, `_`, `-`)";
  return null;
}

/**
 * Read an Eve agent directory — the `agent/` directory of a single-agent project; Eve's flat layout (agent files in the
 * app root beside `package.json`) is not read, and is refused at its first non-slot entry. Refuse what the fleet does not load — every such entry, by
 * name, in one {@link EveRefusedError} — then import `agent.ts` and each tool and read the Markdown skills. Nothing is
 * declared or served; see {@link declareEveAgent} and {@link serveEveTools}.
 */
export async function loadEveAgent(dir: string, options: LoadOptions = {}): Promise<EveAgent> {
  const root = path.resolve(dir);
  const importModule = options.importModule ?? ((file: string) => nativeImport(pathToFileURL(file).href));
  const eveVersion = (options.resolveEveVersion ?? installedEveVersion)(root);
  const refused: EveEntry[] = [];
  if (eveVersion !== EVE_PINNED_VERSION) {
    refused.push({
      entry: "eve",
      status: "not built",
      reason: `the loader reads eve ${EVE_PINNED_VERSION} only; ${eveVersion === null ? "none is installed" : `${eveVersion} is installed`}`,
    });
  }
  const entries = classifyEveDirectory(root);
  refused.push(...entries.filter((e) => e.status !== "loaded"));
  const name = options.name ?? defaultName(root);
  if (!KEY_SEGMENT.test(name)) {
    refused.push({ entry: "name", status: "not built", reason: `the agent's name ${JSON.stringify(name)} is not one key segment` });
  }
  if (!fs.existsSync(path.join(root, "instructions.md"))) {
    refused.push({ entry: "instructions.md", status: "not built", reason: "a root agent's instructions are required; the Markdown form is the one loaded" });
  }
  if (refused.length > 0) throw new EveRefusedError(refused);

  // agent.ts: `model` read, never routed; another key would change what the agent is, so it is refused.
  let model: string | null = null;
  const agentFile = path.join(root, "agent.ts");
  if (fs.existsSync(agentFile)) {
    const config = (await importModule(agentFile)).default as Record<string, unknown> | undefined;
    const extra = Object.keys(config ?? {}).filter((k) => !AGENT_KEYS.has(k));
    if (extra.length > 0) {
      throw new EveRefusedError(extra.map((k) => ({ entry: `agent.ts: ${k}`, status: "not built" as const, reason: "only `model` is read" })));
    }
    model = typeof config?.model === "string" ? config.model : null;
    if (config?.model !== undefined && model === null) {
      throw new EveRefusedError([{ entry: "agent.ts: model", status: "not built", reason: "a `LanguageModel` object is not read; a model id string is" }]);
    }
  }

  const skills: EveSkill[] = [{ name: "instructions", text: fs.readFileSync(path.join(root, "instructions.md"), "utf8") }];
  const skillsDir = path.join(root, "skills");
  if (fs.existsSync(skillsDir)) {
    for (const file of fs.readdirSync(skillsDir).filter((f) => f.endsWith(".md") && !isColocatedTest(f) && !isNeverAuthored(f)).sort()) {
      const skillName = file.slice(0, -".md".length);
      if (!KEY_SEGMENT.test(skillName) || skillName === "instructions") {
        throw new EveRefusedError([{ entry: `skills/${file}`, status: "not built", reason: "its name is not one key segment, or is `instructions`" }]);
      }
      skills.push({ name: skillName, text: fs.readFileSync(path.join(skillsDir, file), "utf8") });
    }
  }

  const tools: EveTool[] = [];
  const toolRefusals: EveEntry[] = [];
  const toolsDir = path.join(root, "tools");
  if (fs.existsSync(toolsDir)) {
    for (const file of toolFiles(toolsDir)) {
      const toolName = file.slice(0, -path.extname(file).length);
      const def = (await importModule(path.join(toolsDir, file))).default;
      const why = toolRefusal(toolName, def);
      if (why !== null) {
        toolRefusals.push({ entry: `tools/${file}`, status: why.includes("does not map") ? "does not map" : "not built", reason: why });
        continue;
      }
      const d = def as { description: string; inputSchema: StandardSchema; execute: (input: unknown, ctx: unknown) => unknown };
      const std = d.inputSchema["~standard"];
      tools.push({
        name: toolName,
        description: d.description,
        inputSchema: std.jsonSchema!.input({ target: "draft-2020-12" }),
        async call(args: unknown) {
          const checked = (await std.validate(args)) as { value?: unknown; issues?: readonly { message: string; path?: readonly unknown[] }[] };
          if (checked.issues) throw new EveToolInputError(toolName, checked.issues);
          // Eve passes a context of its runtime (session, sandbox, auth); the fleet has none to give, so a tool that
          // reads it fails at its call — stated in the README.
          const output = await d.execute(checked.value, {});
          // A streamed result (an async iterable — Eve's yielded tool output) has no one value to answer with.
          if (output !== null && typeof output === "object" && Symbol.asyncIterator in output) {
            throw new Error(`${toolName}: a streamed result (an async iterable) is not carried; return one value`);
          }
          return output;
        },
      });
    }
  }
  if (toolRefusals.length > 0) throw new EveRefusedError(toolRefusals);

  const schedules: EveSchedule[] = [];
  const scheduleRefusals: EveEntry[] = [];
  const schedulesDir = path.join(root, "schedules");
  if (fs.existsSync(schedulesDir)) {
    const files = fs
      .readdirSync(schedulesDir, { withFileTypes: true })
      .filter((d) => d.isFile() && SCHEDULE_EXTENSIONS.includes(path.extname(d.name)) && !isColocatedTest(d.name) && !d.name.startsWith("."))
      .map((d) => d.name)
      .sort();
    for (const file of files) {
      const scheduleName = file.slice(0, -path.extname(file).length);
      const entry = `schedules/${file}`;
      const refuse = (reason: string) => scheduleRefusals.push({ entry, status: "not built", reason });
      if (!KEY_SEGMENT.test(scheduleName)) {
        refuse("its name is not one key segment (lower case, digits, `_`, `-`)");
        continue;
      }
      let cron: string;
      let form: "markdown" | "run";
      let markdown: string | undefined;
      let run: EveSchedule["run"];
      if (file.endsWith(".md")) {
        const fm = scheduleFrontmatter(fs.readFileSync(path.join(schedulesDir, file), "utf8"));
        if ("refused" in fm) {
          refuse(fm.refused);
          continue;
        }
        [cron, form, markdown] = [fm.cron, "markdown", fm.body];
      } else {
        const def = (await importModule(path.join(schedulesDir, file))).default as Record<string, unknown> | undefined;
        if (typeof def !== "object" || def === null) {
          refuse("its default export is not a schedule definition");
          continue;
        }
        const extra = Object.keys(def).filter((k) => !["cron", "markdown", "run"].includes(k));
        if (extra.length > 0) {
          refuse(`it sets ${extra.map((k) => `\`${k}\``).join(", ")}; only \`cron\`, \`markdown\` and \`run\` are read`);
          continue;
        }
        const hasMarkdown = typeof def.markdown === "string";
        const hasRun = typeof def.run === "function";
        if (typeof def.cron !== "string" || hasMarkdown === hasRun) {
          refuse("a schedule is a `cron` string and exactly one of `markdown` or `run`");
          continue;
        }
        cron = def.cron;
        form = hasRun ? "run" : "markdown";
        markdown = hasMarkdown ? (def.markdown as string) : undefined;
        run = hasRun ? (def.run as EveSchedule["run"]) : undefined;
      }
      const period = cronAsPeriod(cron);
      if ("refused" in period) {
        refuse(period.refused);
        continue;
      }
      schedules.push({ name: scheduleName, id: `${name}.${scheduleName}`, cron, ...period, form, markdown, run });
    }
  }
  if (scheduleRefusals.length > 0) throw new EveRefusedError(scheduleRefusals);

  return { name, dir: root, eveVersion: eveVersion!, model, skills, tools, schedules, entries };
}

/** A tool's arguments its own schema refused. */
export class EveToolInputError extends Error {
  constructor(public readonly tool: string, public readonly issues: readonly { message: string; path?: readonly unknown[] }[]) {
    super(`${tool}: the arguments do not match its schema: ${issues.map((i) => `${(i.path ?? []).join(".") || "(root)"}: ${i.message}`).join("; ")}`);
    this.name = "EveToolInputError";
  }
}

/** A running MCP server for an Eve agent's tools. */
export interface EveToolServer {
  /** The URL a node bridges, e.g. `http://127.0.0.1:41234/`. */
  readonly url: string;
  close(): Promise<void>;
}

/**
 * Serve an Eve agent's tools as an MCP server over HTTP (JSON-RPC 2.0 on `POST /`: `initialize`, `tools/list`,
 * `tools/call`) for a node to bridge with `connect_mcp_server`: the node writes each tool under
 * `tools/{name}/{node}` and forwards `mcp.invoke` here, after its own `/mcp` door has decided the call. The server
 * itself decides nothing — bind it where only the bridging node reaches it (default `127.0.0.1`).
 */
export function serveEveTools(agent: EveAgent, options: { port?: number; host?: string } = {}): Promise<EveToolServer> {
  const host = options.host ?? "127.0.0.1";
  const server = http.createServer((req, res) => {
    if (req.method !== "POST") {
      res.writeHead(405).end();
      return;
    }
    // The bridge (`connect_mcp_server`) posts JSON and sends no `Origin`. A browser does send one, and a page's
    // "simple" cross-origin POST cannot set `application/json` — so neither reaches a tool past `/mcp`'s decision.
    if (req.headers.origin !== undefined) {
      res.writeHead(403, { "content-type": "application/json" }).end(JSON.stringify({ error: "a request with an Origin is refused" }));
      return;
    }
    if (!/^application\/json(\s*;|$)/i.test(req.headers["content-type"] ?? "")) {
      res.writeHead(415, { "content-type": "application/json" }).end(JSON.stringify({ error: "content-type must be application/json" }));
      return;
    }
    const chunks: Buffer[] = [];
    req.on("data", (c: Buffer) => chunks.push(c));
    req.on("end", () => {
      answer(agent, Buffer.concat(chunks).toString("utf8"))
        .then((reply) => {
          if (reply === null) {
            res.writeHead(202).end();
            return;
          }
          res.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify(reply));
        })
        .catch((e: unknown) => {
          // Never an unhandled rejection: the process serving the tools outlives a request it could not answer.
          res
            .writeHead(500, { "content-type": "application/json" })
            .end(JSON.stringify({ jsonrpc: "2.0", id: null, error: { code: -32603, message: e instanceof Error ? e.message : String(e) } }));
        });
    });
  });
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(options.port ?? 0, host, () => {
      const address = server.address();
      const port = typeof address === "object" && address !== null ? address.port : options.port;
      resolve({
        url: `http://${host}:${port}/`,
        close: () => new Promise<void>((done) => server.close(() => done())),
      });
    });
  });
}

/** One JSON-RPC answer, or `null` for a notification. Exported for the tests. */
export async function answer(agent: EveAgent, body: string): Promise<Record<string, unknown> | null> {
  let parsed: unknown;
  try {
    parsed = JSON.parse(body);
  } catch {
    return { jsonrpc: "2.0", id: null, error: { code: -32700, message: "parse error" } };
  }
  // One request object per POST, as the bridge sends: a batch, `null` or a scalar is an invalid request, answered.
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    const what = Array.isArray(parsed) ? "a batch is not supported" : "the body is not a request object";
    return { jsonrpc: "2.0", id: null, error: { code: -32600, message: `invalid request: ${what}` } };
  }
  const msg = parsed as { id?: unknown; method?: unknown; params?: { name?: unknown; arguments?: unknown } | null };
  const id = msg.id ?? null;
  if (msg.id === undefined) return null; // a notification (`notifications/initialized`) is not answered
  switch (msg.method) {
    case "initialize":
      return {
        jsonrpc: "2.0",
        id,
        result: {
          protocolVersion: "2024-11-05",
          capabilities: { tools: {} },
          serverInfo: { name: `eve:${agent.name}`, version: agent.eveVersion },
        },
      };
    case "tools/list":
      return {
        jsonrpc: "2.0",
        id,
        result: { tools: agent.tools.map((t) => ({ name: t.name, description: t.description, inputSchema: t.inputSchema })) },
      };
    case "tools/call": {
      const tool = agent.tools.find((t) => t.name === msg.params?.name);
      if (!tool) return { jsonrpc: "2.0", id, error: { code: -32602, message: `unknown tool: ${String(msg.params?.name)}` } };
      try {
        const output = await tool.call(msg.params?.arguments ?? {});
        const structured = typeof output === "object" && output !== null && !Array.isArray(output);
        return {
          jsonrpc: "2.0",
          id,
          result: {
            content: [{ type: "text", text: typeof output === "string" ? output : JSON.stringify(output) }],
            ...(structured ? { structuredContent: output } : {}),
          },
        };
      } catch (e) {
        // A tool's own failure is a result with `isError`, as MCP has it; the model sees it, the protocol does not fail.
        return { jsonrpc: "2.0", id, result: { content: [{ type: "text", text: e instanceof Error ? e.message : String(e) }], isError: true } };
      }
    }
    default:
      return { jsonrpc: "2.0", id, error: { code: -32601, message: `method not found: ${String(msg.method)}` } };
  }
}

/** The unit an Eve agent is declared as: its principal, `runtime = "typescript"`, and one capability naming it. */
export function eveUnitToml(agent: EveAgent, options: { principal?: string } = {}): string {
  const q = (s: string) => JSON.stringify(s);
  const lines = [
    `# Written by mycelium-ts's Eve loader from ${q(path.basename(agent.dir))} (eve ${agent.eveVersion}).`,
    ...(options.principal !== undefined ? [`principal = ${q(options.principal)}`] : []),
    `runtime = "typescript"`,
    "",
    "[[capability]]",
    `ns   = "eve"`,
    `name = ${q(agent.name)}`,
    "  [capability.attrs]",
    `  eve_version = ${q(agent.eveVersion)}`,
    `  tools       = ${q(agent.tools.map((t) => t.name).join(","))}`,
    `  skills      = ${q(agent.skills.map((s) => s.name).join(","))}`,
    "",
  ];
  return lines.join("\n");
}

/** What {@link declareEveAgent} declared. */
export interface EveDeclaration {
  readonly unit: UnitHandle;
  /** `(ns, name)` of every prompt skill written. */
  readonly skills: readonly [string, string][];
}

/**
 * Declare an Eve agent to a gateway: the unit ({@link eveUnitToml}) through `POST /gateway/units/declare`
 * (`cap:write`), and each skill through `PUT /gateway/prompts/{agent}/{skill}` (`llm:write`, a node built with
 * `llm`) — the text as the system prompt, `{{input}}` as the user's turn. The tools are not declared here: a node
 * bridges the server {@link serveEveTools} runs.
 */
export async function declareEveAgent(
  agent: EveAgent,
  to: { mycelium: MyceliumAgent; prompts: PromptSkillClient; principal?: string; leaseSecs?: number },
): Promise<EveDeclaration> {
  const unit = await to.mycelium.declareUnits(eveUnitToml(agent, { principal: to.principal }), { leaseSecs: to.leaseSecs });
  const skills: [string, string][] = [];
  for (const skill of agent.skills) {
    await to.prompts.updatePrompt(agent.name, skill.name, {
      system: skill.text,
      userTemplate: "{{input}}",
      maxTokens: 512,
      temperature: 0,
      metadata: { source: "eve", eve_version: agent.eveVersion },
    });
    skills.push([agent.name, skill.name]);
  }
  return { unit, skills };
}

/** How {@link runEveSchedules} runs an agent's schedules. */
export interface RunSchedulesOptions {
  /** The holder band. Default: the capability {@link eveUnitToml} declares, `eve/<agent>` — the node the unit was declared
   *  on, which {@link declareEveAgent} puts in the band. */
  holders?: { ns: string; name: string };
  /** What a missed run is. Default `skip`: an Eve cron does not catch up. */
  onMissed?: ScheduleDeclaration["onMissed"];
  /**
   * Runs a Markdown schedule's prompt — Eve runs the agent on it and discards the output. The fleet's runtime for an
   * agent turn is not built here (row R routes inference, not agent turns), so a Markdown schedule needs this; without
   * it, {@link runEveSchedules} refuses the schedules by name before opening any stream.
   */
  onMarkdown?: (prompt: string, run: ScheduleRun, schedule: EveSchedule) => Promise<void> | void;
  signal?: AbortSignal;
}

/**
 * Run an Eve agent's schedules through the gateway (adopter-surface Y12): one `scheduleRuns` stream per schedule, the
 * node `mycelium` is connected to standing in its holder band. A `run` schedule's handler is called with Eve's
 * `waitUntil` (awaited before the run is acknowledged) and a `to` that throws — channels are not built; a Markdown
 * schedule's prompt goes to `onMarkdown`. A handler that throws is the run's failure, acknowledged as one. Resolves
 * when every stream ends (`signal`).
 */
export async function runEveSchedules(agent: EveAgent, mycelium: MyceliumAgent, options: RunSchedulesOptions = {}): Promise<void> {
  const markdownOnly = agent.schedules.filter((sc) => sc.form === "markdown");
  if (markdownOnly.length > 0 && !options.onMarkdown) {
    throw new EveRefusedError(markdownOnly.map((sc) => ({
      entry: `schedules/${sc.name}`,
      status: "not built" as const,
      reason: "a Markdown schedule runs the agent on its prompt, which the fleet does not do here: pass `onMarkdown`",
    })));
  }
  const holders = options.holders ?? { ns: "eve", name: agent.name };
  await Promise.all(agent.schedules.map((sc) => {
    const decl: ScheduleDeclaration = { id: sc.id, periodSecs: sc.periodSecs, phaseSecs: sc.phaseSecs, onMissed: options.onMissed ?? "skip", holders };
    return mycelium.runSchedule(decl, async (run) => {
      if (sc.form === "markdown") {
        await options.onMarkdown!(sc.markdown ?? "", run, sc);
        return;
      }
      const tasks: Promise<unknown>[] = [];
      await sc.run!({
        to: () => {
          throw new Error(`${sc.id}: \`to\` selects a channel, and channels are not built on the fleet (Y6)`);
        },
        waitUntil: (task) => {
          tasks.push(task);
        },
        appAuth: {},
      });
      await Promise.all(tasks);
    }, { signal: options.signal }).catch((e: unknown) => {
      if (!options.signal?.aborted) throw e;
    });
  }));
}
