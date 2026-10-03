// Takes the README screenshots from a running sandbox, driving a headless
// Chromium over its DevTools port. No dependencies: Node's own fetch and
// WebSocket. Run it through `make screenshots` (scripts/screenshots.sh),
// which starts the sandbox and the browser.
//
// Needs Node 22 or newer (the global WebSocket).
//
//   node scripts/screenshots.mjs <sandbox url> <devtools port> <output dir>
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const [url, port, out] = process.argv.slice(2);
if (!url || !port || !out) {
  console.error("usage: screenshots.mjs <sandbox url> <devtools port> <output dir>");
  process.exit(2);
}
mkdirSync(out, { recursive: true });

// The clock and every relative time show this moment, so the pictures do
// not change from one run to the next. It is shortly after the sandbox's
// data was "fetched".
const NOW = "2026-09-30T11:41:07Z";
const WIDTH = 1700;
const SCALE = 1.5;

const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
const page = targets.find((target) => target.type === "page");
if (!page) {
  console.error(`no page to drive on DevTools port ${port}; is it the browser this script started?`);
  process.exit(1);
}
const socket = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.onopen = resolve;
  socket.onerror = reject;
});
let sequence = 0;
const pending = new Map();
socket.onmessage = (message) => {
  const reply = JSON.parse(message.data);
  const done = pending.get(reply.id);
  if (done) {
    pending.delete(reply.id);
    reply.error ? done.reject(new Error(reply.error.message)) : done.resolve(reply.result);
  }
};
const send = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const id = ++sequence;
    pending.set(id, { resolve, reject });
    socket.send(JSON.stringify({ id, method, params }));
  });
const evaluate = async (expression) => {
  const result = await send("Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true,
  });
  if (result.exceptionDetails) {
    throw new Error(result.exceptionDetails.exception?.description ?? "evaluation failed");
  }
  return result.result.value;
};
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** Waits until `expression` is true in the page. */
async function until(expression, what) {
  for (let attempt = 0; attempt < 100; attempt++) {
    if (await evaluate(`Boolean(${expression})`)) return;
    await sleep(100);
  }
  throw new Error(`timed out waiting for ${what}`);
}

const click = (selector) =>
  evaluate(`(() => {
    const element = document.querySelector(${JSON.stringify(selector)});
    if (!element) throw new Error("nothing matches ${selector.replaceAll('"', "'")}");
    element.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  })()`);

/** Calls one of the sandbox's state-changing routes, as the page does. */
const post = (path, body) =>
  evaluate(`fetch(${JSON.stringify(path)}, {
    method: "POST",
    headers: { "content-type": "application/json", "x-requested-with": "tasks-pending" },
    body: ${JSON.stringify(JSON.stringify(body))},
  }).then((response) => { if (!response.ok) throw new Error(${JSON.stringify(path)} + " " + response.status); })`);

async function theme(scheme) {
  await send("Emulation.setEmulatedMedia", {
    features: [{ name: "prefers-color-scheme", value: scheme }],
  });
}

// A headless browser reports no hover, so the page keeps every card's
// buttons visible, as on a touch screen. The pictures show a desktop, where
// they appear on hover: only pressed or open buttons stay.
const DESKTOP_STYLE = `
  .mark:not([aria-pressed="true"]):not([aria-expanded="true"]) { opacity: 0 !important; }
`;

async function load() {
  await send("Page.navigate", { url });
  await until('document.querySelector(".board .card")', "the cards");
  // The sandbox flags its newest cards: without the dots the pictures would
  // silently lose a feature.
  await until('document.querySelector(".changed-dot")', "the changed dots");
  // Logos and the weather widget arrive after the first draw. Logos below
  // the window are lazy; the pictures take the whole page, so load them all.
  await until(
    `[...document.images].every((image) => {
      image.loading = "eager";
      return image.complete && image.naturalWidth > 0;
    })`,
    "the source icons",
  );
  await until('document.querySelector(".weather")', "the weather widget");
  await evaluate(`(() => {
    const style = document.createElement("style");
    style.textContent = ${JSON.stringify(DESKTOP_STYLE)};
    document.head.append(style);
  })()`);
  await sleep(300);
}

/** Saves the page (the whole document, or the viewport when `height` is set). */
async function shot(name, height) {
  const full = await evaluate("document.documentElement.scrollHeight");
  const { data } = await send("Page.captureScreenshot", {
    format: "png",
    captureBeyondViewport: true,
    clip: { x: 0, y: 0, width: WIDTH, height: height ?? full, scale: 1 },
  });
  writeFileSync(join(out, name), Buffer.from(data, "base64"));
  console.log(`  ${name}`);
}

await send("Page.enable");
await send("Emulation.setDeviceMetricsOverride", {
  width: WIDTH,
  height: 1000,
  deviceScaleFactor: SCALE,
  mobile: false,
});
await send("Emulation.setTimezoneOverride", { timezoneId: "UTC" });
await send("Emulation.setLocaleOverride", { locale: "en-US" });
await send("Page.addScriptToEvaluateOnNewDocument", {
  source: `(() => {
    const fixed = new Date(${JSON.stringify(NOW)}).getTime();
    const Real = Date;
    class Fixed extends Real {
      constructor(...args) { args.length ? super(...args) : super(fixed); }
      static now() { return fixed; }
    }
    window.Date = Fixed;
  })();`,
});

await theme("dark");
await load();
const ids = await evaluate(
  `fetch("/api/v1/snapshot").then((r) => r.json()).then((s) =>
    s.boards.flatMap((b) => b.groups).flatMap((g) => g.columns).flatMap((c) => c.cards).map((c) => c.id))`,
);
const need = (id) => {
  if (!ids.includes(id)) throw new Error(`the sandbox has no card ${id}`);
  return id;
};

// The dashboard: two cards marked as in progress, the newest ones flagged
// as changed since the last look.
await post("/api/v1/marks", { id: need("github:pull:220"), marked: true });
await post("/api/v1/marks", { id: need("linear:ENG-142"), marked: true });
await load();
await until('document.querySelectorAll(".now .card").length === 2', "the Now row");
console.log("screenshots:");
await shot("tasks-pending-sandbox.png");
await theme("light");
await sleep(300);
await shot("tasks-pending-sandbox-light.png");
await theme("dark");

// Snoozing: the menu open on a card.
await click(`[data-snooze-menu="${need("github:review:214")}"]`);
await until('document.querySelector(".snooze-menu")', "the snooze menu");
// Heights: enough of the page to show the open menu or dialog whole.
await shot("tasks-pending-snooze.png", 780);
await click(`[data-snooze-menu="github:review:214"]`);

// The snoozed list. The sandbox checks a snooze time against the real
// clock (only the page's clock is fixed), so the timed one ends on a real
// Monday morning at least two days ahead: its label in the picture is the
// one text that changes from run to run.
const monday = new Date(Date.now() + 2 * 86_400_000);
monday.setUTCDate(monday.getUTCDate() + ((8 - monday.getUTCDay()) % 7));
monday.setUTCHours(8, 0, 0, 0);
await post("/api/v1/snooze", { id: need("github:review:218"), snoozed: true, until: null });
await post("/api/v1/snooze", {
  id: need("todoist:later:1"),
  snoozed: true,
  until: monday.toISOString(),
});
await load();
await click('[data-action="snoozed"]');
await until('document.querySelectorAll(".snoozed-row").length === 2', "the snoozed list");
await shot("tasks-pending-snoozed.png", 700);
await click(".snoozed-modal .close");

// Source health and settings.
await click('[data-action="sources"]');
await until('document.querySelector(".sources-modal")', "the sources dialog");
await shot("tasks-pending-sources.png", 700);
await click(".sources-modal .close");
await click('[data-action="settings"]');
await until('document.querySelector(".settings-modal")', "the settings dialog");
await shot("tasks-pending-settings.png", 1000);

socket.close();
