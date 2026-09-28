import { describe, expect, test } from "vitest";
import type { DashboardSnapshot, PendingCard } from "./contract.gen";
import { groupKey, renderApp, type View } from "./render";

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
          columns: [{ name: "Today", cards: [card("todo-1")] }],
        },
      ],
    },
  ],
});

const view = (extra: Partial<View> = {}): View => ({
  board: null,
  expanded: new Set(),
  sourcesOpen: false,
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

  /** The board filter is optional: "All" by default, or a single board. */
  test("the board filter narrows the page to one board", () => {
    expect(ready()).toMatch(/class="filter active" data-board="">\s*All/);

    const html = ready(view({ board: "Personal" }));

    expect(html).toMatch(
      /class="filter active" data-board="Personal">\s*Personal/,
    );
    expect(html).toContain("todoist</h2>");
    expect(html).not.toContain("plane</h2>");
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
    const closed = ready();
    expect(closed).toContain('data-action="sources"');
    expect(closed).not.toContain('class="sources-modal"');
    expect(closed).not.toContain('class="attention"');

    const broken = snapshot();
    broken.sources[1] = {
      name: "github",
      status: "failed",
      last_refresh_at: null,
      message: "401 <bad> credentials",
    };
    const open = ready(view({ sourcesOpen: true }), broken);
    expect(open).toMatch(
      /data-action="sources"[^>]*>\s*<span class="attention"[^>]*><svg/,
    );
    expect(open).toContain('class="sources-modal"');
    expect(open).toMatch(/plane[\s\S]*Work[\s\S]*ready/);
    expect(open).toContain("401 &lt;bad&gt; credentials");
    expect(open).toContain('data-action="close-sources"');
  });

  /** A saved config with an error also lights the button's warning. */
  test("config errors light the warning and show in the modal", () => {
    const broken = snapshot();
    broken.config_error = "invalid config config.toml: <bad>";

    const html = ready(view({ sourcesOpen: true }), broken);

    expect(html).toMatch(
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

    const html = ready(view(), withDue);

    expect(html).not.toContain("due ");
    expect(html).not.toMatch(/class="card[\s\S]*updated/);
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

  /** Before the first response, the page says it is loading. */
  test("loading state", () => {
    expect(renderApp({ kind: "loading" })).toContain("Loading");
  });

  /** With data on screen, a button refreshes every source now. */
  test("offers a refresh button", () => {
    expect(ready()).toContain('data-action="refresh"');
  });
});
