import { describe, expect, test } from "vitest";
import type { DashboardSnapshot, PendingCard } from "./contract.gen";
import {
  COLUMN_LIMIT,
  columnKey,
  DEFAULT_VIEW,
  groupKey,
  hideKey,
  renderApp,
  renderControls,
  type View,
} from "./render";

const card = (id: string, extra: Partial<PendingCard> = {}): PendingCard => ({
  id,
  title: id,
  body: "",
  source: "test",
  url: null,
  due_at: null,
  severity: "info",
  updated_at: "2026-09-28T09:00:00Z",
  ...extra,
});

const snapshot = (): DashboardSnapshot => ({
  generated_at: "2026-09-28T10:00:00Z",
  config_error: null,
  sources: [
    {
      name: "plane",
      status: "ready",
      last_refresh_at: "2026-09-28T09:55:00Z",
      message: null,
    },
    {
      name: "github",
      status: "ready",
      last_refresh_at: "2026-09-28T09:50:00Z",
      message: null,
    },
    {
      name: "todoist",
      status: "ready",
      last_refresh_at: "2026-09-28T09:40:00Z",
      message: null,
    },
  ],
  boards: [
    {
      name: "Work",
      groups: [
        {
          source: "plane",
          icon: null,
          columns: [
            {
              name: "Mine",
              cards: [
                card("API-1", {
                  title: "<script>alert(1)</script>",
                  url: "https://plane.example/acme/projects/p/issues/1",
                  severity: "critical",
                }),
              ],
            },
            { name: "Empty inbox", cards: [] },
          ],
        },
        {
          source: "github",
          icon: null,
          columns: [
            { name: "Review", cards: [card("gh-1")] },
            { name: "My PRs", cards: [] },
            { name: "Notifications", cards: [] },
          ],
        },
      ],
    },
    {
      name: "Personal",
      groups: [
        {
          source: "todoist",
          icon: null,
          columns: [{ name: "Today", cards: [card("todo-1")] }],
        },
      ],
    },
  ],
});

const view = (extra: Partial<View> = {}): View => ({
  ...DEFAULT_VIEW,
  ...extra,
});

const ready = (v: View = view(), data: DashboardSnapshot = snapshot()) =>
  renderApp(
    {
      kind: "ready",
      fetchedAt: new Date("2026-09-28T10:00:00Z"),
      snapshot: data,
    },
    v,
  );

/** The buttons and last update shown next to the clock. */
const controls = (data: DashboardSnapshot = snapshot(), v: View = view()) =>
  renderControls(
    {
      kind: "ready",
      fetchedAt: new Date("2026-09-28T10:00:00Z"),
      snapshot: data,
    },
    v,
  );

describe("renderApp", () => {
  /**
   * With the API down, the page says it is unavailable and why, and shows no
   * cards, so nobody acts on made-up pending work.
   */
  test("shows the API error and no cards when unavailable", () => {
    const html = renderApp({ kind: "unavailable", error: "HTTP 503" });

    expect(html).toContain("API unavailable");
    expect(html).toContain("HTTP 503");
    expect(html).not.toContain('class="card');
  });

  /** The page shows every board's groups, each labelled with its board. */
  test("shows every board's groups, each labelled with its board", () => {
    const html = ready();

    expect(html).toMatch(
      /plane<\/h2>\s*<span class="group-board">Work<\/span>/,
    );
    expect(html).toMatch(
      /github<\/h2>\s*<span class="group-board">Work<\/span>/,
    );
    expect(html).toMatch(
      /todoist<\/h2>\s*<span class="group-board">Personal<\/span>/,
    );
  });

  /**
   * The board filter lives in a settings dialog opened by a gear, the third
   * button next to the clock; the page itself shows no filter. It is
   * optional: "All" by default, or a single board.
   */
  test("the board filter lives in the settings dialog", () => {
    expect(controls()).toMatch(
      /data-action="sources"[\s\S]*data-action="refresh"[\s\S]*data-action="settings"/,
    );
    expect(ready()).not.toContain("data-board=");

    const settings = ready(view({ settingsOpen: true }));
    expect(settings).toContain('class="modal settings-modal"');
    expect(settings).toMatch(/class="filter active" data-board="">\s*All/);
    expect(settings).toContain('data-action="close-modal"');

    const html = ready(view({ board: "Personal", settingsOpen: true }));
    expect(html).toMatch(
      /class="filter active" data-board="Personal">\s*Personal/,
    );
    expect(html).toContain("todoist</h2>");
    expect(html).not.toContain("plane</h2>");
  });

  /**
   * Settings also hide a whole board, a group or a single column: they list
   * every board with its groups and columns, each with a "shown" checkbox;
   * children of a hidden item are greyed out.
   */
  test("settings list boards, groups and columns to hide", () => {
    const hidden = new Set([hideKey.group("Work", "github")]);
    const html = ready(view({ settingsOpen: true, hidden }));
    const modal =
      html.match(
        /<section class="modal settings-modal"[\s\S]*<\/section>/,
      )?.[0] ?? "";

    for (const key of [
      hideKey.board("Work"),
      hideKey.board("Personal"),
      hideKey.group("Work", "plane"),
      hideKey.column("Work", "plane", "Mine"),
      hideKey.column("Personal", "todoist", "Today"),
    ]) {
      expect(modal).toMatch(new RegExp(`data-hide="${key}" checked`));
    }
    expect(modal).toMatch(
      new RegExp(`data-hide="${hideKey.group("Work", "github")}"(?! checked)`),
    );
    expect(modal).toMatch(
      new RegExp(
        `data-hide="${hideKey.column("Work", "github", "Review")}" checked disabled`,
      ),
    );
  });

  /** A hidden board, group or column is not drawn, whatever the filter. */
  test("hidden boards, groups and columns are not drawn", () => {
    const board = ready(view({ hidden: new Set([hideKey.board("Personal")]) }));
    expect(board).not.toContain("todoist</h2>");
    expect(board).toContain("plane</h2>");

    const group = ready(
      view({ hidden: new Set([hideKey.group("Work", "plane")]) }),
    );
    expect(group).not.toContain("plane</h2>");
    expect(group).toContain("github</h2>");

    // Hidden empty columns stay hidden when empty columns are shown, and
    // are not counted.
    const column = ready(
      view({
        hidden: new Set([hideKey.column("Work", "github", "My PRs")]),
        expanded: new Set([groupKey("Work", "github")]),
      }),
    );
    expect(column).not.toContain("My PRs");
    expect(column).toContain("Notifications");
    expect(
      ready(
        view({ hidden: new Set([hideKey.column("Work", "github", "My PRs")]) }),
      ),
    ).toMatch(/<\/svg> 1 empty/);

    // A group whose columns are all hidden goes away too.
    const all = ready(
      view({
        hidden: new Set([hideKey.column("Personal", "todoist", "Today")]),
      }),
    );
    expect(all).not.toContain("todoist</h2>");
  });

  /** The gear also shows when something is hidden. */
  test("the gear marks hidden items", () => {
    expect(
      controls(
        snapshot(),
        view({ hidden: new Set([hideKey.board("Personal")]) }),
      ),
    ).toMatch(/aria-label="Settings \(1 hidden\)"[\s\S]*class="filter-dot"/);
  });

  /**
   * Settings pick how often the page refreshes every source on its own:
   * off, or every 1, 5, 15 or 30 minutes; 1 minute by default.
   */
  test("settings choose the auto-refresh interval", () => {
    const html = ready(view({ settingsOpen: true }));
    expect(html).toMatch(
      /class="filter active" data-auto-refresh="1">\s*1 min/,
    );
    for (const minutes of ["0", "5", "15", "30"]) {
      expect(html).toContain(`data-auto-refresh="${minutes}"`);
    }

    const off = ready(view({ settingsOpen: true, autoRefreshMinutes: 0 }));
    expect(off).toMatch(/class="filter active" data-auto-refresh="0">\s*Off/);
  });

  /** While one board is picked, the gear says so, so nothing looks missing. */
  test("the gear marks an active board filter", () => {
    expect(controls()).not.toContain('class="filter-dot"');
    expect(controls(snapshot(), view({ board: "Personal" }))).toMatch(
      /data-action="settings"[^>]*aria-label="Settings \(showing Personal\)"[\s\S]*class="filter-dot"/,
    );
  });

  /**
   * Groups run left to right in config order (the order of `sources`),
   * whatever board they belong to.
   */
  test("groups follow the config order across boards", () => {
    const data = snapshot();
    data.sources = [data.sources[2], data.sources[1], data.sources[0]];

    const order = [...ready(view(), data).matchAll(/([a-z]+)<\/h2>/g)].map(
      (match) => match[1],
    );

    expect(order).toEqual(["todoist", "github", "plane"]);
  });

  /**
   * A group's stacks sit one above another; clicking a stack's header
   * collapses it to the header and its count, and clicking again opens it.
   */
  test("stack headers collapse their stack", () => {
    const key = columnKey("Work", "plane", "Mine");
    const open = ready();
    expect(open).toMatch(
      new RegExp(
        `<button[^>]*class="stack-header"[^>]*data-collapse="${key}"[^>]*aria-expanded="true"`,
      ),
    );
    expect(open).toContain("alert(1)");

    const collapsed = ready(view({ collapsed: new Set([key]) }));
    expect(collapsed).toMatch(
      new RegExp(
        `data-collapse="${key}"[^>]*aria-expanded="false"[\\s\\S]*?Mine[\\s\\S]*?class="count">1<`,
      ),
    );
    expect(collapsed).not.toContain("alert(1)");
  });

  /**
   * Empty columns are hidden; a button in the group's corner says how many,
   * and opening it shows them with the source's last update.
   */
  test("empty columns hide behind a per-group toggle", () => {
    const collapsed = ready();
    expect(collapsed).not.toContain("My PRs");
    expect(collapsed).not.toContain("Empty inbox");
    expect(collapsed).toMatch(
      new RegExp(
        `data-toggle-empty="${groupKey("Work", "github")}"[^>]*>\\s*<svg[\\s\\S]*?</svg> 2 empty`,
      ),
    );

    const expanded = ready(
      view({ expanded: new Set([groupKey("Work", "github")]) }),
    );
    expect(expanded).toContain("My PRs");
    expect(expanded).toContain("Notifications");
    expect(expanded).toContain("updated");
    expect(expanded).not.toContain("Empty inbox");
    expect(expanded).toMatch(/hide empty/);
  });

  /** Cards carry no severity label or colour (Info/Warning/Critical). */
  test("cards carry no severity marker", () => {
    const html = ready();

    expect(html).not.toMatch(/>\s*(Info|Warning|Critical)\s*</);
    expect(html).not.toContain("severity");
    expect(html).toContain('class="card"');
  });

  /**
   * Text from sources is escaped, since issue titles may contain HTML; the
   * title links only to http(s) URLs.
   */
  test("escapes source text and links only http urls", () => {
    const html = ready();
    expect(html).toContain("&lt;script&gt;alert(1)&lt;/script&gt;");
    expect(html).not.toContain("<script>");
    expect(html).toContain(
      'href="https://plane.example/acme/projects/p/issues/1"',
    );

    const unsafe = snapshot();
    unsafe.boards[0].groups[1].columns[0].cards[0].url = "javascript:alert(1)";
    expect(ready(view(), unsafe)).not.toContain('href="javascript');
  });

  /**
   * Sources are not spread over the page: a button opens a modal with each
   * source, its board, status, last fetch and failure reason.
   */
  test("sources live behind a button that opens a modal", () => {
    expect(controls()).toContain('data-action="sources"');
    expect(controls()).not.toContain('class="attention"');
    expect(ready()).not.toContain('class="modal sources-modal"');

    const broken = snapshot();
    broken.sources[1] = {
      name: "github",
      status: "failed",
      last_refresh_at: null,
      message: "401 <bad> credentials",
    };
    const open = ready(view({ sourcesOpen: true }), broken);
    expect(controls(broken)).toMatch(
      /data-action="sources"[^>]*>\s*<span class="attention"[^>]*><svg/,
    );
    expect(open).toContain('class="modal sources-modal"');
    expect(open).toMatch(/plane[\s\S]*Work[\s\S]*ready/);
    expect(open).toContain("401 &lt;bad&gt; credentials");
    expect(open).toContain('data-action="close-modal"');
  });

  /** A saved config with an error also lights the button's warning. */
  test("config errors light the warning and show in the modal", () => {
    const broken = snapshot();
    broken.config_error = "invalid config config.toml: <bad>";

    const html = ready(view({ sourcesOpen: true }), broken);

    expect(controls(broken)).toMatch(
      /data-action="sources"[^>]*>\s*<span class="attention"[^>]*><svg/,
    );
    expect(html).toContain("invalid config config.toml: &lt;bad&gt;");
  });

  /**
   * Cards repeat no dates: the source writes the due or event time into the
   * body, and the last fetch is at the top.
   */
  test("cards show no separate due or update line", () => {
    const withDue = snapshot();
    withDue.boards[0].groups[1].columns[0].cards[0].due_at =
      "2026-09-29T15:00:00Z";

    const cards = (
      ready(view(), withDue).match(/<article[\s\S]*?<\/article>/g) ?? []
    ).join("");

    expect(cards).toContain("gh-1");
    expect(cards).not.toContain("due ");
    expect(cards).not.toContain("updated");
  });

  /**
   * When the API goes down after data was shown, the cards stay visible under
   * a warning that they are old, with the reason.
   */
  test("stale state keeps the cards under a warning", () => {
    const html = renderApp(
      {
        kind: "stale",
        fetchedAt: new Date("2026-09-28T10:00:00Z"),
        error: "HTTP 502",
        snapshot: snapshot(),
      },
      view(),
    );

    expect(html).toContain("alert(1)");
    expect(html).toContain('class="stale"');
    expect(html).toContain("HTTP 502");
  });

  /** Long columns show their first cards and a button for the rest. */
  test("long columns show the first cards and a show-more button", () => {
    const data = snapshot();
    const cards = data.boards[1].groups[0].columns[0].cards;
    for (let i = 2; i <= COLUMN_LIMIT + 2; i++) {
      cards.push(card(`todo-${i}`));
    }
    const key = columnKey("Personal", "todoist", "Today");

    const closed = ready(view(), data);
    expect(closed).toContain(`todo-${COLUMN_LIMIT}`);
    expect(closed).not.toContain(`todo-${COLUMN_LIMIT + 1}`);
    expect(closed).toMatch(new RegExp(`data-more-column="${key}">Show 2 more`));

    const open = ready(view({ openColumns: new Set([key]) }), data);
    expect(open).toContain(`todo-${COLUMN_LIMIT + 2}`);
    expect(open).toContain("Show less");
  });

  /**
   * A configured icon shows next to the source's name, with its dark
   * variant for dark themes; a non-http URL falls back to the generic icon.
   */
  test("groups show their configured icon", () => {
    const data = snapshot();
    data.boards[0].groups[1].icon = {
      url: "https://cdn.example/github.svg",
      dark_url: "https://cdn.example/github-light.svg",
    };
    data.boards[0].groups[0].icon = {
      url: "javascript:alert(1)",
      dark_url: null,
    };

    const html = ready(view(), data);

    expect(html).toContain(
      '<source srcset="https://cdn.example/github-light.svg" media="(prefers-color-scheme: dark)">',
    );
    expect(html).toContain('<img src="https://cdn.example/github.svg"');
    expect(html).not.toContain("javascript:");
  });

  /** Before the first response, the page says it is loading. */
  test("loading state", () => {
    expect(renderApp({ kind: "loading" })).toContain("Loading");
  });

  /**
   * Next to the clock: Sources and Refresh, with the last data fetch below
   * them. Before any data there is nothing to refresh or inspect.
   */
  test("controls next to the clock show the buttons and last fetch", () => {
    const html = controls();

    expect(html).toMatch(
      /data-action="sources"[\s\S]*data-action="refresh"[\s\S]*class="updated"/,
    );
    expect(renderControls({ kind: "loading" })).toBe("");
  });

  /** The page has no header: no product title, no pending count. */
  test("the dashboard has no header, title or pending count", () => {
    const html = ready();

    expect(html).not.toContain("TasksPending");
    expect(html).not.toContain("pending<");
    expect(html).not.toContain('data-action="refresh"');
  });
});
