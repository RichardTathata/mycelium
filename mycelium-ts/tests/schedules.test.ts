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
