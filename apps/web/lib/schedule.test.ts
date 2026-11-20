import assert from "node:assert/strict";
import { test } from "node:test";

import { DEFAULT_MODEL, buildCron, cronError, describeSchedule, nextRuns, parseCron } from "./schedule.ts";

test("simple shapes round-trip through the model", () => {
  const cases: Array<[string, string]> = [
    ["*/30 * * * *", "minutes"],
    ["* * * * *", "minutes"],
    ["0 * * * *", "hourly"],
    ["15 */2 * * *", "hourly"],
    ["0 9 * * *", "daily"],
    ["0 9 * * 1-5", "weekly"],
    ["30 7 * * 0,6", "weekly"],
    ["0 9 * * 1,3,5", "weekly"],
    ["0 9 1 * *", "monthly"],
  ];
  for (const [cron, frequency] of cases) {
    const model = parseCron(cron);
    assert.equal(model.frequency, frequency, cron);
    assert.equal(buildCron(model), cron, cron);
  }
});

test("other expressions stay custom", () => {
  for (const cron of ["0 9 15 * 1", "0 9 * 1 *", "0 9 * * MON", "0 0 9 * * *", "nonsense"]) {
    assert.equal(parseCron(cron).frequency, "custom", cron);
  }
});

test("day lists are normalized", () => {
  assert.equal(parseCron("5 4 * * 7").days.join(), "0");
  assert.equal(parseCron("0 9 * * 1,2,3,4,5,6,0").frequency, "daily");
  assert.equal(buildCron({ ...DEFAULT_MODEL, frequency: "weekly", days: [1, 3, 4, 5] }), "0 9 * * 1,3-5");
});

test("descriptions name the time zone of clock times", () => {
  assert.equal(describeSchedule("0 9 * * 1-5", "Europe/Amsterdam"), "Every weekday at 09:00 Amsterdam time");
  assert.equal(describeSchedule("0 9 * * 1-5"), "Every weekday at 09:00 UTC");
  assert.equal(describeSchedule("*/15 * * * *", "Europe/Amsterdam"), "Every 15 minutes");
  assert.equal(describeSchedule("0 9 31 * *", "UTC"), "On the 31st of every month at 09:00 UTC");
  assert.equal(describeSchedule("0 9 15 * 1"), "Cron 0 9 15 * 1 (UTC)");
});

test("next runs follow the time zone, including daylight saving", () => {
  const winter = nextRuns("0 9 * * *", "Europe/Amsterdam", 1, new Date("2026-01-15T00:00:00Z"));
  const summer = nextRuns("0 9 * * *", "Europe/Amsterdam", 1, new Date("2026-07-15T00:00:00Z"));
  assert.equal(winter[0].toISOString(), "2026-01-15T08:00:00.000Z");
  assert.equal(summer[0].toISOString(), "2026-07-15T07:00:00.000Z");
  assert.deepEqual(nextRuns("61 * * * *", null), []);
});

test("invalid expressions explain themselves", () => {
  assert.equal(cronError("0 9 * * 1-5"), null);
  assert.match(cronError("0 9 * *") ?? "", /five fields/);
  assert.match(cronError("61 9 * * *") ?? "", /minute/);
  assert.equal(cronError("0 9 31 2 *"), "This schedule never runs.");
});

test("a step that does not divide the period is not an even interval", () => {
  // */45 fires at :00 and :45; */7 hours restarts at midnight.
  assert.equal(parseCron("*/45 * * * *").frequency, "custom");
  assert.equal(parseCron("0 */7 * * *").frequency, "custom");
  assert.equal(parseCron("*/20 * * * *").frequency, "minutes");
  assert.equal(parseCron("0 */6 * * *").frequency, "hourly");
});
