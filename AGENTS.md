# AGENTS.md

This is the canonical operating guide for agents working in this repository.

## Product Intent

TasksPending aggregates pending work from multiple refreshable sources and shows one dashboard model in a native Linux window:

- a Rust daemon (`tasks-pending serve`) that refreshes the sources and answers a local HTTP API;
- a Rust GTK4/libadwaita window (`tasks-pending`, the default command) that reads that API, or runs the sources itself when no daemon answers.

Use the vocabulary in `crates/pending-core`: `DashboardSnapshot`, `Board` (a filter/area), `Group` (one source's stacks), `Column` (one filter; shown and configured as a "stack", `[[sources.stacks]]`), `PendingCard`, and `SourceStatus`.

## Commands

- `make` or `make help`: list every target with a one-line description (from the `## ` comment on each target; add one to any new target).
- `make bootstrap`: fetch the Rust dependencies.
- `make doctor`: read-only environment diagnostics (Rust, pkg-config, gtk4 and libadwaita development files).
- `make ci-check`: read-only verification gate.
- `make ci`: format, then run the verification gate.
- `make test`: Rust tests.
- `make up`: run the daemon on `127.0.0.1:61000`.
- `make app`: open the window (over the daemon when it runs, else over the sources).
- `make sandbox`: open the window over deterministic simulated cards, without reading config, credentials, cache, or external sources.
- `make screenshots`: regenerate the README pictures in `docs/assets` from the sandbox window (`tasks-pending sandbox --screenshot <file> --scene board|light|sources|settings|snoozed`); run it after a change that alters how the window looks. It needs a display that is drawing frames (not one on DPMS off). Without one, GTK's Broadway backend works headless at 1024×768: `gtk4-broadwayd :5 &` then `GDK_BACKEND=broadway BROADWAY_DISPLAY=:5 make screenshots`.
- `make changelog-check`: check `CHANGELOG.md`'s sections.
- `make install-test` / `make release-installer-test` / `make release-assets-test`: validate the source installer, the public release installer and its stable download aliases without network access or changes to the machine.
- `make release-local` / `make release-smoke`: build the release bundle under `dist/`, and smoke test it.
- `make install` / `make uninstall`: build a release and install it under `PREFIX` (default `~/.local`) with the desktop entry, icon and user service, or remove it.
- `make deploy`: build, install under `~/.local`, (re)start the daemon and check that it answers; the idempotent way to put a checkout to work on this machine.

Do not treat `make ci` as read-only; it runs formatters. Use `make ci-check` when measuring current state.

## Architecture Rules

- Keep source-specific API details out of the daemon and the window. Normalize into `pending-core`.
- Put contracts and invariants in the core crate first, then adapt them in the API, the client and the window.
- The window talks to a dashboard only through `pending-client`, over loopback. When it must run the sources itself, it serves `pending-api`'s routes on a private port and uses the same client: one code path.
- What the window shows is worked out in `pending-app`'s `present` module without widgets, and tested there; the `ui` modules only build widgets from it.
- Prefer explicit small ports over dynamic registries until there is real duplication.
- Avoid copying volatile provider details into docs; link to source code or live provider docs instead.
- Secrets stay out of git. Commit examples, never real tokens.
- The repository is in English: code, comments, test descriptions, UI text, docs and commit messages.

## The Window

GTK4 and libadwaita, through `gtk4-rs` and `libadwaita-rs` without a component framework. The oldest supported versions are GTK 4.14 and libadwaita 1.5 (Ubuntu 24.04, where CI builds); do not use newer API without raising the crate features and the documented minimum.

Visual style: simple and quiet, described in `crates/pending-app/DESIGN.md` (read it before changing how the window looks). Colours come from the app's own palette in `crates/pending-app/src/theme.rs` (dark by default, light when the system asks), filled into `crates/pending-app/src/style.css`; no Omarchy or GTK theme colours. Icons are the system's symbolic icons (names present in Adwaita and Yaru); source logos are fetched from their configured URL. Cards carry no severity colour or label. Thin 1px borders over shadows, one accent, 6/12/18 px rhythm.

## Changes That Travel Together

- **New source kind:**
  - `SourceKind` (pending-core config);
  - a `sources` arm in `pending_runtime::config::plan`;
  - a new crate with its stack type (`deny_unknown_fields`);
  - `docs/operations.md` (kind table and section);
  - `config.example.toml`;
  - `docs/install.md` (tokens table).
- **Model change (`pending-core` types):** struct literals in `pending-api`, `pending-client` and `pending-app` tests usually need the new field. The API's JSON is the contract the window reads; keep it backwards compatible (`#[serde(default)]`) because a window may talk to an older daemon.
- **Per-stack option for every kind:** `StackConfig` (pending-core), then `SourceSpec` / `SourceReport` if the snapshot needs it.
- **State the user changes from the window (marks, snoozes):** the store in `pending-runtime` (`marks.rs`, `snoozes.rs`, `looks.rs`, all on `store.rs`), the `Dashboard` trait, a route in `pending-api` guarded by the dashboard header, a method in `pending-client`, a `Command` in `pending-app`'s backend, and the snapshot field the window reads. Window-only choices go in `View` instead.
- **New window setting:**
  - `View` (pending-app `view.rs`, with its default and validation on load);
  - the settings dialog (`ui/dialogs.rs`);
  - `present.rs` if it changes what is shown;
  - `docs/operations.md`.
- **New API route:** `pending-api` (with a test), `pending-client` (with a test against `sandbox_app`), `docs/operations.md` ("HTTP Endpoints").
- **New CLI command or flag:** the `tasks-pending` CLI test runs Clap's definition checks; `scripts/test-release-bundle.sh` runs `--help` on every command; update `docs/install.md`.
- **Service or install layout:**
  - `scripts/install.sh` and `scripts/test-install.sh`;
  - `scripts/install-release.sh` and `scripts/test-release-installer.sh`;
  - `packaging/` (systemd, applications, icons, PKGBUILD);
  - `tasks-pending setup` and `upgrade` (`crates/tasks-pending/src`);
  - `docs/install.md`.

## Release

- **Release notes:** `CHANGELOG.md` has one `## <version>` section per release, and the release workflow publishes that section as the notes. Changes not yet released go under `## Unreleased` at the top, renamed to the version when cutting the release. `make changelog-check` enforces the shape.
- **Cutting a release:**
  1. Bump `version` in `Cargo.toml` (workspace) and `pkgver` in `packaging/arch/PKGBUILD`. The window shows the workspace version on its own (the snapshot's `version`).
  2. Add the changelog section.
  3. Land it on `master` (features go through `develop`), then push the tag `v<version>`.
- **What the workflow does:**
  - It checks that the tag matches the version and the `PKGBUILD`, then runs `make ci-check`.
  - It builds the Linux x86_64 bundle (`scripts/release-local.sh`) on Ubuntu 24.04, whose GTK sets the oldest supported one, and an Arch package from it (`packaging/arch/PKGBUILD`).
  - It installs the Arch package in a container to check it, then publishes everything with a `.sha256` per asset and `SHA256SUMS`.
  - Run it by hand (`workflow_dispatch`, dry run by default) to build without publishing.
- **How users get it:** the public installer (`scripts/install-release.sh`, published as `tasks-pending-install.sh`) installs the package on Arch and the bundle under `~/.local/lib/tasks-pending` elsewhere, then runs `tasks-pending setup`; `tasks-pending upgrade` fetches the next release the same way. Keep the installer, `upgrade` and `setup` in step: they share the directory layout, the environment variables and the `install.sh --start` hand-over.
- **Local checks:** `make release-local` builds the bundle for this machine; `make deploy` installs this checkout and restarts the daemon.
