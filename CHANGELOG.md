# Changelog

## 0.6.1

### Security

- rustls, which TasksPending uses for HTTPS, is now 0.23.46, fixing four advisories in 0.23.45: [GHSA-c8m5-c952-6cm4](https://github.com/advisories/GHSA-c8m5-c952-6cm4) (formatting a session with `Debug` exposes its secrets), [GHSA-w44r-93j7-qv28](https://github.com/advisories/GHSA-w44r-93j7-qv28) (a forged ServerHello leaves later handshakes with only a classical X25519 key share), [GHSA-vp88-798v-h67f](https://github.com/advisories/GHSA-vp88-798v-h67f) (an ECH rejection can expose buffered data) and [GHSA-h2v8-394m-p7vv](https://github.com/advisories/GHSA-h2v8-394m-p7vv) (Brotli certificate decompression can allocate up to 1 GiB). Only the key exchange downgrade reached TasksPending: it builds no Brotli decompression, configures no ECH and never formats a session.

### Changed

- Dependabot no longer opens pull requests in this repository. A daily workflow keeps one issue labelled `dependabot` per package with open alerts, listing each advisory, its severity and the release that fixes it; the issue is edited as the alerts change and closed once none is open.

## 0.6.0

TasksPending is now a native Linux window. The web page and the terminal dashboard are gone, and so are the macOS and aarch64 builds: the release is Linux x86_64, GTK4 and libadwaita.

### Features

- `tasks-pending` opens the dashboard as a GTK4 window: the big clock and the weather beside it, the controls, the groups of stacks with their cards, the Now row, the snooze menu, the pin, and the Sources, Settings and Snoozed dialogs, with the same palette as the page had, dark unless the system asks for light. The keyboard reaches the board's links and buttons. Clicks, keys and scrolling count as looking, at most once a minute. The window reads the daemon at `http://127.0.0.1:61000` (`--daemon` for another address) and, when none answers within two seconds, runs the sources itself on a private loopback port and says so; `--no-daemon` does that on purpose. Opening the app again focuses the window that is open, without starting anything. Its choices (board filter, collapsed stacks, hidden items, auto-refresh) are kept in `$XDG_STATE_HOME/tasks-pending/app-view.json`.
- `tasks-pending sandbox` opens the window over simulated cards; `--screenshot <file>` pictures it and closes, which is how the README picture is made.
- `tasks-pending setup` creates `config.toml` and `env` from the examples when missing, refreshes the launcher caches it finds and starts or restarts the user service, waiting until the daemon answers, as this release, where the unit says it listens; `--no-start` only writes the files, and without systemd or a user manager it writes them and leaves the daemon to you. It refuses to run as root. The installers end with it.
- `tasks-pending upgrade` reads the latest release, downloads its bundle and checksum over https, verifies it, runs the new executable once, swaps it under `~/.local/lib/tasks-pending` and restarts the daemon, putting the previous release back, and running its installer again, if that does not finish; `--check` only reports, `--version` (or `TASKS_PENDING_VERSION`) installs that release when it is newer, an older one only with `--allow-downgrade`, and `TASKS_PENDING_REPOSITORY` or `TASKS_PENDING_RELEASE_BASE_URL` (https only) point at another location, as for the installer. Once a day the daemon looks at the latest release (by the wall clock, so a suspend counts, and a clock set back puts it off by a day at most), an hour later again when a look fails (`GET /api/v1/version`; `TASKS_PENDING_NO_VERSION_CHECK` set to anything but a blank value turns it off) and the window shows `vX.Y.Z available` next to its version.
- The public installer unpacks the release under `~/.local/lib/tasks-pending` and links `~/.local/bin/tasks-pending`, so upgrades swap the directory; it refuses archives with members that would write outside it, links or absolute paths, refuses to run as root, replaces only a directory that holds a release (never an install prefix), checks that the new executable runs before replacing the current one, and keeps the previous release until the new one is set up. On Arch, and on distributions built on it (`ID_LIKE=arch`), it installs the package, letting pacman ask when a terminal is there, and retires an earlier install under `~/.local` (the 0.5.0 bundle, or `make install`) whose user service would otherwise keep the old daemon running; config, tokens and drop-ins stay, and a drop-in that ran the old executable runs the package's. Run as root, it installs the package and leaves `tasks-pending setup` to the user. Every release asset, the installer included, has a `.sha256` published beside it, which proves a download arrived whole.
- The package and the installers ship a desktop entry (`io.github.brunoomariano.TasksPending`) and the icon (the bundle's under `~/.local/share`, wherever `TASKS_PENDING_BIN_DIR` puts the link), so TasksPending is in the app launcher and `omarchy-launch-or-focus TasksPending` focuses it.
- `make deploy` builds a release, installs it under `~/.local`, restarts the daemon and checks that it answers.

### Changed

- The daemon no longer serves a page: `GET /` is one line naming the app. The Omarchy web app launcher (`make webapp`) and `--static-dir` are gone.
- The Arch package depends on `gtk4` and `libadwaita`.
- The user service runs with `NoNewPrivileges=yes` and reads its tokens from `$XDG_CONFIG_HOME/tasks-pending/env` (`%E`), where `setup` creates the file, at the path systemd resolved for the unit rather than under the shell's `XDG_CONFIG_HOME`; the config goes under the user manager's.
- A window reads a source status or card severity that a newer daemon added as degraded or info, instead of failing to read the dashboard.
- `make ci-check` also checks `CHANGELOG.md` (`## Unreleased` only at the top, then one section per version, newest first), and CI runs shellcheck at `--severity=info`.

### Removed

- The web page (Vite and TypeScript), the TUI (`tasks-pending tui`), the macOS and aarch64 Linux bundles, and the launchd service. On macOS, 0.5.0 remains the last version.

## 0.5.0

### Features

- Every provider shows its Dashboard Icons logo by default: iCal sources now have one too, like GitHub, Google Calendar, Plane and Todoist. `icon` still overrides it.
- Any stack can leave cards out with `exclude = ["pattern", ...]` in the config: a card whose title or body matches one of the patterns (regular expressions, compared without case) is absent from that stack only, for example to hide bot pull requests with `"dependabot\\[bot\\]"`. An invalid pattern, or one that matches the empty text (such as `"bot|"`), fails when the config loads, naming the source and the stack.
- A GitHub source without `[[sources.stacks]]` now splits your pull requests by what to do next: "Returned to you" (changes requested, a warning), "Ready to merge" (approved), "Not approved yet" (neither of the two) and "Drafts", between "Review requested" and "Assigned issues". This replaces the single "My pull requests" stack, so the default GitHub group looks different and runs six searches per refresh instead of three. Sources with their own stacks are unchanged. See "GitHub Source" in [docs/operations.md](docs/operations.md).
- GitHub pull request cards show the checks of their last commit: `checks failing`, `checks pending` or `checks passing`, and nothing for a pull request without checks. Failing checks make the card at least a warning. The state comes in the lookup that already asks for the review state, so no request is added.
- New `gitlab` source: your open merge requests (review requested, assigned or authored), open issues (assigned or authored) and pending to-do items, on gitlab.com or a self-managed instance. Stacks can be narrowed by group, project, labels and draft state; overdue issues are critical. It needs `GITLAB_TOKEN` (a personal access token with `read_api`) and, for a self-managed instance, `GITLAB_BASE_URL`. See "GitLab Source" in [docs/operations.md](docs/operations.md).
- New `jira` source: one JQL query per stack, for Jira Cloud (`JIRA_EMAIL` and `JIRA_API_TOKEN`) and Jira Data Center (`JIRA_TOKEN`), both with `JIRA_BASE_URL`. By default it shows your issues in progress and to do. Cards show the issue key, summary, project, status, assignee and due date; overdue and top-priority issues are critical. See "Jira Source" in [docs/operations.md](docs/operations.md).
- New source `kind = "linear"`: Linear issues, by default the ones assigned to you, in the In progress, To do and Backlog stacks. Stacks filter by `assignee`, `state_type`, `state`, `team` and `priority`. Cards show the issue reference as the title and mark overdue and urgent issues as critical. Set `LINEAR_API_KEY` (a personal API key) in the environment. See "Linear Source" in [docs/operations.md](docs/operations.md).
- The GitLab, Jira and Linear sources were built from each provider's API documentation and are covered by tests against stand-in servers; they have not yet been run against live accounts. Please report anything that behaves differently.
- Snooze a card from the web page or the TUI (`z`, and `Z` for the list): it leaves its stacks for an hour, until tomorrow, until next week or until it changes, and always comes back early when its item changes. A quiet "N snoozed" link opens the list to wake a card. Snoozes are kept by the daemon and nothing is written to the provider.
- A small dot marks the cards that changed since you last looked, and the stacks holding them, on the web page and in the TUI. Coming back after 10 minutes away shows what changed in between, including what the dashboard only fetched after you left; the dots clear on the next return. Only deliberate actions (a click, a key, scrolling) count as looking. No counts, no notifications.
- `tasks-pending sandbox` shows the current features: GitHub stacks by next action with review and check state, a Linear group, issue references as titles, and some cards flagged as changed. `make screenshots` regenerates the README pictures from it.

### Fixed

- Calendar events without a modification time no longer look changed on every refresh.
- A mark that cannot be written to disk is now reported (HTTP 500, and a notice in the TUI) instead of looking saved.

## 0.4.0

### Breaking change

- The old command names `pending-api` and `pending-tui` are gone: the Arch package no longer installs them as aliases, the executable no longer answers to them, and the installer no longer migrates 0.1 services that call them. Use `tasks-pending serve` and `tasks-pending tui`. See "Upgrading from 0.1" in [docs/install.md](docs/install.md).

### Features

- `tasks-pending sandbox` (and `make sandbox`) runs the dashboard with simulated cards at <http://127.0.0.1:61001>, without configuration, credentials or network access.
- Mark any card as in progress with its pin button on the web page. Marked cards are highlighted and repeated in a "Now" row above the stacks, which disappears when nothing is marked. Marks are kept by the daemon, shared by every browser, and dropped when the item is finished. In the TUI, `m` marks or unmarks the selected card, with the same marks shown in a "Now" panel above the groups.
- The running version is visible: after the last update time on the web page, and in the TUI header.
- Any stack can be switched off in the config with `enabled = false`: it is not queried and not shown, in the web page or the TUI.
- On Omarchy, the web dashboard shows a small weather widget beside the clock (condition icon, temperature, place and wind, from Omarchy's weather commands through the new `GET /api/v1/weather`). It is absent when weather is unavailable and hidden in windows narrower than 1440 px. The systemd service's PATH now includes Omarchy's command directories; install again to update an existing service file.
- The TUI lays the board out like the web page: groups side by side, each with its stacks one above another under a header with the name and count. `j`/`k` flow from one stack to the next, and `c` (or a click on the header) collapses or expands a stack.

### Fixed

- The web page now redraws when only the config error changes, so a broken config shows up without waiting for the cards to change.
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
