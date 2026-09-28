import "./styles.css";
import { loadSnapshot, requestRefresh } from "./api";
import { startPolling } from "./poll";
import { renderApp } from "./render";
import type { ViewState } from "./state";

const POLL_INTERVAL_MS = 15_000;
/** Sources refresh in the background; read the result shortly after. */
const AFTER_REFRESH_MS = 2_000;
/** Matches the API cooldown; the button comes back even if nothing changed. */
const REFRESH_COOLDOWN_MS = 10_000;
const BOARD_KEY = "tasks-pending.board";

/** The last chosen tab's name; a per-browser convenience, fine to lose. */
function savedBoard(): string {
  try {
    return localStorage.getItem(BOARD_KEY) ?? "";
  } catch {
    return "";
  }
}

function saveBoard(name: string): void {
  try {
    localStorage.setItem(BOARD_KEY, name);
  } catch {
    // Storage unavailable (private mode): the tab just isn't remembered.
  }
}

const app = document.querySelector<HTMLElement>("#app");
if (app) {
  let boardName = savedBoard();
  let shown: ViewState = { kind: "loading" };
  /** While set, the Refresh button stays disabled with this label. */
  let refreshHold: { until: number; label: string } | null = null;

  const boardIndex = (): number => {
    if (shown.kind !== "ready" && shown.kind !== "stale") {
      return 0;
    }
    const index = shown.snapshot.boards.findIndex((b) => b.name === boardName);
    return Math.max(index, 0);
  };

  const draw = () => {
    app.innerHTML = renderApp(shown, boardIndex());
    const button = app.querySelector<HTMLButtonElement>(
      '[data-action="refresh"]',
    );
    if (button && refreshHold && Date.now() < refreshHold.until) {
      button.disabled = true;
      button.textContent = refreshHold.label;
    }
  };
  draw();
  const poller = startPolling({
    load: () => loadSnapshot(),
    onState: (state) => {
      shown = state;
      draw();
    },
    intervalMs: POLL_INTERVAL_MS,
    isHidden: () => document.hidden,
  });
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden) {
      poller.pollNow();
    }
  });
  app.addEventListener("click", async (event) => {
    const target = event.target as HTMLElement;
    const tab = target.closest<HTMLElement>("[data-board]");
    if (tab) {
      if (shown.kind === "ready" || shown.kind === "stale") {
        boardName =
          shown.snapshot.boards[Number(tab.dataset.board)]?.name ?? "";
        saveBoard(boardName);
      }
      draw();
      return;
    }
    const button = target.closest<HTMLButtonElement>('[data-action="refresh"]');
    if (!button) {
      return;
    }
    button.disabled = true;
    const result = await requestRefresh();
    if (result.ok) {
      refreshHold = {
        until: Date.now() + REFRESH_COOLDOWN_MS,
        label: "Refreshing…",
      };
      setTimeout(() => poller.pollNow(), AFTER_REFRESH_MS);
      setTimeout(() => {
        refreshHold = null;
        draw();
      }, REFRESH_COOLDOWN_MS);
      draw();
    } else {
      button.disabled = false;
      button.textContent = `Refresh (${result.error})`;
    }
  });
}
