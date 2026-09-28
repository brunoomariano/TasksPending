import "./styles.css";
import { loadSnapshot, requestRefresh } from "./api";
import { startPolling } from "./poll";
import { renderApp } from "./render";

const POLL_INTERVAL_MS = 15_000;
/** Sources refresh in the background; read the result shortly after. */
const AFTER_REFRESH_MS = 2_000;

const app = document.querySelector<HTMLElement>("#app");
if (app) {
  app.innerHTML = renderApp({ kind: "loading" });
  const poller = startPolling({
    load: () => loadSnapshot(),
    onState: (state) => {
      app.innerHTML = renderApp(state);
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
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>(
      '[data-action="refresh"]',
    );
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
    } else {
      button.disabled = false;
    }
  });
}
