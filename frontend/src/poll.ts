import type { SnapshotResult } from "./api";
import { nextState, type ViewState } from "./state";

export interface PollingOptions {
  load: () => Promise<SnapshotResult>;
  onState: (state: ViewState) => void;
  intervalMs: number;
  isHidden: () => boolean;
}

/**
 * Loads now and then every `intervalMs` while the page is visible. `onState`
 * runs only when what the dashboard shows changes. Returns a stop function.
 */
export function startPolling(options: PollingOptions): () => void {
  let state: ViewState = { kind: "loading" };
  let shownKey = "";
  let timer: ReturnType<typeof setTimeout> | undefined;
  let stopped = false;

  const tick = async () => {
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
    if (!stopped) {
      timer = setTimeout(tick, options.intervalMs);
    }
  };

  timer = setTimeout(tick, 0);
  return () => {
    stopped = true;
    clearTimeout(timer);
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
