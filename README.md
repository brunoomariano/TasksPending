# TasksPending

TasksPending is a local-first pending-work dashboard. It will collect auto-refreshing sources and show the same model through a Rust TUI, a Rust HTTP API, and a lightweight TypeScript frontend.

The first source target is inspired by:

- [`akitaonrails/ghpending`](https://github.com/akitaonrails/ghpending): GitHub pending issues, pull requests, alerts, token fallback, and terminal digest.
- [`akitaonrails/clock-tui`](https://github.com/akitaonrails/clock-tui): Ratatui interface, independent refresh widgets, config-first operation, and release packaging.

## Quick Start

```sh
make bootstrap
make ci-check
make tui
```

Run the API:

```sh
make up
curl http://127.0.0.1:8080/healthz
curl http://127.0.0.1:8080/api/v1/snapshot
```

Configure sources by copying `config.example.toml` to `~/.config/tasks-pending/config.toml`; without it the API serves a built-in sample source. See [docs/operations.md](docs/operations.md).

Run the API and the built web dashboard together, then open http://127.0.0.1:8080:

```sh
make serve
```

Or run the frontend dev server (proxies `/api` to `make up`):

```sh
make frontend-dev
```

## Repository Shape

- `crates/pending-core`: shared domain model, config types, and source contracts.
- `crates/pending-runtime`: refresh scheduling and in-memory source state.
- `crates/pending-github`: GitHub source adapter.
- `crates/pending-plane`: Plane source adapter.
- `crates/pending-google`: Google Calendar source adapter (GNOME Online Accounts).
- `crates/pending-ical`: calendar (iCal) source adapter.
- `crates/pending-todoist`: Todoist source adapter.
- `crates/pending-api`: HTTP API over the shared dashboard snapshot.
- `crates/pending-tui`: terminal dashboard.
- `frontend`: vanilla TypeScript UI built with Vite.
- `docs`: architecture and operation notes.

## Current State

The source contract (`PendingSource`) and the snapshot rules live in `pending-core`; see [docs/architecture.md](docs/architecture.md). The API loads sources from the config file and refreshes them through `pending-runtime`. The dashboard is a kanban: boards (Work, Personal, …) are tabs, each source is a group of columns, and each column is a filter declared in the config. Sources available: `github` (search queries), `plane` (work item filters), `google` (Google Calendar via GNOME Online Accounts), `ical` (any iCal feed), `todoist` (Todoist filters) and the built-in `sample`. The TUI runs the same runtime in-process (no API needed). The web dashboard polls the API every 15 seconds and shows source health; the API serves it from `--static-dir` or the release bundle's `frontend/`.
