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

Run the frontend dev server:

```sh
make frontend-dev
```

## Repository Shape

- `crates/pending-core`: shared domain model, config types, and source contracts.
- `crates/pending-api`: HTTP API over the shared dashboard snapshot.
- `crates/pending-tui`: terminal dashboard.
- `frontend`: vanilla TypeScript UI built with Vite.
- `docs`: architecture and operation notes.

## Current State

This is a bootstrap commit: the app renders a sample snapshot while the source ingestion layer is designed. The next product decisions are the source contract, persistence/cache strategy, and the first real GitHub source.
