import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import type { SnapshotResult } from "./api";
import type { DashboardSnapshot } from "./contract.gen";
import { startPolling } from "./poll";
import type { ViewState } from "./state";

const snapshot = (generatedAt: string, title = "a"): DashboardSnapshot => ({
  generated_at: generatedAt,
  config_error: null,
  marked: [],
  sources: [],
  boards: [
    {
      name: "Work",
      groups: [
        {
          source: "github",
          icon: null,
          columns: [
            {
              name: "Review",
              cards: [
                {
                  id: "a",
                  title,
                  body: "",
                  source: "github",
                  url: null,
                  severity: "info",
                  due_at: null,
                  updated_at: "2026-09-28T09:00:00Z",
                },
              ],
            },
          ],
        },
      ],
    },
  ],
});

function always(result: SnapshotResult) {
  return vi.fn(async () => result);
}

describe("startPolling", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  /**
   * The dashboard polls the API on an interval and redraws when the data
   * changes; a snapshot equal to the previous one (only generated_at differs)
   * does not redraw, so scroll and focus are not lost for nothing.
   */
  test("polls on the interval and re-renders only on changes", async () => {
    const results: SnapshotResult[] = [
      { ok: true, snapshot: snapshot("10:00") },
      { ok: true, snapshot: snapshot("10:01") },
      { ok: true, snapshot: snapshot("10:02", "changed") },
    ];
    const load = vi.fn(async () => results.shift()!);
    const states: ViewState[] = [];

    const { stop } = startPolling({
      load,
      onState: (state) => states.push(state),
      intervalMs: 1000,
      isHidden: () => false,
    });

    await vi.advanceTimersByTimeAsync(0);
    await vi.advanceTimersByTimeAsync(1000);
    await vi.advanceTimersByTimeAsync(1000);
    stop();

    expect(load).toHaveBeenCalledTimes(3);
    expect(states.map((s) => s.kind)).toEqual(["ready", "ready"]);
  });

  /** A failure after a success becomes the stale state, keeping the cards. */
  test("a failed poll after success renders the stale state", async () => {
    const results: SnapshotResult[] = [
      { ok: true, snapshot: snapshot("10:00") },
      { ok: false, error: "HTTP 502" },
    ];
    const load = vi.fn(async () => results.shift()!);
    const states: ViewState[] = [];

    const { stop } = startPolling({
      load,
      onState: (state) => states.push(state),
      intervalMs: 1000,
      isHidden: () => false,
    });
    await vi.advanceTimersByTimeAsync(0);
    await vi.advanceTimersByTimeAsync(1000);
    stop();

    expect(states.map((s) => s.kind)).toEqual(["ready", "stale"]);
  });

  /** With the tab hidden, the dashboard does not poll the API. */
  test("does not poll while the tab is hidden", async () => {
    const load = always({ ok: true, snapshot: snapshot("10:00") });
    let hidden = false;

    const { stop } = startPolling({
      load,
      onState: () => {},
      intervalMs: 1000,
      isHidden: () => hidden,
    });
    await vi.advanceTimersByTimeAsync(0);
    hidden = true;
    await vi.advanceTimersByTimeAsync(5000);
    stop();

    expect(load).toHaveBeenCalledTimes(1);
  });

  /** After stopping, no new poll happens. */
  test("stop cancels future polls", async () => {
    const load = always({ ok: true, snapshot: snapshot("10:00") });
    const { stop } = startPolling({
      load,
      onState: () => {},
      intervalMs: 1000,
      isHidden: () => false,
    });
    await vi.advanceTimersByTimeAsync(0);
    stop();
    await vi.advanceTimersByTimeAsync(5000);

    expect(load).toHaveBeenCalledTimes(1);
  });

  /** A rendering error does not stop the next polls. */
  test("keeps polling when rendering throws", async () => {
    const results: SnapshotResult[] = [
      { ok: true, snapshot: snapshot("10:00") },
      { ok: true, snapshot: snapshot("10:01", "changed") },
    ];
    const load = vi.fn(async () => results.shift()!);
    let calls = 0;

    const poller = startPolling({
      load,
      onState: () => {
        calls += 1;
        if (calls === 1) throw new Error("render bug");
      },
      intervalMs: 1000,
      isHidden: () => false,
    });
    await vi.advanceTimersByTimeAsync(0);
    await vi.advanceTimersByTimeAsync(1000);
    poller.stop();

    expect(load).toHaveBeenCalledTimes(2);
    expect(calls).toBe(2);
  });

  /**
   * When the tab becomes visible again, the dashboard updates right away,
   * without waiting for the interval, and without doubling the scheduled poll.
   */
  test("pollNow refreshes immediately without doubling the schedule", async () => {
    const load = always({ ok: true, snapshot: snapshot("10:00") });
    const poller = startPolling({
      load,
      onState: () => {},
      intervalMs: 1000,
      isHidden: () => false,
    });
    await vi.advanceTimersByTimeAsync(0);

    poller.pollNow();
    await vi.advanceTimersByTimeAsync(0);
    expect(load).toHaveBeenCalledTimes(2);

    await vi.advanceTimersByTimeAsync(999);
    expect(load).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(1);
    expect(load).toHaveBeenCalledTimes(3);
    poller.stop();
  });

  /**
   * If drawing failed, the next poll tries again even with the same data,
   * instead of treating the screen as up to date.
   */
  test("retries rendering the same data after a render failure", async () => {
    const load = always({ ok: true, snapshot: snapshot("10:00") });
    let calls = 0;

    const poller = startPolling({
      load,
      onState: () => {
        calls += 1;
        if (calls === 1) throw new Error("render bug");
      },
      intervalMs: 1000,
      isHidden: () => false,
    });
    await vi.advanceTimersByTimeAsync(0);
    await vi.advanceTimersByTimeAsync(1000);
    await vi.advanceTimersByTimeAsync(1000);
    poller.stop();

    expect(calls).toBe(2);
  });
});
