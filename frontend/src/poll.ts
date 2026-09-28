import type { SnapshotResult } from "./api";
import { nextState, type ViewState } from "./state";

export interface PollingOptions {
  load: () => Promise<SnapshotResult>;
  onState: (state: ViewState) => void;
  intervalMs: number;
  isHidden: () => boolean;
}

export interface Poller {
  stop: () => void;
  /** Loads right away (e.g. when the tab becomes visible again). */
  pollNow: () => void;
}

/**
 * Loads now and then every `intervalMs` while the page is visible. `onState`
 * runs only when what the dashboard shows changes. A failing `onState` does
 * not stop polling.
 */
export function startPolling(options: PollingOptions): Poller {
  let state: ViewState = { kind: "loading" };
  let shownKey = "";
  let timer: ReturnType<typeof setTimeout> | undefined;
  let inFlight = false;
  let stopped = false;

  const schedule = (delayMs: number) => {
    clearTimeout(timer);
    timer = setTimeout(tick, delayMs);
  };

  const tick = async () => {
    inFlight = true;
    try {
      if (!options.isHidden()) {
        const result = await options.load();
        if (stopped) {
          return;
        }
        state = nextState(state, result, new Date());
        const key = viewKey(state);
        if (key !== shownKey) {
          shownKey = key;
          options.onState(state);
        }
      }
    } catch (error) {
      console.error("dashboard update failed", error);
    } finally {
      inFlight = false;
      if (!stopped) {
        schedule(options.intervalMs);
      }
    }
  };

  schedule(0);
  return {
    stop: () => {
      stopped = true;
      clearTimeout(timer);
    },
    pollNow: () => {
      if (!stopped && !inFlight) {
        schedule(0);
      }
    },
  };
}

/** Identity of what is on screen; `generated_at` alone does not count. */
function viewKey(state: ViewState): string {
  switch (state.kind) {
    case "loading":
      return "loading";
    case "unavailable":
      return `unavailable:${state.error}`;
    case "ready":
    case "stale": {
      const { lanes, sources } = state.snapshot;
      const error = state.kind === "stale" ? state.error : "";
      return JSON.stringify([state.kind, error, lanes, sources]);
    }
  }
}
