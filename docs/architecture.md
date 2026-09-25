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

## First-Cut Runtime

The current runtime returns a sample snapshot. The next iteration should introduce:

- a `PendingSource` trait or equivalent async port;
- source refresh scheduling;
- cache/persistence boundaries;
- GitHub as the first concrete source, borrowing ideas from `ghpending`.

## Reference Repositories

- `ghpending` suggests GitHub token precedence, repo normalization, readable terminal output, and robust provider error handling.
- `clock-tui` suggests Ratatui terminal lifecycle handling, independent refresh widgets, config-first behavior, and release packaging.
