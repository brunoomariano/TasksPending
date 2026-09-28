# Architecture

## Goal

TasksPending tracks pending work from refreshable sources and renders it consistently in terminal and web surfaces.

## Model

The shared model starts in `pending-core`:

- `DashboardSnapshot`: the complete view at one refresh point.
- `Lane`: a high-level track, such as Work, Personal, or Open Source.
- `Section`: a group inside a lane, such as GitHub, Review, or Alerts.
- `PendingCard`: an actionable pending item.
- `SourceStatus`: source health and refresh metadata.

## Sources

Every source implements the `PendingSource` port: `refresh()` returns a `SourceBatch` (cards with their section, plus warnings for partial data) or a `SourceError`. The lane comes from the source's configuration (`lane`, default `Inbox`), never from the source itself.

`build_snapshot` turns one `SourceReport` per source into the dashboard. A bad source never takes the dashboard down:

- a failed source shows as `failed` with its error and contributes no cards;
- warnings make the source `degraded`;
- cards with a non-http(s) url, or an id already used by an earlier source, are dropped, and their source shows as `degraded` naming them;
- lanes and sections keep first-seen order; cards sort by severity, then most recent update.

The frontend types in `frontend/src/contract.gen.ts` are generated from these Rust types with `make contract`.

## Runtime

`pending-runtime` owns scheduling. `Aggregator::start` runs one refresh loop per source, each on its own interval (measured from the end of the previous refresh), with a timeout that turns a hung refresh into a failure. The latest `SourceOutcome` of each source lives in memory, so reading a snapshot never waits for a source:

- before the first result, the source is `pending` (`refreshing` on the dashboard);
- a failure after a success keeps the last batch as `stale`;
- the loops stop when the last `Aggregator` handle is dropped.

A source that fails waits longer before its next attempt: the interval doubled per consecutive failure, up to 16×, and never before the `retry_at` a source reports (the GitHub rate-limit reset). The first success returns to the normal interval.

`Aggregator::refresh_now` wakes every source waiting for its interval (the TUI's `r` key).

The last good batch of every source is written to a JSON cache (`$XDG_STATE_HOME/tasks-pending/cache.json`, else `~/.local/state/...`) after each successful refresh, atomically (temp file + rename), since the API and the TUI may run at the same time. On start, a cached source shows those cards as `cached` (dashboard status `refreshing`, "showing data from the previous run") until its first refresh; a failure then keeps them as `stale`. A missing, corrupt or older-format cache is ignored and rewritten.

`pending_runtime::config` locates and loads the config file, validates it with `AppConfig::validate`, and turns each enabled source into a `SourceSpec`. The source's configured `name` is what source health shows.

## Sources Available

- `sample` (`pending-core`): fixed cards.
- `plane` (`pending-plane`): open work items assigned to the API key's owner, per project, filtered locally; request shapes follow PlaneCockpit.
- `github` (`pending-github`): search API for review requests, authored pull requests and assigned issues; token precedence and error handling follow `ghpending`. See `docs/operations.md`.

## Reference Repositories

- `ghpending` suggests GitHub token precedence, repo normalization, readable terminal output, and robust provider error handling.
- `clock-tui` suggests Ratatui terminal lifecycle handling, independent refresh widgets, config-first behavior, and release packaging.
