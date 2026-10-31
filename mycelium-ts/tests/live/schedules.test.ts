/**
 * Adopter-surface Y12 against a live node: a schedule declared through the gateway, its runs received by the client.
 * The node the client is connected to stands in the schedule's holder band — it advertises the band's capability,
 * through the gateway — and hands the client each run; an acknowledged run is the slot run, a second answer is
 * refused, and a declaration the unit-file loader refuses is refused by name. Run as the gateway suite is.
 */

import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

import { MyceliumAgent, ScheduleRefusedError, UnservedRunError } from "../../src/agent";
import { EVE_PINNED_VERSION, loadEveAgent, runEveSchedules } from "../../src/eve";
import type { ScheduleDeclaration, ScheduleRun } from "../../src/types";

const TEST_HOST = process.env.MYCELIUM_TEST_HOST;
const TEST_PORT = parseInt(process.env.MYCELIUM_TEST_PORT ?? "0", 10);
if (process.env.MYCELIUM_LIVE_REQUIRED && !TEST_HOST) {
  throw new Error("MYCELIUM_LIVE_REQUIRED is set but MYCELIUM_TEST_HOST is not: the live suite would skip");
}
const describe_ = TEST_HOST ? describe : describe.skip;

describe_("schedules through the gateway (Y12)", () => {
  const agent = () => new MyceliumAgent(TEST_HOST!, TEST_PORT, 10_000);

  it("hands the client one run per slot, and an acknowledged run is the fleet's last run", async () => {
    const a = agent();
    const tag = Math.random().toString(16).slice(2, 10);
    const decl: ScheduleDeclaration = { id: `ts-${tag}`, periodSecs: 1, holders: { ns: "ts-sched", name: `holder-${tag}` } };
    const band = await a.advertiseCapability("ts-sched", `holder-${tag}`, { intervalSecs: 1 });
    const controller = new AbortController();
    try {
      const runs: ScheduleRun[] = [];
      for await (const run of a.scheduleRuns(decl, { signal: controller.signal })) {
        runs.push(run);
        await a.scheduleAck(run);
        if (runs.length === 2) break;
      }
      for (const run of runs) {
        expect(run.schedule).toBe(decl.id);
        expect(run.kind).toBe("on_time");
        expect(run.operationId).toBe(`sched/${decl.id}/${run.slot}`);
      }
      expect(runs[1].slot).toBeGreaterThan(runs[0].slot);
      await expect(a.scheduleAck(runs[0])).rejects.toBeInstanceOf(UnservedRunError);
      const node = (await a.health()).node_id;
      const last = await a.get(`sched/${decl.id}/last/${node}`);
      expect(last).not.toBeNull();
      expect(parseInt(last!.toString().split(" ")[0], 10)).toBeGreaterThanOrEqual(runs[1].slot);
    } finally {
      controller.abort();
      await band.drop();
    }
  }, 30_000);

  it("runSchedule answers each run with the work's outcome", async () => {
    const a = agent();
    const tag = Math.random().toString(16).slice(2, 10);
    const decl: ScheduleDeclaration = { id: `ts-run-${tag}`, periodSecs: 1, holders: { ns: "ts-sched", name: `holder-${tag}` } };
    const band = await a.advertiseCapability("ts-sched", `holder-${tag}`, { intervalSecs: 1 });
    const controller = new AbortController();
    const seen: string[] = [];
    try {
      await a.runSchedule(decl, (run) => {
        seen.push(run.operationId);
        if (seen.length === 2) controller.abort();
      }, { signal: controller.signal }).catch((e: unknown) => {
        if (!controller.signal.aborted) throw e;
      });
      expect(seen.length).toBeGreaterThanOrEqual(2);
      expect(new Set(seen).size).toBe(seen.length);
    } finally {
      await band.drop();
    }
  }, 30_000);

  it("refuses a declaration the unit-file loader refuses, by name", async () => {
    const a = agent();
    const bad: ScheduleDeclaration = { id: "ts-bad", periodSecs: 0, holders: { ns: "ts-sched", name: "holder" } };
    const consume = async () => {
      for await (const _ of a.scheduleRuns(bad)) { /* never */ }
    };
    const err = await consume().catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ScheduleRefusedError);
    expect((err as ScheduleRefusedError).status).toBe(422);
    expect((err as ScheduleRefusedError).detail).toContain("period_secs");
  });

  it("runs an Eve schedule loaded by Y6's loader: its handler called, waitUntil awaited, the run acknowledged", async () => {
    const a = agent();
    const tag = Math.random().toString(16).slice(2, 10);
    const project = fs.mkdtempSync(path.join(os.tmpdir(), "eve-sched-"));
    const dir = path.join(project, "agent");
    fs.mkdirSync(path.join(dir, "schedules"), { recursive: true });
    fs.writeFileSync(path.join(dir, "instructions.md"), "You keep a heartbeat.\n");
    const marker = path.join(project, "ran");
    fs.writeFileSync(path.join(dir, "schedules", "heartbeat.ts"), `
      const fs = require("node:fs");
      export default {
        cron: "* * * * *",
        async run({ waitUntil }: { waitUntil: (p: Promise<unknown>) => void }) {
          waitUntil(new Promise<void>((done) => setTimeout(() => { fs.writeFileSync(${JSON.stringify(marker)}, "ran"); done(); }, 50)));
        },
      };
    `);
    const eve = await loadEveAgent(dir, { name: `beat-${tag}`, importModule: async (f) => require(f), resolveEveVersion: () => EVE_PINNED_VERSION });
    expect(eve.schedules.map((sc) => [sc.id, sc.periodSecs, sc.phaseSecs])).toEqual([[`beat-${tag}.heartbeat`, 60, 0]]);
    // The node stands in the band the unit's capability names: `eve/<agent>`.
    const band = await a.advertiseCapability("eve", eve.name, { intervalSecs: 1 });
    const controller = new AbortController();
    try {
      const running = runEveSchedules(eve, a, { signal: controller.signal });
      const deadline = Date.now() + 40_000;
      while (!fs.existsSync(marker) && Date.now() < deadline) await new Promise((r) => setTimeout(r, 200));
      expect(fs.existsSync(marker)).toBe(true);
      // Acknowledged: the slot is the fleet's last run once `waitUntil`'s task settled.
      const node = (await a.health()).node_id;
      let last: Buffer | null = null;
      while (last === null && Date.now() < deadline) {
        last = await a.get(`sched/${eve.schedules[0].id}/last/${node}`);
        if (last === null) await new Promise((r) => setTimeout(r, 200));
      }
      expect(last).not.toBeNull();
      controller.abort();
      await running;
    } finally {
      controller.abort();
      await band.drop();
      fs.rmSync(project, { recursive: true, force: true });
    }
  }, 60_000);

  it("tells the client its node is not in the band: a not_eligible status", async () => {
    const a = agent();
    const decl: ScheduleDeclaration = { id: `ts-none-${Math.random().toString(16).slice(2, 10)}`, periodSecs: 1, holders: { ns: "ts-sched", name: "nobody-advertises-this" } };
    const controller = new AbortController();
    const said: string[] = [];
    const reading = (async () => {
      for await (const _ of a.scheduleRuns(decl, { signal: controller.signal, onStatus: (r) => { said.push(r); controller.abort(); } })) {
        throw new Error("a node outside the band hands out no run");
      }
    })().catch((e: unknown) => { if (!controller.signal.aborted) throw e; });
    const deadline = Date.now() + 10_000;
    while (said.length === 0 && Date.now() < deadline) await new Promise((r) => setTimeout(r, 100));
    controller.abort();
    await reading;
    expect(said[0]).toBe("not_eligible");
  }, 20_000);
});
