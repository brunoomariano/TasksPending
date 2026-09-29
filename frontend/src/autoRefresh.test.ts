import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { startAutoRefresh } from "./autoRefresh";

describe("startAutoRefresh", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  /** Every N minutes the page asks the API to refresh every source. */
  test("refreshes every interval", async () => {
    const refresh = vi.fn(async () => {});
    const stop = startAutoRefresh(1, refresh, () => false);

    await vi.advanceTimersByTimeAsync(59_000);
    expect(refresh).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1_000);
    expect(refresh).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(refresh).toHaveBeenCalledTimes(2);

    stop();
    await vi.advanceTimersByTimeAsync(120_000);
    expect(refresh).toHaveBeenCalledTimes(2);
  });

  /** A hidden tab does not spend provider requests; off means never. */
  test("skips hidden tabs and does nothing when off", async () => {
    const refresh = vi.fn(async () => {});
    startAutoRefresh(1, refresh, () => true);
    startAutoRefresh(0, refresh, () => false);

    await vi.advanceTimersByTimeAsync(180_000);

    expect(refresh).not.toHaveBeenCalled();
  });

  /** A failed refresh does not stop the next ones. */
  test("keeps going after a failure", async () => {
    const refresh = vi.fn(async () => {
      throw new Error("API down");
    });
    startAutoRefresh(1, refresh, () => false);

    await vi.advanceTimersByTimeAsync(120_000);

    expect(refresh).toHaveBeenCalledTimes(2);
  });
});
