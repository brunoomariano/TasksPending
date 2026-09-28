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

## First-Cut Runtime

The current runtime returns the snapshot of the built-in `SampleSource`. The next iteration should introduce:

- source refresh scheduling;
- cache/persistence boundaries;
- GitHub as the first concrete source, borrowing ideas from `ghpending`.

## Reference Repositories

- `ghpending` suggests GitHub token precedence, repo normalization, readable terminal output, and robust provider error handling.
- `clock-tui` suggests Ratatui terminal lifecycle handling, independent refresh widgets, config-first behavior, and release packaging.
