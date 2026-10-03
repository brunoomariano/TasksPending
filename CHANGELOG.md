# Changelog

## Unreleased

### Breaking change

- The old command names `pending-api` and `pending-tui` are gone: the Arch package no longer installs them as aliases, the executable no longer answers to them, and the installer no longer migrates 0.1 services that call them. Use `tasks-pending serve` and `tasks-pending tui`. See "Upgrading from 0.1" in [docs/install.md](docs/install.md).

### Features

- `tasks-pending sandbox` (and `make sandbox`) runs the dashboard with simulated cards at <http://127.0.0.1:61001>, without configuration, credentials or network access.
- Mark any card as in progress with its pin button on the web page. Marked cards are highlighted and repeated in a "Now" row above the stacks, which disappears when nothing is marked. Marks are kept by the daemon, shared by every browser, and dropped when the item is finished.
- Any stack can be switched off in the config with `enabled = false`: it is not queried and not shown, in the web page or the TUI.
- On Omarchy, the web dashboard shows a small weather widget beside the clock (condition icon, temperature, place and wind, from Omarchy's weather commands through the new `GET /api/v1/weather`). It is absent when weather is unavailable and hidden in windows narrower than 1440 px. The systemd service's PATH now includes Omarchy's command directories; install again to update an existing service file.

### Fixed

- The TUI board no longer runs a card's body lines together: line breaks show as ` · ` on the card, and the details popup keeps the full text.

## 0.3.1

### Fixed

- Linux installers now warn when `~/.config/tasks-pending/env` is missing, so it is clear that sources requiring credentials will remain unavailable. The warning covers both the standard bundle installer and the Arch package path used by the one-command installer.
- Existing environment files remain untouched during installation and updates.

## 0.3.0

### Breaking change

- The dashboard now listens on `http://127.0.0.1:61000` instead of port 8080, avoiding common development-port conflicts. Run the release installer again or update the Arch package, then update bookmarks and any manually created Omarchy web app. Existing explicit `--listen` overrides remain unchanged.

### Features

- Add a board-based TasksPending icon to the installed application, browser favicon, and README banner.

### Security

- Refuse non-loopback listen addresses because the local API has no authentication.

## 0.2.0

- Release one self-contained `tasks-pending` executable per platform instead of separate API and TUI binaries plus a frontend folder.
- Run the local dashboard with `tasks-pending serve` and the terminal dashboard with `tasks-pending tui`.
- Embed the production frontend in release builds while retaining `--static-dir` as a local development override.
- Keep `pending-api` and `pending-tui` aliases in the Arch package so existing service overrides continue to work during an upgrade.
- Install or update the latest release with one command, platform detection, SHA-256 verification, and automatic daemon start or restart.

## 0.1.0

First release: one local dashboard for the work waiting on you across GitHub, Plane, calendars and Todoist, as a background daemon with a web page, and as a terminal UI.

### Run it

- `pending-api` is a daemon that refreshes every source in the background and serves the dashboard at <http://127.0.0.1:8080>. It only answers requests addressed to local host names.
- It runs as a systemd user service on Linux or a launchd agent on macOS.
- On Omarchy, `make webapp` adds it to the launcher as a web app.
- `pending-tui` shows the same dashboard in the terminal, without the daemon.
- Packages:
  - an Arch Linux package (x86_64);
  - release bundles for Linux (x86_64, aarch64) and macOS (Apple silicon, Intel), each with an installer;
  - `make install` / `make uninstall` from source.
- Install and configuration guide: [docs/install.md](docs/install.md).

### Sources

- **GitHub:**
  - stacks of search queries (review requests, your PRs, issues);
  - notifications (`unread` or `all`);
  - Dependabot, secret scanning and code scanning alerts for chosen repos (`alerts = ["owner/repo"]`);
  - pull request review state (approved, changes requested, review required) and draft status;
  - comment counts on cards.
- **Plane:**
  - work items filtered by assignee, state, state group, project and priority, with the issue reference as the card title;
  - one request at a time, 15 s per request, to spare self-hosted servers.
- **Google Calendar** through GNOME Online Accounts (no extra login). **iCal** feeds.
  - Both expand recurrences and sort events into time buckets (now, today, tomorrow, later).
- **Todoist** filter queries.

### Dashboard

- **Layout:**
  - each source is a group of collapsible stacks;
  - groups run left to right and stacks top to bottom, both in config order;
  - each group shows the source's icon (e.g. from dashboardicons.com).
- **Web page:**
  - a big block clock, with Sources, Refresh and Settings beside it;
  - Settings: board filter, show/hide switches for any board, group or stack, and auto-refresh (default every minute).
- **TUI:**
  - the same block clock (hidden on small terminals);
  - a details popup for the selected card (`Space`) and one for the sources (`s`);
  - PageUp/PageDown, Home/End, mouse wheel and click to move around.
- **Stack sorting:** stacks can list oldest items first (`sort = "oldest"`) to surface stale work.

### Reliability

- **Refresh and failure handling:**
  - each source refreshes on its own schedule, with backoff after failures and a per-source timeout;
  - a failing source never takes the dashboard down, and it keeps its last data, marked as stale.
- **Cache:** the last good data is cached between restarts.
- **Config reload and validation:**
  - saving the config applies it without restarting;
  - a broken file is reported while the previous config keeps running;
  - unknown keys, duplicate names and invalid filters are rejected, naming the file, source and stack.
- **Secrets:** tokens come only from the environment and never appear in logs or on the page.
