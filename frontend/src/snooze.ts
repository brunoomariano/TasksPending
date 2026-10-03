/** Hiding a card for a while: the choices and when each one ends. */

export type SnoozeChoice = "hour" | "tomorrow" | "week" | "change";

export const SNOOZE_CHOICES: readonly { key: SnoozeChoice; label: string }[] = [
  { key: "hour", label: "1 hour" },
  { key: "tomorrow", label: "Tomorrow" },
  { key: "week", label: "Next week" },
  { key: "change", label: "Until it changes" },
];

/** Hour of the morning a snoozed card comes back, local time. */
const MORNING_HOUR = 8;

/**
 * When a card snoozed now comes back: in an hour, tomorrow morning, or next
 * Monday morning (local time). `null` waits for the item to change. A card
 * always comes back early when its item changes.
 */
export function snoozeUntil(choice: SnoozeChoice, now: Date): Date | null {
  const morning = (daysAhead: number) =>
    new Date(
      now.getFullYear(),
      now.getMonth(),
      now.getDate() + daysAhead,
      MORNING_HOUR,
    );
  switch (choice) {
    case "hour":
      return new Date(now.getTime() + 60 * 60 * 1000);
    case "tomorrow":
      return morning(1);
    case "week":
      // Days to the next Monday; a Monday goes to the following one.
      return morning((8 - now.getDay()) % 7 || 7);
    case "change":
      return null;
  }
}

/** "until Thu 08:00" for a time, "until it changes" without one. */
export function untilText(until: string | null): string {
  if (until === null) {
    return "until it changes";
  }
  const date = new Date(until);
  return Number.isNaN(date.getTime())
    ? "until later"
    : `until ${date.toLocaleString(undefined, {
        weekday: "short",
        day: "numeric",
        month: "short",
        hour: "2-digit",
        minute: "2-digit",
      })}`;
}
