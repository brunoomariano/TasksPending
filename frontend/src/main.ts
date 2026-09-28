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

/** The last chosen tab; a per-browser convenience, fine to lose. */
function savedBoard(): number {
  try {
    return Number(localStorage.getItem(BOARD_KEY)) || 0;
  } catch {
    return 0;
  }
}

function saveBoard(board: number): void {
  try {
    localStorage.setItem(BOARD_KEY, String(board));
  } catch {
    // Storage unavailable (private mode): the tab just isn't remembered.
  }
}

const app = document.querySelector<HTMLElement>("#app");
if (app) {
  let board = savedBoard();
  let shown: ViewState = { kind: "loading" };
  const draw = () => {
    app.innerHTML = renderApp(shown, board);
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
      board = Number(tab.dataset.board) || 0;
      saveBoard(board);
      draw();
      return;
    }
    const button = target.closest<HTMLButtonElement>('[data-action="refresh"]');
    if (!button) {
      return;
    }
    button.disabled = true;
    const result = await requestRefresh();
    button.textContent = result.ok
      ? "Refreshing…"
      : `Refresh (${result.error})`;
    if (result.ok) {
      setTimeout(() => poller.pollNow(), AFTER_REFRESH_MS);
      setTimeout(() => {
        button.disabled = false;
        button.textContent = "Refresh";
      }, REFRESH_COOLDOWN_MS);
    } else {
      button.disabled = false;
    }
  });
}
