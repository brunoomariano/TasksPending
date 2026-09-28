import "./styles.css";
import { loadSnapshot } from "./api";
import { startPolling } from "./poll";
import { renderApp } from "./render";

const POLL_INTERVAL_MS = 15_000;

const app = document.querySelector<HTMLElement>("#app");
if (app) {
  app.innerHTML = renderApp({ kind: "loading" });
  startPolling({
    load: () => loadSnapshot(),
    onState: (state) => {
      app.innerHTML = renderApp(state);
    },
    intervalMs: POLL_INTERVAL_MS,
    isHidden: () => document.hidden,
  });
}
