import { describe, expect, test } from "vitest";
import { SNOOZE_CHOICES, snoozeUntil, untilText } from "./snooze";

describe("snoozeUntil", () => {
  // A Wednesday afternoon, local time.
  const now = new Date(2026, 9, 7, 15, 30, 0);

  /** The menu offers an hour, tomorrow, next week, or waiting for a change. */
  test("offers four choices", () => {
    expect(SNOOZE_CHOICES.map((choice) => choice.key)).toEqual([
      "hour",
      "tomorrow",
      "week",
      "change",
    ]);
  });

  /** "1 hour" is an hour from now. */
  test("an hour from now", () => {
    expect(snoozeUntil("hour", now)).toEqual(new Date(2026, 9, 7, 16, 30, 0));
  });

  /** "Tomorrow" is the next day at 8 in the morning, local time. */
  test("tomorrow morning", () => {
    expect(snoozeUntil("tomorrow", now)).toEqual(new Date(2026, 9, 8, 8, 0, 0));
  });

  /**
   * "Next week" is the next Monday at 8, also when today is a Monday or the
   * weekend.
   */
  test("next monday morning", () => {
    const monday = new Date(2026, 9, 12, 8, 0, 0);
    expect(snoozeUntil("week", now)).toEqual(monday);
    expect(snoozeUntil("week", new Date(2026, 9, 5, 9, 0, 0))).toEqual(monday);
    expect(snoozeUntil("week", new Date(2026, 9, 11, 23, 0, 0))).toEqual(
      monday,
    );
  });

  /** "Until it changes" has no time. */
  test("until it changes has no time", () => {
    expect(snoozeUntil("change", now)).toBeNull();
  });

  /** The snoozed list says when each card comes back. */
  test("describes when a card comes back", () => {
    expect(untilText(null)).toBe("until it changes");
    expect(untilText("2026-10-08T11:00:00Z")).toMatch(/^until /);
  });
});
