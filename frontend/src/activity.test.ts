import { describe, expect, test, vi } from "vitest";
import { ACTIVITY_EVENTS, startActivity } from "./activity";

/** A stand-in for `window` that records listeners. */
function fakeTarget() {
  const listeners = new Map<string, () => void>();
  return {
    listeners,
    addEventListener: (type: string, listener: () => void) =>
      listeners.set(type, listener),
    removeEventListener: (type: string) => listeners.delete(type),
  };
}

describe("startActivity", () => {
  /**
   * Looking at the page is any activity on it; it is reported right away and
   * then at most once a minute, however much the mouse moves.
   */
  test("reports activity at once and then at most once a minute", () => {
    const target = fakeTarget();
    const report = vi.fn();
    let now = 1_000_000;

    startActivity(report, { target, now: () => now });
    expect([...target.listeners.keys()].sort()).toEqual(
      [...ACTIVITY_EVENTS].sort(),
    );

    target.listeners.get("pointermove")!();
    target.listeners.get("pointermove")!();
    target.listeners.get("keydown")!();
    expect(report).toHaveBeenCalledTimes(1);

    now += 59_000;
    target.listeners.get("pointerdown")!();
    expect(report).toHaveBeenCalledTimes(1);

    now += 1_000;
    target.listeners.get("focus")!();
    expect(report).toHaveBeenCalledTimes(2);
  });

  /** Stopping removes the listeners. */
  test("stop removes the listeners", () => {
    const target = fakeTarget();

    startActivity(() => {}, { target, now: () => 0 })();

    expect(target.listeners.size).toBe(0);
  });
});
