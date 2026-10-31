/** Adopter-surface Y12, offline: the `[[schedule]]` a declaration sends. */

import { scheduleToml } from "../src/agent";

describe("scheduleToml", () => {
  it("is the unit file's schedule section", () => {
    expect(scheduleToml({ id: "close", periodSecs: 60, phaseSecs: 5, onMissed: "run_once", holders: { ns: "ledger", name: "closer" } })).toBe(
      '[[schedule]]\nid = "close"\nperiod_secs = 60\nphase_secs = 5\non_missed = "run_once"\n[schedule.holders]\nns = "ledger"\nname = "closer"\n',
    );
  });

  it("quotes a string so it cannot add a key", () => {
    const toml = scheduleToml({ id: 'a"\nperiod_secs = 1', periodSecs: 60, holders: { ns: "ledger", name: "closer" } });
    expect(toml.split("\n")[1]).toBe('id = "a\\"\\nperiod_secs = 1"');
  });

  it.each([1.5, -1, Number.NaN])("refuses a period that is not whole seconds (%p)", (bad) => {
    expect(() => scheduleToml({ id: "close", periodSecs: bad, holders: { ns: "ledger", name: "closer" } })).toThrow(RangeError);
  });
});

import { MyceliumAgent, UnservedRunError, tomlBasicString } from "../src/agent";
import type { ScheduleRun } from "../src/types";

describe("tomlBasicString", () => {
  it("escapes what TOML forbids raw and writes every other character as itself", () => {
    expect(tomlBasicString("a\"b\\c")).toBe('"a\\"b\\\\c"');
    expect(tomlBasicString("\b\t\n\f\r")).toBe('"\\b\\t\\n\\f\\r"');
    expect(tomlBasicString("\u0000\u0001\u001f")).toBe('"\\u0000\\u0001\\u001F"');
    expect(tomlBasicString("del\u007f")).toBe('"del\\u007F"'); // JSON.stringify leaves DEL raw
    expect(tomlBasicString("🌱 é")).toBe('"🌱 é"'); // a non-BMP character is one character, never a surrogate escape
  });
});

describe("runSchedule", () => {
  const run = (slot: number): ScheduleRun => ({
    runId: `close/${slot}/x`, schedule: "close", slot, kind: "on_time", firstSlot: slot, slotStartMs: slot * 1000,
    operationId: `sched/close/${slot}`,
  });

  it("goes on reading after an answer that does not land — refused or lost — each a missed run", async () => {
    const agent = new MyceliumAgent("127.0.0.1", 1);
    const handed = [run(1), run(2), run(3)];
    (agent as unknown as { scheduleRuns: () => AsyncGenerator<ScheduleRun> }).scheduleRuns = async function* () {
      yield* handed;
    };
    const answers: Array<() => Promise<void>> = [
      async () => { throw new UnservedRunError("close/1/x", "403 unserved_run"); },
      async () => { throw new TypeError("fetch failed"); },
      async () => {},
    ];
    (agent as unknown as { scheduleAck: () => Promise<void> }).scheduleAck = () => answers.shift()!();
    const worked: number[] = [];
    const ackErrors: unknown[] = [];
    await agent.runSchedule(
      { id: "close", periodSecs: 1, holders: { ns: "ledger", name: "closer" } },
      (r) => { worked.push(r.slot); },
      { onAckError: (_r, e) => ackErrors.push(e) },
    );
    expect(worked).toEqual([1, 2, 3]);
    expect(ackErrors.map((e) => (e as Error).name)).toEqual(["UnservedRunError", "TypeError"]);
  });
});
