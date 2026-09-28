import "./styles.css";
import { loadSnapshot, requestRefresh } from "./api";
import { startPolling } from "./poll";
import { renderApp, type View } from "./render";
import type { ViewState } from "./state";

const POLL_INTERVAL_MS = 15_000;
/** Sources refresh in the background; read the result shortly after. */
const AFTER_REFRESH_MS = 2_000;
/** Matches the API cooldown; the button comes back even if nothing changed. */
const REFRESH_COOLDOWN_MS = 10_000;
const VIEW_KEY = "tasks-pending.view";

/** Filter and expanded groups, remembered per browser; fine to lose. */
function savedView(): View {
  try {
    const saved = JSON.parse(localStorage.getItem(VIEW_KEY) ?? "{}") as {
      board?: string | null;
      expanded?: string[];
    };
    return {
      board: typeof saved.board === "string" ? saved.board : null,
      expanded: new Set(Array.isArray(saved.expanded) ? saved.expanded : []),
      sourcesOpen: false,
    };
  } catch {
    return { board: null, expanded: new Set(), sourcesOpen: false };
  }
}

function saveView(view: View): void {
  try {
    localStorage.setItem(
      VIEW_KEY,
      JSON.stringify({ board: view.board, expanded: [...view.expanded] }),
    );
  } catch {
    // Storage unavailable (private mode): the view just isn't remembered.
  }
}

const app = document.querySelector<HTMLElement>("#app");
if (app) {
  let view = savedView();
  let shown: ViewState = { kind: "loading" };
  /** While set, the Refresh button stays disabled with this label. */
  let refreshHold: { until: number; label: string } | null = null;

  const draw = () => {
    app.innerHTML = renderApp(shown, view);
    const button = app.querySelector<HTMLButtonElement>(
      '[data-action="refresh"]',
    );
    if (button && refreshHold && Date.now() < refreshHold.until) {
      button.disabled = true;
      button.textContent = refreshHold.label;
    }
  };
  const change = (next: Partial<View>) => {
    view = { ...view, ...next };
    saveView(view);
    draw();
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
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && view.sourcesOpen) {
      change({ sourcesOpen: false });
    }
  });
  app.addEventListener("click", async (event) => {
    const target = event.target as HTMLElement;

    const filter = target.closest<HTMLElement>("[data-board]");
    if (filter) {
      change({ board: filter.dataset.board || null });
      return;
    }
    const toggle = target.closest<HTMLElement>("[data-toggle-empty]");
    if (toggle) {
      const key = toggle.dataset.toggleEmpty ?? "";
      const expanded = new Set(view.expanded);
      if (!expanded.delete(key)) {
        expanded.add(key);
      }
      change({ expanded });
      return;
    }
    if (target.closest('[data-action="sources"]')) {
      change({ sourcesOpen: true });
      return;
    }
    // Close on the × button or a click on the backdrop itself.
    const close = target.closest<HTMLElement>('[data-action="close-sources"]');
    if (close && (close === target || close.classList.contains("close"))) {
      change({ sourcesOpen: false });
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
