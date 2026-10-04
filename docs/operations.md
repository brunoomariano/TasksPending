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
| `github` | `query` (search syntax), `notifications` (`unread`/`all`) or `alerts` (`owner/repo` list), `severity` | Review requested, Returned to you, Ready to merge, Not approved yet, Drafts, Assigned issues |
| `gitlab` | `merge_requests` (review_requested/assigned/authored), `issues` (assigned/authored) or `todos = true`; `group` or `project`, `labels`, `draft`, `severity` | Review requested, Assigned merge requests, Assigned issues, To-dos |
| `plane` | `assignee` (me/none/others/any), `state_group`, `state`, `project`, `priority` | In progress, To do, Backlog (yours) |
| `jira` | `jql` (JQL query), `severity` | In progress, To do (yours) |
| `linear` | `assignee` (me/none/others/any), `state_type`, `state`, `team`, `priority` | In progress, To do, Backlog (yours) |
| `google` | `when` (now/today/tomorrow/later), `calendar` (names) | Now, Today, Tomorrow, Next 30 days |
| `ical` | `when` (now/today/tomorrow/later) | Now, Today, Tomorrow, Next 30 days |
| `todoist` | `filter` (Todoist filter query) | Today (today \| overdue), Next 7 days |
| `sample` | none | Review, Next |

Every stack, whatever the kind, also takes `enabled`, `sort` and `exclude`. `enabled = false` switches a stack off without deleting it: it is not queried and not shown in the web page or the TUI (its filter is still checked for typos), and a source whose stacks are all off is not scheduled. This is the durable way to hide a stack everywhere; the switches in the web page's Settings only affect that browser. Cards in a stack are ordered by severity (critical first), then due time (soonest first), then most recent update (`sort = "newest"`, the default). `sort = "oldest"` puts the least recently updated first instead, to surface stale work such as pull requests nobody touched in weeks. Severity and due time still come first, so critical and overdue cards stay on top; within a query stack the severity is usually the same for every card, so staleness decides the order.

Every stack also takes `exclude`, a list of patterns (empty by default): a card whose title or body matches any of them is left out of that stack. The source's other stacks are not affected, so a card found by two stacks stays in the one that does not exclude it. Excluded cards are simply absent: they are not counted and the source does not become degraded. Typical uses:

```toml
[[sources.stacks]]
name = "Review requested"
query = "is:open is:pr archived:false review-requested:@me"
exclude = [
  "dependabot\\[bot\\]",   # bot noise, by author
  "^chore\\(deps\\)",      # ... or by title prefix
  "^acme/legacy#",         # a whole repository (a Plane project: "^OPS-")
  "\\bwip\\b",             # a word, such as a label written in the title
]
```

Matching rules:

- Patterns are regular expressions in the [`regex` crate syntax](https://docs.rs/regex/latest/regex/#syntax), compared without case. In a TOML basic string a backslash is written twice (`"\\["`); a literal string in single quotes takes it once (`'\['`).
- A pattern matches anywhere in the text unless anchored. The title and the body are tested separately, each as one text: `^` and `$` mean the start and end of the title or of the whole body, not of each body line (start a pattern with `(?m)` for that).
- The text is what the card shows: the title, and the body as written in the card (for GitHub searches, `owner/repo#12 · @author` plus draft, review and comment notes). Fields a card does not show, such as labels, cannot be matched.
- An invalid or empty pattern, or one that matches the empty text (`"bot|"`, `".*"`, `"^$"`: it would hide cards whatever they say), fails when the config loads, naming the file, the source, the stack and the pattern, also in a stack with `enabled = false`. The `sample` source has fixed stacks and takes none of these keys.
- Patterns apply when cards arrive (and to cached cards at startup), so a changed list takes effect when the config is reloaded and the source refreshes. The cache holds the cards already filtered.

A marked card (see "Now" below) that `exclude` hides from every stack is off the dashboard but keeps its mark, since the item is still open at its source; the mark shows again when the pattern goes away, and is dropped as usual once the source no longer lists the item.

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

## Jira Source

`kind = "jira"` runs one JQL query per stack (`jql`, the same syntax as Jira's issue search; optional `severity`). This source was built from Jira's API documentation and is covered by tests against a stand-in server; it has not yet been run against a live Jira account, so please report anything that behaves differently. Settings come only from the environment:

- `JIRA_BASE_URL`: the site address, e.g. `https://yourcompany.atlassian.net` (a Data Center context path such as `https://jira.example.com/jira` is kept);
- Jira Cloud: `JIRA_EMAIL` and `JIRA_API_TOKEN` (an API token from the Atlassian account's security settings), sent as HTTP Basic auth;
- Jira Data Center: `JIRA_TOKEN` alone (a personal access token), sent as `Authorization: Bearer`.

The credentials also pick the API: with `JIRA_EMAIL` and `JIRA_API_TOKEN` the source asks Jira Cloud's enhanced search (`GET /rest/api/3/search/jql`, paged with `nextPageToken`); with only `JIRA_TOKEN` it asks Data Center's `GET /rest/api/2/search` (paged with `startAt`). When all three are set, the Cloud pair wins. `JIRA_EMAIL` with `JIRA_TOKEN` but no `JIRA_API_TOKEN` mixes the two and is refused, saying which variable to set or remove. Missing variables make the source fail with their names. Credentials are never logged or shown, and redirects are not followed, so they never reach another host.

By default: **In progress** (`assignee = currentUser() AND statusCategory = "In Progress" ORDER BY updated DESC`) and **To do** (`assignee = currentUser() AND statusCategory = "To Do" ORDER BY priority DESC, updated DESC`). Other useful queries:

- in review: `assignee = currentUser() AND status = "In Review"`;
- reported by me and still open: `reporter = currentUser() AND statusCategory != Done`;
- watching: `watcher = currentUser() AND statusCategory != Done`;
- the current sprint: `assignee = currentUser() AND sprint in openSprints()`.

Jira Cloud refuses a query without any restriction (only `ORDER BY ...`); filter by assignee, project or the like. An issue found by several queries shows in each of those stacks.

- Cards show the issue key (e.g. `PROJ-123`) as the title, then the summary, project, status, assignee and due date, and link to `<JIRA_BASE_URL>/browse/<key>`. A due date in the past is critical; so are the priorities Highest, Blocker and Critical; High and Major are warnings; the rest is info. A stack `severity` replaces all of that.
- Each query asks only for the fields the cards use, 100 issues per page, up to 5 pages; more than that shows as a warning naming the stack.
- Queries run three at a time, each request limited to 15 s. A stack also has 25 s for all its pages together; past that it becomes a warning (`<stack>: timed out after 25s`) and shows no cards. All the stacks of a refresh share 50 s, kept below the default `timeout_seconds` of 60: with many slow stacks, the ones there is no time left for become warnings (`<stack>: not finished: the refresh used up its 50s for all stacks`) and the stacks that answered still show. A failing query (invalid JQL shows Jira's own explanation) becomes a warning naming its stack while the others still show; all queries failing makes the source failed. A rejected login (401, or Data Center answering as an anonymous user) says which variables to check. On a rate limit (429) that stops every stack, the source waits for the time Jira gives in `Retry-After` before asking again; when only some stacks are limited, they become warnings naming that time and the source is asked again at its usual interval.
## Linear Source

`kind = "linear"` runs one GraphQL query per stack against Linear's API and fills the stack with the issues Linear returns for its filter (by default, yours). This source was built from Linear's API documentation and is covered by tests against a stand-in server; it has not yet been run against a live Linear account, so please report anything that behaves differently. Settings come only from the environment:

- `LINEAR_API_KEY`: a personal API key (Linear → Settings → Security & access → Personal API keys; read access is enough). It is sent as the `Authorization` header, never logged or shown; redirects are not followed, so the key never reaches another host;
- `LINEAR_API_URL` (optional): the GraphQL endpoint, `https://api.linear.app/graphql` by default.

Without the key the source fails naming the variable. Stack filters: `assignee` (`me` by default, `none`, `others`, `any`), `state_type` (`triage`, `backlog`, `unstarted`, `started`, `completed`, `canceled`), `state` (names such as `In Review`, compared without case), `team` (team keys such as `ENG`, the prefix of the team's issue references, written as Linear shows them) and `priority` (`urgent`, `high`, `medium`, `low`, `none`). Without `state_type` and `state`, only open types match (triage, backlog, unstarted, started); naming states selects them in any type (`state = ["Done"]` works), and giving both requires both. Invalid values fail at startup. Archived issues never show.

Cards show the issue reference (e.g. `ENG-123`) as the title, then the issue title, team, state, the assignee's display name (in stacks that are not only yours) and the due date, and link to the issue. Urgent priority and a past due date make a card critical; high priority makes it a warning. "Overdue" uses the machine's local date, and never applies to an issue in a completed or canceled state (a stack can list those with `state_type`). The source shows Linear's logo unless `icon` replaces it.

The filters run on Linear's side, so each stack costs one request per 100 issues, most recently updated first. A stack reads at most 500 issues; beyond that it shows a warning. Stacks are queried three at a time, each request with 15 s. A stack also has 25 s for all its pages together; past that it becomes a warning (`<stack>: timed out after 25s`) and shows no cards. All the stacks of a refresh share 50 s, kept below the default `timeout_seconds` of 60: with many slow stacks, the ones there is no time left for become warnings (`<stack>: not finished: the refresh used up its 50s for all stacks`) and the stacks that answered still show. A failing or slow stack becomes a warning naming it and the other stacks still show; when every stack fails, the source fails. A rejected key says to check `LINEAR_API_KEY`. When Linear's rate limit is hit (an API key has 2,500 requests per hour), the source reports the reset time; when every stack was stopped by it, both scheduled and manual refreshes wait for it (up to an hour), and when only some were, they become warnings and the source is asked again at its usual interval. Error messages never include the key or the request URL.

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

The board is laid out as on the web page: the selected board's groups (sources) sit side by side in config order, each in a box at least 30 cells wide, and the board scrolls sideways when they don't all fit. Inside a group, its stacks sit one above another: a header line with the stack's name and card count, then its cards, two rows each (title, then the details on one line, with line breaks shown as ` · `). An empty stack is only its dimmed header with `(0)`; a collapsed stack is only its header, marked `▸` instead of `▾`. A group taller than the screen scrolls to keep the selection visible, with a scrollbar on its right edge.

`m` marks the selected card as in progress, or unmarks it (also from its details); a failure shows in the footer. The marks are the ones the web page uses (the same `marks.json`), so each shows the other's. A marked card has `●` before its title in its stacks, and the marked cards that are on the dashboard are repeated in a **Now** panel between the Sources panel and the groups: one line each (title, then source), in the order they were marked, whatever the selected board. The panel shows at most five rows (with more marked cards, four of them and `+N more`), is absent when nothing is marked, and hides when it would leave the board fewer than 10 rows; the clock hides before it does.

`z` snoozes the selected card: a small menu offers **1 hour**, **Tomorrow** (the next day at 8:00, local time), **Next week** (the next Monday at 8:00; on a Monday, the following one) and **Until it changes**, each with the time it ends. `1`–`4` choose right away, or `j`/`k` and `Enter`; `Esc`, `q` or `z` close the menu without snoozing. The card leaves the board and the footer says until when; a failure (including a snooze that could not be saved) shows there too. The menu belongs to the card it was opened on: if a refresh takes that card away, the menu closes instead of acting on its neighbour, and so does the details popup. The snoozes are the ones the web page uses (the same `snoozes.json`), with the same rules: a snoozed card comes back early when its item changes. While cards are snoozed, the header says `· N snoozed` after the version. `Z` lists them, one line each (title, source, and `until <local time>` or `until it changes`): `j`/`k` select one, `w` or `Enter` wakes it, and `Esc`, `q` or `Z` close the list.

A small dim `•` before a card's title means the item changed since you last looked, and the same dot after a stack's count means the stack holds such a card, also when it is collapsed. There is no count and no label; a marked card shows `●`, then the dot. The rules and the file are the web page's (`looks.json`), so the TUI and the browsers agree. In the TUI, "looking" is starting it, any key press, a click or the wheel (the pointer merely moving over the terminal does not count), reported to the dashboard at most once a minute.

Keys: `1`–`9` or `Tab`/`Shift+Tab` switch boards; `h`/`l` or left/right move between groups, starting at the group's first card; `j`/`k` or up/down move through the cards of the group, from the last card of a stack to the first card of the next one and back, `PageUp`/`PageDown` by as many cards as fit on screen, and `Home`/`End` (or `g`/`G`) jump to the group's first and last card; `c` collapses the selected card's stack, leaving the selection on its header (which `j`/`k` still stop at, skipping its cards), and `c` there expands it again; collapsed stacks stay collapsed until the TUI quits; `Enter` opens the selected card's link (`xdg-open`, or `open` on macOS), `Space` or `d` shows the selected card in full, `m` marks or unmarks it as in progress, `z` snoozes it, `Z` lists the snoozed cards, `s` shows every source's full status, `r` refreshes every source now (at most once every 10 s, to respect provider rate limits), `q`/`Esc`/`Ctrl-C` quit. The Sources panel shows one line per source with its status and failure reason (clipped to the width; `s` shows it whole); feedback and failures show in the footer, which otherwise reminds the everyday keys.

Details open in a centered popup of at most 110 × 30 cells: the card's title, source, board, stack, whether it is marked as in progress, due date, last update, link and whole body, or each source's status, last refresh and full message. `j`/`k`, up/down, `PageUp`/`PageDown` and `Home`/`End` scroll it; `Esc` or `q` close it (they don't quit while it is open), as does the key that opened it; `Enter` still opens the card's link.

The TUI captures the mouse: the wheel moves the selection through the group under the pointer (focusing that group first), or scrolls the open popup (in the snooze menu and the snoozed list, it moves their selection); a click selects a card, collapses or expands a stack when it lands on its header, or else focuses the group. While it runs, hold `Shift` to select text with the mouse (most terminals).

## Web Dashboard

A big clock sits at the top of the page: the date, then the local time as `HH:MM:SS` in block digits (the "bricks" font of clock-tui), redrawn each second on its own, without touching the dashboard below. Next to it are the **Sources**, **Refresh** and settings (gear) buttons, with the time of the last data fetch and the running version (e.g. `v0.4.0`) below them; there is no other page header.

On Omarchy, a small weather widget sits on the other side of the clock: an icon for the current condition, the temperature, and below them the place and the wind, as `omarchy-weather-status` reports them (set the place with `omarchy-weather-location --set <name>`). It is optional context. It is not on the page at all when weather is unavailable (another system, no network, the commands failing), and it is hidden when the window is narrower than 1440 px, so it never takes room from the clock. The page asks `GET /api/v1/weather` on load, every 10 minutes while the tab is visible, and when the tab becomes visible again, independently of the dashboard polling. The sandbox shows a fixed sample.

The page shows the groups side by side in config order, each labelled with its board, wrapping onto a new row when the window is narrow. Inside a group, stacks sit one above another; clicking a stack's name collapses it to its name and count, and clicking again opens it. The gear opens **Settings**: an optional board filter (All, or one board), and a switch for every board, group and stack to show or hide it (hiding a board or group greys out the switches inside it, which keep their own setting). A group whose stacks are all hidden disappears. **Auto-refresh** makes the page ask the API to refresh every source every 1 (the default), 5, 15 or 30 minutes, or never; it pauses while the tab is hidden, and a source already refreshing finishes before it starts again, so Plane still gets one request at a time. The gear shows a dot while a filter or hidden item is in effect. Settings, collapsed stacks and the rest of the page's choices are remembered per browser (`localStorage`). Empty stacks are hidden: a button at the right of the group header tells how many there are and shows them, with the source's last fetch time. Stacks show their first five cards, with a button for the rest. Cards show the title (a link when the source has one) and at most two lines of the details the source writes, which include the due or event time (the full text is in the tooltip). Each group shows its source's `icon`, falling back to a generic one.

Source health is behind the **Sources** button, which opens a dialog with each source's board, status, last fetch and failure reason, plus a config error when the saved config was not reloaded. The button shows a warning icon when any source is degraded or failed, or the config has an error.

Every card has a pin button (shown on hover) that marks it as in progress; click it again to unmark. Marked cards stay in their stacks, highlighted, and are repeated in a **Now** row above the stacks, in the order they were marked, with no limit; the row is absent when nothing is marked, and it ignores the board filter and hidden items. Marks are kept by the daemon in `$XDG_STATE_HOME/tasks-pending/marks.json` (next to the cache), so every browser shows the same ones and they survive restarts. The TUI shares the same file (`m` marks the selected card). A mark is dropped on its own when its card is gone after a clean refresh of its source that finished after the card was marked (the item was done elsewhere); while the source is failing, refreshing or showing old data, the mark is kept, and so is the mark of a card that a stack's `exclude` patterns hide. Marks of a source that is not on the dashboard (switched off, or absent from the config in use) are kept for a month, so they are back when the source is. Deleting `marks.json` clears every mark.

Next to the pin, the moon button snoozes a card: it leaves its stacks for **1 hour**, until **tomorrow** (8:00), until **next week** (Monday 8:00) or **until it changes**. Whatever the choice, a snoozed card comes back early when its item changes (its last update moves), so snoozing never hides news. Nothing is written to the provider. While cards are snoozed, a quiet "N snoozed" link after the update time opens the list, where **Wake** brings a card back now. Snoozes are kept by the daemon in `$XDG_STATE_HOME/tasks-pending/snoozes.json`, shared by every browser, and forgotten when the card is finished elsewhere, like marks. A snoozed card keeps its mark and returns to the Now row when it wakes.

A small dot before a card's title means the item changed since you last looked, and the same dot after a stack's count means the stack holds such a card (also when it is collapsed or the card is behind "Show more"). There is no count and no label. "Looking" is doing something on the page: a click, a key, scrolling, a touch. The pointer merely crossing the window, or the window taking focus under it, does not count, so the dots are not cleared by accident. When you come back after 10 minutes or more without activity, the dots show what changed since your previous activity; cards that change while you keep using the page get a dot too, and all of them clear the next time you return. A change counts from the moment the dashboard fetched it: an item that changed just before you left, but that its source only refreshed after, is flagged on your return, because it was never on screen. The daemon keeps these times in `$XDG_STATE_HOME/tasks-pending/looks.json`, so every browser agrees.

The page polls `/api/v1/snapshot` every 15 seconds while the tab is visible and redraws only when something changed. If the API stops answering after a successful load, the last cards stay on screen under a warning. The Refresh button calls `POST /api/v1/refresh`.

`tasks-pending serve` serves the release build's embedded frontend at `/`. `--static-dir <dir>` overrides it for local frontend development; that directory must have an `index.html`. A binary built without frontend assets and run without the override serves only the API.

## HTTP Endpoints

- `GET /healthz`: process health and version.
- `GET /api/v1/snapshot`: dashboard snapshot.
- `POST /api/v1/refresh`: refresh every source now. Requires the header `x-requested-with: tasks-pending` (403 without it, so other sites open in the browser cannot trigger it) and answers 429 with `retry_after_secs` when called again within 10 s.
- `POST /api/v1/marks`: mark or unmark a card as in progress, with a JSON body `{"id": "<card id>", "marked": true}`. Requires the same header (403 without it); 204 on success, 404 when no card with that id is on the dashboard, 500 when the mark could not be written (the log says why). The snapshot lists the marked ids in `marked`.
- `POST /api/v1/snooze`: snooze or wake a card, with a JSON body `{"id": "<card id>", "snoozed": true, "until": "<RFC 3339 time>"}`; without `until` (or with `null`) the card waits for its item to change, and `"snoozed": false` wakes it. Requires the same header (403 without it); 204 on success, 404 when the card is not on the dashboard, 400 when `until` is in the past, 500 when the snooze could not be written (the log says why). Snoozing a card that is already snoozed replaces its time. The snapshot leaves snoozed cards out of the boards and lists them in `snoozed`.
- `POST /api/v1/look`: record that the user is looking at the dashboard (the page sends it on activity, at most once a minute). Requires the same header (403 without it); answers 204. The snapshot lists in `changed` the ids of the cards updated since the previous sitting ended (or since the refresh their source was showing then, when that is earlier).
- `GET /api/v1/weather`: the weather Omarchy reports, for the widget beside the clock. Answers `{"available": true, "place": "Recife", "temperature": "29°C", "wind": "←15km/h", "condition": "partly-cloudy-day"}`, or `{"available": false}` when there is none. `condition` is one of `clear-day`, `clear-night`, `partly-cloudy-day`, `partly-cloudy-night`, `cloudy`, `fog`, `rain`, `sleet`, `snow`, `thunder` or `unknown`. The daemon runs `omarchy-weather-status` and `omarchy-weather-icon` from its `PATH` (no shell, no arguments, 6 s each at most) and keeps the answer in memory for 10 minutes (1 minute for a failure), so requests do not start processes. A status line in an unexpected format counts as unavailable.
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

Every source also takes `refresh_seconds` and `timeout_seconds` (overriding the global ones). Every provider (GitHub, GitLab, Jira, Linear, Plane, Google Calendar, iCal, Todoist) shows its logo from Dashboard Icons by default; GitHub and Linear also change to their light marks in dark themes. Set `icon` / `icon_dark` to replace a logo with image URLs (http or https), with `icon_dark` used on dark themes. [Dashboard Icons](https://dashboardicons.com) has logos for most tools, served as `https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons/svg/<name>.svg` (`github`, `github-light`, `plane`, `todoist`, `google-calendar`, `ical`). The browser loads them from that host, so it learns your IP address and which icons you use.

Edits are picked up while running: the API and the TUI check the file every 2 seconds, and a manual refresh (`r`, the web Refresh button) also reloads it first. Sources are rebuilt only when the file's text changed, with the cache keeping their cards on screen meanwhile. A file that fails to load keeps the previous config running and shows the error in the TUI's Sources panel and as a banner on the web. Environment variables are read at startup; changing them still needs a restart.

## GitHub Source

`kind = "github"` runs one search per stack (`query`, github.com search syntax; optional `severity`), or reads notifications (`notifications = "unread"`, matching github.com's Unread tab, or `"all"` for read and unread — the REST API cannot tell which ones you marked as done on github.com, so those show too; `inbox` is accepted as the old name of `all`; unread ones are warnings, cards link to the pull request or issue, or to the repository for other kinds), or shows security alerts (`alerts = ["owner/repo", ...]`, see below). A stack sets exactly one of the three. The notifications API needs a classic token with the `repo` or `notifications` scope, as `gh auth token` provides; fine-grained tokens cannot read it. An item found by several searches shows in each of those stacks.

Without `[[sources.stacks]]`, the stacks say what to do next:

| stack | query | severity |
|---|---|---|
| Review requested | `is:open is:pr archived:false review-requested:@me` | warning |
| Returned to you | `is:open is:pr archived:false author:@me draft:false review:changes_requested` | warning |
| Ready to merge | `is:open is:pr archived:false author:@me draft:false review:approved` | info |
| Not approved yet | `is:open is:pr archived:false author:@me draft:false -review:approved -review:changes_requested` | info |
| Drafts | `is:open is:pr archived:false author:@me draft:true` | info |
| Assigned issues | `is:open is:issue archived:false assignee:@me` | info |

Each of your open pull requests is in exactly one of the four middle stacks. "Not approved yet" is whatever is neither approved nor returned, so a pull request that only got comments stays there (`review:none` would drop it, and `review:required` only matches repositories that require reviews). The name states only what the search knows: in a repository that needs no review, such a pull request may be yours to merge rather than waiting on anyone. "Ready to merge" means approved; it does not look at checks or merge conflicts. "Returned to you" follows GitHub's review state, which can stay at "changes requested" after you push fixes and ask for a new review, until the reviewer answers or the old review is dismissed. To get a single stack of your pull requests back (`is:open is:pr archived:false author:@me`), or to keep to one organization (`org:acme`), declare your own stacks; `config.example.toml` has these as a starting point. The defaults are six searches per refresh.

- Token: `GITHUB_TOKEN`, then `GH_TOKEN`, then `gh auth token` (given up after 5 s; the API logs before running it, the TUI starts silently until then), resolved once at startup for all GitHub sources. Without a token the source shows as failed with a setup hint; restart after logging in.
- Each stack is one search; they run three at a time (GitHub discourages concurrent searches, and the search API allows 30 requests per minute per user, so keep the number of GitHub stacks modest). Each search is limited to 10 s, so a hanging search becomes a warning for its stack instead of failing the refresh. Keep `timeout_seconds` (default 60) above 10, or the aggregator timeout fails the whole refresh first.
- One failed search keeps the other stacks and makes the source degraded; all searches failing makes it failed. Rate limiting reports the reset time, and both scheduled and manual refreshes wait for it (up to an hour); a manual refresh only skips the ordinary backoff.
- Each search loads up to 50 items; more than that, or GitHub reporting incomplete results, shows as a warning. Draft pull requests are marked in the card, and issues and pull requests with comments show how many (`3 comments`).
- Pull request cards show their review state (`review required`, `approved`, `changes requested`), `draft` and how the checks of the last commit are doing: `checks failing` (the rollup is a failure or an error), `checks pending` (still running, or a required check has not reported yet) or `checks passing`; a pull request without checks says nothing about them. After the searches, one GraphQL request (`POST /graphql`, up to 100 pull requests per request) asks for every pull request of the refresh; a pull request in several stacks is asked for once. The checks come in that same request, so they add no requests, only a little GraphQL cost per pull request. `changes requested` and `checks failing` each make the card at least a warning. If that request fails or GraphQL returns errors, the cards show without review state and checks, and the source is degraded with a warning (the same error for many pull requests is reported once). A token that may not read check status is the exception (GitHub answers `FORBIDDEN` for that field: a fine-grained token without read access to checks and commit statuses, or an organization behind SSO): the checks are left out without a warning, and the review state still shows. Any other error on the checks is reported like the rest.
- An `alerts` stack reads the open Dependabot, secret scanning and code scanning alerts of each listed repository (first 100 of each kind; a full page shows as a warning), three requests at a time. Each alert is a card titled `owner/repo: <summary>` linking to the alert, with the kind, the package or rule and the severity in the body. Severity: a leaked secret is critical; a critical advisory or code scanning security severity is critical, a high one (or a code scanning rule of level `error` without a security severity) is a warning, the rest is info. A stack `severity` replaces all of that. A kind that is not set up for a repository (404, such as code scanning without any analysis) simply has no alerts. A kind that is disabled or that the token cannot read (403) becomes a warning naming the repository and the kind while the rest still shows, and so does a repository where every kind answers 404 (usually a typo in its name). The stack fails only when nothing could be read. Secret scanning is asked to leave the secret itself out of its reply. The token needs access to those alerts: a classic token with `repo` (or `security_events` for code scanning), or a fine-grained token with the Dependabot alerts, Secret scanning alerts and Code scanning alerts read permissions. Each repository costs three requests per refresh, so keep the list short or give the source a longer `refresh_seconds` and `timeout_seconds`.
- Error messages never include the token or request URLs.

## GitLab Source

`kind = "gitlab"` shows your open merge requests and issues and your pending to-do items, on gitlab.com or a self-managed instance, through the REST API (v4). This source was built from GitLab's API documentation and is covered by tests against a stand-in server; it has not yet been run against a live GitLab account, so please report anything that behaves differently. A stack sets exactly one of:

- `merge_requests = "review_requested"`, `"assigned"` or `"authored"`: open merge requests where you are a reviewer, an assignee or the author;
- `issues = "assigned"` or `"authored"`: open issues;
- `todos = true`: pending to-do items.

By default: review requests (warning), assigned merge requests, assigned issues and to-dos. An item found by several stacks shows in each of them. The group shows GitLab's logo unless the source sets `icon`.

- Settings come only from the environment: `GITLAB_TOKEN`, a personal access token with the `read_api` scope, and optionally `GITLAB_BASE_URL` for a self-managed instance (default `https://gitlab.com`; give the instance address, without `/api/v4`). Without a token the source shows as failed with a setup hint; restart after setting it.
- The token is sent as the `PRIVATE-TOKEN` header. Redirects are not followed, so it never reaches another host: a `GITLAB_BASE_URL` that redirects (`http` to `https`, a moved instance) fails with a message saying so. Error messages never include the token or request URLs.
- Merge request and issue stacks look across everything the token sees. `group = "acme/platform"` narrows a stack to one group and its subgroups, `project = "acme/api"` to one project (full paths, or numeric ids; at most one of the two; `.` and `..` are not accepted as path parts). `labels = ["bug", "backend"]` keeps items carrying every listed label, and `draft = true` or `false` (merge requests only) keeps only drafts or only non-drafts. Merge requests of archived projects are left out. None of these keys applies to to-dos.
- Cards are titled after the merge request or issue and show its reference (`acme/api!12`, `acme/api#7`), the author, `draft`, and the number of comments when there are any. Issues with a due date show `due 2026-10-10`, or `overdue since 2026-10-01` once that day has passed on the machine's local date; overdue issues are critical. Review requests are warnings and everything else is info; a stack `severity` replaces all of that. To-do cards are titled after their target and show the action (`assigned`, `mentioned`, `review requested`, …), the project or group and who caused it, and link to the target.
- Each stack is one list request of up to 100 items, followed page by page up to five pages (500 items); more than that shows as a warning. Stacks run three at a time and each request is limited to 10 s, so a hanging stack becomes a warning instead of failing the refresh. A stack also has 25 s for all its pages together; past that it becomes a warning (`<stack>: timed out after 25s`) and shows no cards. All the stacks of a refresh share 50 s, kept below the default `timeout_seconds` of 60: with many slow stacks, the ones there is no time left for become warnings (`<stack>: not finished: the refresh used up its 50s for all stacks`) and the stacks that answered still show. Keep `timeout_seconds` (default 60) above 10.
- One failed stack keeps the others and makes the source degraded; all stacks failing makes it failed. A rejected token (401), a token without the scope (403) and a group or project that is not found (404) say so in the message. Rate limiting (429) reports when to try again, from `Retry-After` or `RateLimit-Reset`; when it stopped every stack, both scheduled and manual refreshes wait for that time (up to an hour), and when only some stacks were limited, they become warnings and the source is asked again at its usual interval.

## Observability

The API initializes `tracing_subscriber` (filter via `RUST_LOG`). Every source refresh logs the source name, duration, item and warning counts, or the failure reason. Sources must never put secrets in error messages, because those reach logs and the dashboard.
