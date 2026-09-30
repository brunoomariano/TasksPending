# AGENTS.md

This is the canonical operating guide for agents working in this repository.

## Product Intent

TasksPending aggregates pending work from multiple refreshable sources and exposes one dashboard model through:

- a Rust TUI;
- a Rust HTTP API;
- a lightweight TypeScript frontend.

Use the vocabulary in `crates/pending-core`: `DashboardSnapshot`, `Board` (a tab/area), `Group` (one source's stacks), `Column` (one filter; shown and configured as a "stack", `[[sources.stacks]]`), `PendingCard`, and `SourceStatus`.

## Commands

- `make` or `make help`: list every target with a one-line description (from the `## ` comment on each target; add one to any new target).
- `make bootstrap`: install/fetch local dependencies.
- `make doctor`: read-only environment diagnostics.
- `make ci-check`: read-only verification gate.
- `make ci`: format, then run the verification gate.
- `make test`: Rust tests.
- `make up`: run the API on `127.0.0.1:61000`.
- `make serve`: build the frontend and run the API serving it on `127.0.0.1:61000`.
- `make sandbox`: build the frontend and run deterministic simulated cards on `127.0.0.1:61001`, without reading config, credentials, cache, or external sources.
- `make tui`: run the TUI.
- `make contract`: regenerate `frontend/src/contract.gen.ts` from the `pending-core` model.
- `make release-installer-test` / `make release-assets-test`: validate the public release installer and its stable download aliases without network access.
- `make frontend-dev`: run the frontend dev server.
- `make install-test`: test installer behavior without changing the local system.
- `make release-local`: build a local release bundle under `dist/`.
- `make install` / `make uninstall`: build a release and install it under `PREFIX` (default `~/.local`) with the user service, or remove it.
- `make webapp`: create the Omarchy web app for the dashboard.

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

## Changes That Travel Together

- **New source kind:**
  - `SourceKind` (pending-core config);
  - a `sources` arm in `pending_runtime::config::plan`;
  - a new crate with its stack type (`deny_unknown_fields`);
  - `docs/operations.md` (kind table and section);
  - `config.example.toml`;
  - `docs/install.md` (tokens table).
- **Model change (`pending-core` types with `TS`):** run `make contract` and commit `frontend/src/contract.gen.ts`, because a core test fails when it is stale. Struct literals in `pending-tui` and `pending-api` tests usually need the new field.
- **Per-stack option for every kind:** `StackConfig` (pending-core), then `SourceSpec` / `SourceReport` if the snapshot needs it.
- **New web setting:**
  - `View` and `DEFAULT_VIEW` (render.ts);
  - the settings dialog;
  - `savedView` / `saveView` (main.ts, validated on load);
  - `docs/operations.md`.
- **New TUI key:**
  - `app.rs` handling and tests;
  - the footer help;
  - the TUI section of `docs/operations.md`.
- **New CLI flag:** the `tasks-pending` CLI test runs Clap's definition checks; update `docs/install.md` if it matters to the service file.
- **Service or install layout:**
  - `scripts/install.sh`;
  - `packaging/` (systemd, launchd, PKGBUILD);
  - the embedded frontend build in `pending-api`;
  - `docs/install.md`.

## Release

- **Release notes:** `CHANGELOG.md` has one `## <version>` section per release, and the release workflow publishes that section as the notes.
- **Cutting a release:**
  1. Bump `version` in `Cargo.toml` (workspace) and `frontend/package.json`.
  2. Add the changelog section.
  3. Land it on `master`, then push the tag `v<version>`.
- **What the workflow does:**
  - It checks that the tag matches the version, then runs `make ci-check`.
  - It builds bundles for `x86_64`/`aarch64` Linux and macOS (`scripts/release-local.sh`; aarch64 Linux is cross-compiled with Ubuntu's `gcc-aarch64-linux-gnu`) and an Arch package from the x86_64 bundle (`packaging/arch/PKGBUILD`).
  - It installs the Arch package in a container to check it, then publishes everything with `SHA256SUMS`.
  - Run it by hand (`workflow_dispatch`, dry run by default) to build without publishing.
- **Local checks:** `make release-local` builds the bundle for this machine; `make install` / `make uninstall` install from source under `PREFIX` (default `~/.local`).
