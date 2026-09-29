# Operations

## Local Commands

- API: `make up`
- API + web dashboard: `make serve`
- TUI: `make tui`
- Frontend: `make frontend-dev`
- Verification: `make ci-check`
- Native release bundle: `make release-smoke`

## Repository maintenance

Once the repository is public, run `./scripts/enable-public-branch-protection.sh` from an account with repository administration permission. It requires the `ci`, `bundle`, `macos`, and `scripts` checks on `master`, requires a linear history and resolved review conversations, and blocks force pushes and branch deletion.

## Boards and Stacks

Each source is a group of stacks, one per `[[sources.stacks]]` entry (or the source's defaults; `[[sources.columns]]`, the old name, still works). A card shows in every stack whose filter it matches. The file order is the screen order: sources run left to right in the order they appear, whatever their `board`, and a source's stacks run top to bottom in the order they are listed, so reordering means moving blocks in the file. Each source also belongs to a board (`board`, formerly `lane`), used to filter the web page and as the TUI's tabs. See `config.example.toml` for every kind's stack keys:

| kind | stack keys | default stacks |
|---|---|---|
| `github` | `query` (search syntax), `notifications` (`unread`/`all`) or `alerts` (`owner/repo` list), `severity` | Review requested, My pull requests, Assigned issues |
| `plane` | `assignee` (me/none/others/any), `state_group`, `state`, `project`, `priority` | In progress, To do, Backlog (yours) |
| `google` | `when` (now/today/tomorrow/later), `calendar` (names) | Now, Today, Tomorrow, Next 30 days |
| `ical` | `when` (now/today/tomorrow/later) | Now, Today, Tomorrow, Next 30 days |
| `todoist` | `filter` (Todoist filter query) | Today (today \| overdue), Next 7 days |
| `sample` | none | Review, Next |

Every stack, whatever the kind, also takes `sort`. Cards in a stack are ordered by severity (critical first), then due time (soonest first), then most recent update (`sort = "newest"`, the default). `sort = "oldest"` puts the least recently updated first instead, to surface stale work such as pull requests nobody touched in weeks. Severity and due time still come first, so critical and overdue cards stay on top; within a query stack the severity is usually the same for every card, so staleness decides the order.

Unknown keys or invalid values fail at startup naming the source and the stack.

## Source Cache

After every successful refresh, each source's cards are saved to `$XDG_STATE_HOME/tasks-pending/cache.json` (default `~/.local/state/tasks-pending/cache.json`). A restart shows them immediately, marked as data from the previous run, while sources refresh. Delete the file to start empty; a corrupt file is ignored and rewritten.

## Plane Source

`kind = "plane"` reads the open work items of every project of one workspace and fills each stack with the items matching its filter (by default, yours). Settings come only from the environment:

- `PLANE_BASE_URL`: instance URL (self-hosted, or `https://api.plane.so` for Plane Cloud);
- `PLANE_WORKSPACE_SLUG` (or `PLANE_WORKSPACE`);
- `PLANE_API_KEY` (or `PLANE_TOKEN`): sent as `x-api-key`, never logged or shown; redirects are not followed, so the key never reaches another host;
- `PLANE_WEB_URL` (optional): web app origin for card links; defaults to `PLANE_BASE_URL`, or `https://app.plane.so` when the API is Plane Cloud's.

Missing variables make the source fail with their names. Stack filters: `assignee` (`me` by default, `none`, `others`, `any`), `state_group` (`backlog`, `unstarted`, `started`, `completed`, `cancelled`), `state` (names such as `In Review`, compared without case, accents included), `project` (identifiers) and `priority` (`urgent`, `high`, `medium`, `low`, `none`). Without `state_group` and `state`, only open groups match; naming states selects them in any group (`state = ["Done"]` works). Invalid group or priority values fail at startup. Items whose state cannot be resolved are skipped, with a warning only when some stack could have shown them. Cards show the issue reference (e.g. `API-12`) as the title, then the issue name, project, state and assignees' names. Assignee and state filters run locally, because some Plane deployments ignore them server-side. Urgent priority and a past target date make a card critical; high priority makes it a warning. "Overdue" uses the machine's local date. Requests go out one at a time, each waiting for the previous one, so a self-hosted server never gets a burst; each request has 15 s. Work items are listed without `expand` (it makes Plane's responses several times slower); state and assignee names come from each project's states and the workspace's members, kept in memory and fetched again only when an unknown id shows up. A refresh therefore takes the sum of every project's requests: give the source its own `timeout_seconds` (for example 180) instead of raising the global one. A failing or slow project becomes a warning naming its identifier, and the other projects still show.

## Google Calendar Source

`kind = "google"` reads Google Calendar through the Calendar API with an access token from GNOME Online Accounts (GOA), over the session D-Bus. There is no OAuth client of our own, no Google Cloud project and no token on disk: GOA owns the account and renews its tokens (the source asks for new credentials once when Google rejects a token).

Setup (works outside GNOME, e.g. Hyprland):

1. `sudo pacman -S gnome-online-accounts gnome-online-accounts-gtk` (needs a running secret service such as gnome-keyring);
2. run `gnome-online-accounts-gtk`, add the Google account and keep **Calendar** enabled;
3. `TASKS_PENDING_GOOGLE_ACCOUNT=<e-mail>` picks an account when there are several (default: the first Google account with Calendar enabled).

It reads the calendars visible in Google Calendar (the primary one and the others ticked in its sidebar), with `singleEvents=true` so Google expands recurrences, and shows the same window and time buckets as the iCal source. Cancelled events and events you declined are hidden; a meeting present in several calendars shows once. Cards link to the event and show its calendar. Stacks take `when` (time buckets) and `calendar` (calendar names, case-insensitive). A failing calendar becomes a warning; the others still show. A Google Workspace admin may block GNOME's OAuth client; then the account cannot be added in GOA.

## Calendar Source

`kind = "ical"` reads an iCal feed from `TASKS_PENDING_ICAL_URL`. For Google Calendar, use the calendar's "Secret address in iCal format" (Settings → the calendar → Integrate calendar); no Google Cloud project or OAuth is needed. The URL grants read access to the calendar: keep it out of git and out of the config file. Errors never show it.

It shows events that are not over yet, starting within 30 days, at most 25, soonest first, in the machine's time zone, in time buckets: now (in progress), today, tomorrow, later. By default each bucket is a stack; `when = [...]` on a stack groups buckets. Events in progress or starting within an hour are warnings. Recurring events are expanded (RRULE, including `UNTIL` given as a date or a floating time; RDATE; EXDATE), all-day events cover the whole local day even when clocks change, moved or cancelled occurrences (RECURRENCE-ID) are respected, and cancelled events are hidden. Cards from a Google feed link to that day in Google Calendar. Entries with an unknown time zone or unreadable recurrence become a warning. Known limits: Windows time zone names (Outlook feeds) are not understood; a series whose first occurrence falls on a midnight that does not exist in its zone (clocks jumping forward at 00:00, e.g. America/Santiago) cannot be expanded; series with more than about 500 occurrences a day are cut short.

## Todoist Source

`kind = "todoist"` runs one Todoist filter query per stack (the same syntax as the app, e.g. `today | overdue`, `#Work & @waiting`). The token comes from `TODOIST_API_TOKEN` (Todoist Settings → Integrations → Developer → API token); without it the source fails with that hint. Overdue tasks are critical (all-day tasks from the next day, timed ones once their time passes), priority p1 is a warning; cards show the project, due date and labels, and link to the task.

## TUI

`tasks-pending tui` runs the same aggregator as the API in-process, so it works without the API running. It reads the same config (`--config` or the default locations) and redraws every 250 ms.

A big clock sits at the top, as on the web page: the date, then the local time as `HH:MM:SS` in block digits, centered. The digits grow with the terminal (up to a third of its height) and the clock hides when the terminal is narrower than 62 columns or when showing it would leave the board fewer than 10 rows.

Keys: `1`–`9` or `Tab`/`Shift+Tab` switch boards; `h`/`l` or left/right move between columns (the board scrolls sideways when they don't fit); `j`/`k` or up/down move within a column (it scrolls to keep the selection visible), `PageUp`/`PageDown` by as many cards as fit on screen, and `Home`/`End` (or `g`/`G`) jump to its first and last card; `Enter` opens the selected card's link (`xdg-open`, or `open` on macOS), `Space` or `d` shows the selected card in full, `s` shows every source's full status, `r` refreshes every source now (at most once every 10 s, to respect provider rate limits), `q`/`Esc`/`Ctrl-C` quit. The Sources panel shows one line per source with its status and failure reason (clipped to the width; `s` shows it whole); feedback and failures show in the footer.

Details open in a centered popup of at most 110 × 30 cells: the card's title, source, board, column, due date, last update, link and whole body, or each source's status, last refresh and full message. `j`/`k`, up/down, `PageUp`/`PageDown` and `Home`/`End` scroll it; `Esc` or `q` close it (they don't quit while it is open), as does the key that opened it; `Enter` still opens the card's link.

The TUI captures the mouse: the wheel moves the selection in the column under the pointer (focusing that column first), or scrolls the open popup; a click selects a card, or focuses the column. While it runs, hold `Shift` to select text with the mouse (most terminals).

## Web Dashboard

A big clock sits at the top of the page: the date, then the local time as `HH:MM:SS` in block digits (the "bricks" font of clock-tui), redrawn each second on its own, without touching the dashboard below. Next to it are the **Sources**, **Refresh** and settings (gear) buttons, with the time of the last data fetch below them; there is no other page header.

The page shows the groups side by side in config order, each labelled with its board, wrapping onto a new row when the window is narrow. Inside a group, stacks sit one above another; clicking a stack's name collapses it to its name and count, and clicking again opens it. The gear opens **Settings**: an optional board filter (All, or one board), and a switch for every board, group and stack to show or hide it (hiding a board or group greys out the switches inside it, which keep their own setting). A group whose stacks are all hidden disappears. **Auto-refresh** makes the page ask the API to refresh every source every 1 (the default), 5, 15 or 30 minutes, or never; it pauses while the tab is hidden, and a source already refreshing finishes before it starts again, so Plane still gets one request at a time. The gear shows a dot while a filter or hidden item is in effect. Settings, collapsed stacks and the rest of the page's choices are remembered per browser (`localStorage`). Empty stacks are hidden: a button at the right of the group header tells how many there are and shows them, with the source's last fetch time. Stacks show their first five cards, with a button for the rest. Cards show the title (a link when the source has one) and at most two lines of the details the source writes, which include the due or event time (the full text is in the tooltip). Each group shows its source's `icon`, falling back to a generic one.

Source health is behind the **Sources** button, which opens a dialog with each source's board, status, last fetch and failure reason, plus a config error when the saved config was not reloaded. The button shows a warning icon when any source is degraded or failed, or the config has an error.

The page polls `/api/v1/snapshot` every 15 seconds while the tab is visible and redraws only when something changed. If the API stops answering after a successful load, the last cards stay on screen under a warning. The Refresh button calls `POST /api/v1/refresh`.

`tasks-pending serve` serves the release build's embedded frontend at `/`. `--static-dir <dir>` overrides it for local frontend development; that directory must have an `index.html`. A binary built without frontend assets and run without the override serves only the API.

## HTTP Endpoints

- `GET /healthz`: process health and version.
- `GET /api/v1/snapshot`: dashboard snapshot.
- `POST /api/v1/refresh`: refresh every source now. Requires the header `x-requested-with: tasks-pending` (403 without it, so other sites open in the browser cannot trigger it) and answers 429 with `retry_after_secs` when called again within 10 s.
- `GET /*`: embedded frontend, or the explicit static directory used for development.

Every request must be addressed to `localhost`, `127.0.0.1` or `[::1]` (any port); other `Host` values get 421, so a page that points its own domain at 127.0.0.1 (DNS rebinding) cannot read the dashboard. Requests without a `Host` header (non-browser clients) are served.

## Configuration

The API reads a TOML file (see `config.example.toml`). The path is the first of these that is set (it does not fall through to the next one when the file is missing):

1. `--config <path>`;
2. `$TASKS_PENDING_CONFIG`;
3. `$XDG_CONFIG_HOME/tasks-pending/config.toml`;
4. `~/.config/tasks-pending/config.toml`.

An explicit path (1 or 2) that does not exist is an error. When no file exists at the default location (3 or 4), the API logs a warning and serves the built-in `sample` source. Syntax errors, unknown keys (top level or inside a source), duplicate source names and zero intervals fail at startup, naming the file. Empty variables and relative `XDG_CONFIG_HOME`/`HOME` values are ignored.

Tokens are read from the environment by each source, never from the config file.

Every source also takes `refresh_seconds` and `timeout_seconds` (overriding the global ones), and `icon` / `icon_dark`: image URLs (http or https) shown next to the source's name on the web, `icon_dark` on dark themes. [Dashboard Icons](https://dashboardicons.com) has logos for most tools, served as `https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons/svg/<name>.svg` (`github`, `github-light`, `plane`, `todoist`, `google-calendar`, `ical`). The browser loads them from that host, so it learns your IP address and which icons you use.

Edits are picked up while running: the API and the TUI check the file every 2 seconds, and a manual refresh (`r`, the web Refresh button) also reloads it first. Sources are rebuilt only when the file's text changed, with the cache keeping their cards on screen meanwhile. A file that fails to load keeps the previous config running and shows the error in the TUI's Sources panel and as a banner on the web. Environment variables are read at startup; changing them still needs a restart.

## GitHub Source

`kind = "github"` runs one search per stack (`query`, github.com search syntax; optional `severity`), or reads notifications (`notifications = "unread"`, matching github.com's Unread tab, or `"all"` for read and unread — the REST API cannot tell which ones you marked as done on github.com, so those show too; `inbox` is accepted as the old name of `all`; unread ones are warnings, cards link to the pull request or issue, or to the repository for other kinds), or shows security alerts (`alerts = ["owner/repo", ...]`, see below). A stack sets exactly one of the three. The notifications API needs a classic token with the `repo` or `notifications` scope, as `gh auth token` provides; fine-grained tokens cannot read it. By default: review requests (warning), your open pull requests and your assigned issues. An item found by several searches shows in each of those stacks.

- Token: `GITHUB_TOKEN`, then `GH_TOKEN`, then `gh auth token` (given up after 5 s; the API logs before running it, the TUI starts silently until then), resolved once at startup for all GitHub sources. Without a token the source shows as failed with a setup hint; restart after logging in.
- Each stack is one search; they run three at a time (GitHub discourages concurrent searches, and the search API allows 30 requests per minute per user, so keep the number of GitHub stacks modest). Each search is limited to 10 s, so a hanging search becomes a warning for its stack instead of failing the refresh. Keep `timeout_seconds` (default 60) above 10, or the aggregator timeout fails the whole refresh first.
- One failed search keeps the other stacks and makes the source degraded; all searches failing makes it failed. Rate limiting reports the reset time, and both scheduled and manual refreshes wait for it (up to an hour); a manual refresh only skips the ordinary backoff.
- Each search loads up to 50 items; more than that, or GitHub reporting incomplete results, shows as a warning. Draft pull requests are marked in the card, and issues and pull requests with comments show how many (`3 comments`).
- Pull request cards show their review state (`review required`, `approved`, `changes requested`) and `draft`. After the searches, one GraphQL request (`POST /graphql`, up to 100 pull requests per request) asks for every pull request of the refresh; a pull request in several stacks is asked for once. `changes requested` makes the card at least a warning. If that request fails or GraphQL returns errors, the cards show without review state and the source is degraded with a warning.
- An `alerts` stack reads the open Dependabot, secret scanning and code scanning alerts of each listed repository (first 100 of each kind; a full page shows as a warning), three requests at a time. Each alert is a card titled `owner/repo: <summary>` linking to the alert, with the kind, the package or rule and the severity in the body. Severity: a leaked secret is critical; a critical advisory or code scanning security severity is critical, a high one (or a code scanning rule of level `error` without a security severity) is a warning, the rest is info. A stack `severity` replaces all of that. A kind that is not set up for a repository (404, such as code scanning without any analysis) simply has no alerts. A kind that is disabled or that the token cannot read (403) becomes a warning naming the repository and the kind while the rest still shows, and so does a repository where every kind answers 404 (usually a typo in its name). The stack fails only when nothing could be read. Secret scanning is asked to leave the secret itself out of its reply. The token needs access to those alerts: a classic token with `repo` (or `security_events` for code scanning), or a fine-grained token with the Dependabot alerts, Secret scanning alerts and Code scanning alerts read permissions. Each repository costs three requests per refresh, so keep the list short or give the source a longer `refresh_seconds` and `timeout_seconds`.
- Error messages never include the token or request URLs.

## Observability

The API initializes `tracing_subscriber` (filter via `RUST_LOG`). Every source refresh logs the source name, duration, item and warning counts, or the failure reason. Sources must never put secrets in error messages, because those reach logs and the dashboard.
