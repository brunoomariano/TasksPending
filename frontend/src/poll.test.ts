import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import type { SnapshotResult } from "./api";
import type { DashboardSnapshot } from "./contract.gen";
import { startPolling } from "./poll";
import type { ViewState } from "./state";

const snapshot = (generatedAt: string, title = "a"): DashboardSnapshot => ({
  generated_at: generatedAt,
  sources: [],
  lanes: [
    {
      name: "Work",
      sections: [
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
              updated_at: "2026-09-28T09:00:00Z",
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
   * O dashboard consulta a API no intervalo e redesenha quando os dados
   * mudam; um snapshot igual ao anterior (só com outro generated_at) não
   * redesenha, para não perder scroll nem foco à toa.
   */
  test("polls on the interval and re-renders only on changes", async () => {
    const results: SnapshotResult[] = [
      { ok: true, snapshot: snapshot("10:00") },
      { ok: true, snapshot: snapshot("10:01") },
      { ok: true, snapshot: snapshot("10:02", "changed") },
    ];
    const load = vi.fn(async () => results.shift()!);
    const states: ViewState[] = [];

    const stop = startPolling({
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

  /** Uma falha depois de um sucesso vira estado antigo, sem sumir com os cards. */
  test("a failed poll after success renders the stale state", async () => {
    const results: SnapshotResult[] = [
      { ok: true, snapshot: snapshot("10:00") },
      { ok: false, error: "HTTP 502" },
    ];
    const load = vi.fn(async () => results.shift()!);
    const states: ViewState[] = [];

    const stop = startPolling({
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

  /** Com a aba escondida, o dashboard não consulta a API. */
  test("does not poll while the tab is hidden", async () => {
    const load = always({ ok: true, snapshot: snapshot("10:00") });
    let hidden = false;

    const stop = startPolling({
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

  /** Depois de parar, nenhuma consulta nova acontece. */
  test("stop cancels future polls", async () => {
    const load = always({ ok: true, snapshot: snapshot("10:00") });
    const stop = startPolling({
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
});
