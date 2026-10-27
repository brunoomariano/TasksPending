# Architecture

## Goal

TasksPending tracks pending work from refreshable sources, keeps it fresh in a small daemon, and shows it in a native window.

## Model

The shared model starts in `pending-core`:

- `DashboardSnapshot`: the complete view at one refresh point.
- `Board`: an area of work shown as a tab, such as Work, Personal or Contributions.
- `Group`: one configured source's columns inside a board (Plane, GitHub…).
- `Column`: one filter of that source, shown as a stack (Assigned to me, Review requested…).
- `PendingCard`: an actionable pending item.
- `SourceStatus`: source health and refresh metadata.
- `WeatherAnswer`: the weather beside the clock, as the daemon reports it.
- `release`: where releases are published and how versions compare, for the update check and `tasks-pending upgrade`.

## Sources

Every source implements the `PendingSource` port: `columns()` names its columns (from `[[sources.stacks]]`, or the source's defaults), and `refresh()` returns a `SourceBatch` (cards with their column, plus warnings for partial data) or a `SourceError`. The board comes from the source's configuration (`board`, formerly `lane`, default `Inbox`). Column filter keys are kind-specific: each source crate defines its column type with `deny_unknown_fields`, and `pending_runtime::config` parses `[[sources.stacks]]` into it, naming the source and column on errors.

`build_snapshot` turns one `SourceReport` per source into the dashboard. A bad source never takes the dashboard down:

- a failed source shows as `failed` with its error and contributes no cards;
- warnings make the source `degraded`;
- a card may appear in several columns (they are independent filters); within one column, a repeated id or a non-http(s) url drops the card and the source shows as `degraded` naming it;
- boards and groups keep configuration order; declared columns show even when empty; cards sort by severity, then due time (soonest first), then most recent update, or least recent for a stack with `sort = "oldest"` (read by `pending-core`'s `StackConfig` for every kind and carried to `build_snapshot` through `SourceSpec`/`SourceReport::sorts`).

## Runtime

`pending-runtime` owns scheduling. `Aggregator::start` runs one refresh loop per source, each on its own interval (measured from the end of the previous refresh), with a timeout that turns a hung refresh into a failure. The latest `SourceOutcome` of each source lives in memory, so reading a snapshot never waits for a source:

- before the first result, the source is `pending` (`refreshing` on the dashboard);
- a failure after a success keeps the last batch as `stale`;
- the loops stop when the last `Aggregator` handle is dropped.

A source that fails waits longer before its next attempt: the interval doubled per consecutive failure, up to 16×, and never before the `retry_at` a source reports (the GitHub rate-limit reset), capped at an hour. `refresh_now` skips the ordinary backoff but still waits for a `retry_at`. The first success returns to the normal interval.

`pending_runtime::live::Live` wraps the aggregator for the daemon: it rebuilds it when the config file's text changes (polled every 2 s, and on `refresh_now`), keeps the previous one when the new file fails to load, and reports that in `DashboardSnapshot::config_error`. The API talks to it through the small `Dashboard` trait, which the sandbox implements over fixed cards.

`Aggregator::refresh_now` wakes every source waiting for its interval (the Refresh button).

The last good batch of every source is written to a JSON cache (`$XDG_STATE_HOME/tasks-pending/cache.json`, else `~/.local/state/...`) after each successful refresh. Saves are serialized in-process and merge into the file on disk, so a daemon and a window running the sources itself with different sources keep each other's entries; each write goes to a private (0600) temporary file that is renamed into place. Entries not refreshed for 30 days are dropped. On start, a cached source shows those cards as `cached` (dashboard status `refreshing`, "showing data from the previous run") until its first refresh; a failure then keeps them as `stale`. A missing, corrupt or older-format cache is ignored and rewritten.

`pending_runtime::config` locates and loads the config file, validates it with `AppConfig::validate`, and turns each enabled source into a `SourceSpec`. The source's configured `name` is what source health shows.

## Daemon, Client and Window

`pending-api` is the daemon's HTTP surface (axum): the snapshot, refresh, marks, snoozes and looks over the `Dashboard` trait, the weather Omarchy reports, and a once-a-day look at the latest release. It only binds loopback addresses and only answers local `Host` names. `start_live` builds the live dashboard and its routes; `serve` binds them on the daemon's port, and the window binds them on an ephemeral port when it has to run the sources itself, so there is one code path for talking to a dashboard.

`pending-client` is that path: one method per route, over loopback only, sending the header the state-changing routes require.

`pending-app` is the GTK4 window. `present::page` turns a snapshot and the user's `View` (board filter, collapsed stacks, hidden items) into what to draw, with no widgets involved, and is tested that way; `ui` rebuilds the widgets whenever that page changes and keeps the hero (clock, weather, controls) updated in place. A worker on the Tokio runtime runs the window's commands against the client one at a time and sends events back to the GTK thread. The stylesheet is generated from the app's own palette (dark or light, following the system) because GTK CSS has no variables on every version the app supports.

## Sources Available

- `sample` (`pending-core`): fixed cards.
- `google` (`pending-google`): Google Calendar API with tokens from GNOME Online Accounts over D-Bus (`zbus`); visible calendars, recurrences expanded by Google; cards and time buckets shared with `pending-ical` (`occurrence_items`).
- `ical` (`pending-ical`): upcoming events from an iCal feed (Google Calendar's secret address); recurrences expanded with `rrule`. Cards set `due_at` to the event start.
- `plane` (`pending-plane`): the workspace's work items, read per project and filtered locally per column; request shapes follow PlaneCockpit.
- `jira` (`pending-jira`): one JQL query per column, three at a time; Jira Cloud's enhanced search with e-mail and API token, or Data Center's v2 search with a personal access token. See `docs/operations.md`.
- `linear` (`pending-linear`): one GraphQL `issues` query per column, filtered by Linear (assignee, state type, state name, team, priority), three at a time; the rate-limit reset becomes the source's `retry_at`.
- `todoist` (`pending-todoist`): one Todoist filter query per column (API v1).
- `gitlab` (`pending-gitlab`): one REST list request per column (merge requests or issues by role, or pending to-do items; defaults: review requests, assigned merge requests, assigned issues, to-dos), three at a time, paged. See `docs/operations.md`.
- `github` (`pending-github`): one search API query per column (defaults: review requests, your pull requests split by next action (returned, ready to merge, not approved yet, drafts), assigned issues), three at a time; token precedence and error handling follow `ghpending`. See `docs/operations.md`.

## Reference Repositories

- `ghpending` suggests GitHub token precedence, repo normalization, readable terminal output, and robust provider error handling.
- `clock-tui` suggests the block clock, independent refresh widgets, config-first behavior, and release packaging.
- `ai-memory` suggests the install flow: checksummed release bundles under a versioned directory, a linked executable, `setup` after installing and `upgrade` that swaps the bundle.
