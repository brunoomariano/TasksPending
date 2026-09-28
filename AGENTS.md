# AGENTS.md

This is the canonical operating guide for agents working in this repository.

## Product Intent

TasksPending aggregates pending work from multiple refreshable sources and exposes one dashboard model through:

- a Rust TUI;
- a Rust HTTP API;
- a lightweight TypeScript frontend.

Use the vocabulary in `crates/pending-core`: `DashboardSnapshot`, `Board` (a tab/area), `Group` (one source's columns), `Column` (one filter), `PendingCard`, and `SourceStatus`.

## Commands

- `make bootstrap`: install/fetch local dependencies.
- `make doctor`: read-only environment diagnostics.
- `make ci-check`: read-only verification gate.
- `make ci`: format, then run the verification gate.
- `make test`: Rust tests.
- `make up`: run the API on `127.0.0.1:8080`.
- `make serve`: build the frontend and run the API serving it on `127.0.0.1:8080`.
- `make tui`: run the TUI.
- `make contract`: regenerate `frontend/src/contract.gen.ts` from the `pending-core` model.
- `make frontend-dev`: run the frontend dev server.
- `make release-local`: build a local release bundle under `dist/`.

Do not treat `make ci` as read-only; it runs formatters. Use `make ci-check` when measuring current state.

## Architecture Rules

- Keep source-specific API details out of UI crates. Normalize into `pending-core`.
- Put contracts and invariants in the core crate first, then adapt them in API/TUI/frontend.
- The frontend never hand-writes API types: it imports them from `contract.gen.ts`, and a core test fails when that file is stale.
- Prefer explicit small ports over dynamic registries until there is real duplication.
- Avoid copying volatile provider details into docs; link to source code or live provider docs instead.
- Secrets stay out of git. Commit examples, never real tokens.
- The repository is in English: code, comments, test descriptions, UI text, docs and commit messages.

## Frontend

The frontend is intentionally lightweight: Vite plus vanilla TypeScript. Keep framework adoption as an explicit decision when dashboard interaction becomes complex enough to justify it.

Visual style: simple and quiet. Colours come from the design tokens at the top of `frontend/src/styles.css` (dark by default, light when the system asks). Icons are inline SVG in `render.ts` (Lucide shapes), no icon font or package. Cards carry no severity colour or label.

## Release

GitHub Actions build Linux release artifacts for `pending-api`, `pending-tui`, and `frontend/dist`, plus SHA256 files. Local release packaging is available through `make release-local`.
