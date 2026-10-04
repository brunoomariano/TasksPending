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
   * Looking at the page is doing something on it; it is reported right away
   * and then at most once a minute, however many events arrive.
   */
  test("reports activity at once and then at most once a minute", () => {
    const target = fakeTarget();
    const report = vi.fn();
    let now = 1_000_000;

    startActivity(report, { target, now: () => now });
    expect([...target.listeners.keys()].sort()).toEqual(
      [...ACTIVITY_EVENTS].sort(),
    );

    target.listeners.get("pointerdown")!();
    target.listeners.get("pointerdown")!();
    target.listeners.get("keydown")!();
    expect(report).toHaveBeenCalledTimes(1);

    now += 59_000;
    target.listeners.get("pointerdown")!();
    expect(report).toHaveBeenCalledTimes(1);

    now += 1_000;
    target.listeners.get("keydown")!();
    expect(report).toHaveBeenCalledTimes(2);
  });

  /**
   * The pointer crossing the window, or the window taking focus under it,
   * is not looking: nothing listens to those, so they cannot start a new
   * sitting and clear the changed dots unseen.
   */
  test("pointer movement and focus alone are not activity", () => {
    const target = fakeTarget();

    startActivity(() => {}, { target, now: () => 0 });

    for (const type of ["pointermove", "mousemove", "focus", "pointerenter"]) {
      expect(target.listeners.has(type), type).toBe(false);
    }
  });

  /**
   * Reading by scrolling is looking too: the wheel, a touch and the page
   * scrolling all count, so a long read is not mistaken for time away.
   */
  test("scrolling and touch count as activity", () => {
    for (const type of ["wheel", "scroll", "touchstart"]) {
      const target = fakeTarget();
      const report = vi.fn();
      startActivity(report, { target, now: () => 0 });

      target.listeners.get(type)?.();

      expect(report, type).toHaveBeenCalledTimes(1);
    }
  });

  /** Stopping removes the listeners. */
  test("stop removes the listeners", () => {
    const target = fakeTarget();

    startActivity(() => {}, { target, now: () => 0 })();

    expect(target.listeners.size).toBe(0);
  });
});
