import "./styles.css";
import { loadSnapshot } from "./api";
import { renderApp } from "./render";

loadSnapshot().then((result) => {
  const app = document.querySelector<HTMLElement>("#app");
  if (app) {
    app.innerHTML = renderApp(result);
  }
});
