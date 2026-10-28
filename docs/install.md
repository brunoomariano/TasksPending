# Install and run TasksPending

TasksPending is one executable. `tasks-pending` (or `tasks-pending app`) opens the window. `tasks-pending serve` runs the daemon: it refreshes your sources in the background and answers the window at <http://127.0.0.1:61000>, on this machine only. When no daemon is running, the window starts the sources itself, so it works either way; the daemon is what keeps the cards fresh while the window is closed and remembers what changed since you last looked.

## Platform

Linux x86_64, with Arch Linux and Omarchy as the operational target. The window needs GTK 4.14 and libadwaita 1.5 or newer (Ubuntu 24.04, Fedora 40, Arch) and glibc 2.39 or newer; on older systems, build from source.

## 1. Install and run

The supported path is one command. It installs the latest release, checks every download against its published SHA-256, and sets up the launcher entry and the daemon.

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/brunoomariano/TasksPending/releases/latest/download/tasks-pending-install.sh | sh
```

On Arch Linux, and on distributions built on it (Omarchy, Manjaro: `ID_LIKE=arch` in `/etc/os-release`), it installs the release's pacman package, letting pacman ask as usual when a terminal is there. Elsewhere it unpacks the release bundle under `~/.local/lib/tasks-pending`, after checking that the new executable runs on this machine, and links `~/.local/bin/tasks-pending` to it; it refuses to run as root, since everything it installs is yours. Run as root, the Arch path installs the package and stops there: `tasks-pending setup` and `tasks-pending upgrade` refuse root too, so run setup as yourself afterwards. Both paths end with `tasks-pending setup`:

- `~/.config/tasks-pending/config.toml` and `env` are created from the examples when missing, and kept as they are otherwise;
- the launcher caches are refreshed, so **TasksPending** shows up in the app launcher;
- the user service is started, or restarted when it was running, and the installer waits until the daemon answers where the unit says it listens.

`tasks-pending setup --no-start` does the first two and leaves the daemon alone, for a machine without systemd or to fill in the env file first.

Run the same command to update, or `tasks-pending upgrade` (see "Update and uninstall"). Your configuration, tokens and service override stay untouched. With the example configuration, the dashboard starts with the built-in sample source.

The installer reads a few optional variables: `TASKS_PENDING_VERSION` (a release to install instead of the latest, e.g. `0.6.0`), `TASKS_PENDING_LIB_DIR` and `TASKS_PENDING_BIN_DIR` (where the bundle and the link go; the desktop entry and icons stay under `~/.local/share`, where launchers look), `TASKS_PENDING_REPOSITORY` (another GitHub repository, `owner/name`) and `TASKS_PENDING_RELEASE_BASE_URL` (another releases location, `https://` only). The same variables drive `tasks-pending upgrade`.

About the checksums: each asset's `.sha256` is published beside it, on the same release page, so the check proves that the download arrived whole, not who built it. Whoever controls the releases location controls both, which is why only https origins you trust belong in `TASKS_PENDING_RELEASE_BASE_URL`. Signing the checksums (or publishing GitHub artifact attestations) is the planned next step; until then, the trust is in GitHub and in the repository's protected branches.

### Manual options

**Arch Linux**: download `tasks-pending-bin-<version>-1-x86_64.pkg.tar.zst` from the [releases](https://github.com/brunoomariano/TasksPending/releases) and run

```sh
sudo pacman -U tasks-pending-bin-*.pkg.tar.zst
tasks-pending setup
```

The executable goes to `/usr/bin`, the desktop entry and icon under `/usr/share`, and the user service to `/usr/lib/systemd/user/tasks-pending.service`.

**From a release bundle**: download `tasks-pending-<version>-x86_64-unknown-linux-gnu.tar.gz`, check it against its `.sha256`, unpack it and run its installer:

```sh
sha256sum -c tasks-pending-*.tar.gz.sha256
tar xzf tasks-pending-*.tar.gz
cd tasks-pending-*/ && ./scripts/install.sh --start
```

It installs under `~/.local` (set `PREFIX` to change it) and, with `--start`, hands over to `tasks-pending setup`. Without `--start` it prints the next steps instead. The bundle also runs in place: `./bin/tasks-pending sandbox` opens the window over simulated cards without installing anything.

**From source** (Rust 1.95 and the gtk4 and libadwaita development files; `make doctor` checks them):

```sh
make install          # build a release and install it under ~/.local
make deploy           # the same, then (re)start the daemon and check it answers
make uninstall        # remove it again (your config stays)
```

`make install` never touches your config or tokens and never starts the service; it prints the next steps. `make deploy` is the idempotent way to put this checkout to work on this machine.

## 2. Config file

The daemon and the window read `~/.config/tasks-pending/config.toml` (or `$XDG_CONFIG_HOME/tasks-pending/config.toml`; `--config` and `TASKS_PENDING_CONFIG` override both). `tasks-pending setup` creates it from the example; by hand:

```sh
mkdir -p ~/.config/tasks-pending
cp -n ~/.local/share/tasks-pending/config.example.toml ~/.config/tasks-pending/config.toml
# Arch package: /usr/share/tasks-pending/config.example.toml
```

The file lists your sources. The window follows its order: each `[[sources]]` is a group, left to right in file order, and each `[[sources.stacks]]` is a stack of cards inside it, top to bottom in file order. To reorder, move whole blocks.

```toml
refresh_seconds = 300        # how often each source refreshes
timeout_seconds = 60         # a slower refresh counts as a failure

[[sources]]
name = "agenda"              # unique; shown in the window and the Sources dialog
kind = "google"              # github, gitlab, jira, linear, plane, google, ical, todoist, sample
board = "Personal"           # area: the board filter in Settings
icon = "https://cdn.jsdelivr.net/gh/homarr-labs/dashboard-icons/svg/google-calendar.svg"

  [[sources.stacks]]
  name = "Today"
  when = ["now", "today"]    # filter keys depend on the kind

[[sources]]
name = "plane"
kind = "plane"
board = "Work"
timeout_seconds = 180        # per-source override (Plane is slow)

  [[sources.stacks]]
  name = "In review"
  assignee = "any"
  state = ["In Review"]
```

Every key, per kind, is in [`config.example.toml`](../config.example.toml) and [operations.md](operations.md). Saved changes are picked up within a couple of seconds, without restarting; a file with an error is reported in the **Sources** dialog while the previous config keeps running. Icons can come from [Dashboard Icons](https://dashboardicons.com) (`icon`, plus `icon_dark` for dark themes).

## 3. Tokens

Tokens never go in `config.toml`. The daemon reads them from its environment: `~/.config/tasks-pending/env`, one `KEY=value` per line, readable only by you. `tasks-pending setup` creates it from the example (mode 600); by hand:

```sh
cp -n ~/.local/share/tasks-pending/env.example ~/.config/tasks-pending/env
# Arch package: /usr/share/tasks-pending/env.example
chmod 600 ~/.config/tasks-pending/env
$EDITOR ~/.config/tasks-pending/env
```

After changing it, restart the user service (`tasks-pending setup` does that too).

| Source | Variables |
|---|---|
| `github` | `GITHUB_TOKEN` or `GH_TOKEN`; else `gh auth token` (the service's PATH includes `~/.local/bin` and mise shims) |
| `gitlab` | `GITLAB_TOKEN` (personal access token with `read_api`); `GITLAB_BASE_URL` for a self-managed instance (default `https://gitlab.com`) |
| `plane` | `PLANE_BASE_URL`, `PLANE_WORKSPACE` (or `PLANE_WORKSPACE_SLUG`), `PLANE_TOKEN` (or `PLANE_API_KEY`) |
| `jira` | `JIRA_BASE_URL`, plus `JIRA_EMAIL` and `JIRA_API_TOKEN` (Jira Cloud) or `JIRA_TOKEN` alone (Jira Data Center personal access token) |
| `linear` | `LINEAR_API_KEY` (a personal API key) |
| `todoist` | `TODOIST_API_TOKEN` |
| `google` | none: uses your GNOME Online Accounts login over the session bus |
| `ical` | `TASKS_PENDING_ICAL_URL` (the feed URL is a secret) |

Addresses that receive a token (`GITLAB_BASE_URL`, `JIRA_BASE_URL`, `LINEAR_API_URL`) must start with `https://`, so the token is never sent in the clear; `http://` is accepted only for `localhost`.

The window run without a daemon (`tasks-pending app --no-daemon`, or when none answers) uses your shell's environment instead.

## 4. The window

Open **TasksPending** from the app launcher, or run `tasks-pending`. It looks for the daemon at `http://127.0.0.1:61000` (`--daemon <url>` for another port) and, when none answers, starts the sources itself on a loopback port of its own and says so under the controls. `--no-daemon` skips the daemon on purpose. What the window shows and how it behaves is in [operations.md](operations.md).

**Omarchy**: the desktop entry makes the window a launcher app like any other, tiled by Hyprland. For a key that opens it or focuses it when it is already open, add to `~/.config/hypr/bindings.lua`:

```lua
o.bind("SUPER + ALT + T", "TasksPending", "omarchy-launch-or-focus TasksPending 'uwsm-app -- tasks-pending'")
```

(`TasksPending` matches the window's app id, `io.github.brunoomariano.TasksPending`.)

## 5. Manage the daemon

`tasks-pending setup` has already started the daemon. Use these commands to inspect it, or to start a manual installation:

```sh
systemctl --user enable --now tasks-pending   # start now and at login
systemctl --user status tasks-pending
journalctl --user -u tasks-pending -f         # logs
systemctl --user restart tasks-pending        # after editing the env file
```

The daemon only accepts loopback listen addresses, and its API only answers requests addressed to `localhost`, `127.0.0.1` or `[::1]`. Its address in a browser shows one line pointing at the app; there is no web page.

To use another port, override the command with a drop-in, which survives reinstalls (the installer rewrites the unit file itself), and open the window with `--daemon`:

```sh
systemctl --user edit tasks-pending
# [Service]
# ExecStart=
# ExecStart=/usr/bin/tasks-pending serve --listen 127.0.0.1:8090
tasks-pending --daemon http://127.0.0.1:8090
```

For a local install, replace `/usr/bin` with the absolute path to your own `tasks-pending` executable. `tasks-pending setup` reads the port from the unit as systemd shows it, so it waits for the daemon where it listens; when the service is active but does not answer there, it says so and leaves it running.

## 6. Update and uninstall

- `tasks-pending upgrade` (installed by the release installer): reads the latest release, downloads its bundle and checksum over https, verifies it, runs the new executable once to be sure it works on this machine, swaps it under `~/.local/lib/tasks-pending` and restarts the daemon; if the install does not finish, the previous release is put back. `--check` only says whether a newer release exists; `--version 0.6.1` (or `TASKS_PENDING_VERSION`) installs that one, and the installer's other variables apply too. Once a day the daemon looks at the latest release too, and the window shows `vX.Y.Z available` next to its version when there is one (`TASKS_PENDING_NO_VERSION_CHECK=1` in the env file turns that off).
- Arch: `sudo pacman -U` the new package, then `tasks-pending setup`.
- Bundle or source: run the new bundle's `./scripts/install.sh --start` (or `make deploy`) again.
- Uninstall: `./scripts/install.sh --uninstall` or `make uninstall` (stops and removes the service, the launcher entry and the executable or its link; your config, env file and cache stay, and so does the release directory of a release install, `~/.local/lib/tasks-pending`, which you remove by hand). Arch: `sudo pacman -R tasks-pending-bin`.

The last successful data of each source is cached in `$XDG_STATE_HOME/tasks-pending/` (`~/.local/state/tasks-pending/`), so the window has cards right after a restart. The window's own choices (board filter, collapsed stacks, hidden items, auto-refresh) are in `app-view.json` there.

## Troubleshooting

- **The window says the dashboard is unavailable**: it could reach neither the daemon nor start the sources; the line under the controls says which. `systemctl --user status tasks-pending`; if port 61000 is taken, use another port (see "Manage the daemon").
- **The window shows "no daemon running"**: it is showing the sources from your shell; start the service with `tasks-pending setup` to get background refreshes.
- **A source shows "not configured"**: its variables are missing from the env file; fix it and restart the service.
- **GitHub works in the window without a daemon but not with one**: `gh` is not on the service's PATH; set `GITHUB_TOKEN` in the env file.
- **No weather beside the clock on Omarchy**: the widget needs `omarchy-weather-status` to work for the daemon. Run it in a terminal first: it fails when offline or when the place is unknown (`omarchy-weather-location --set <name>`). The systemd service's PATH includes `/usr/share/omarchy/bin` and `~/.local/share/omarchy/bin`; a service file installed before that keeps the old PATH until you install again. A failure is retried after a minute, and the widget is hidden in windows narrower than 1440 px.
- **Google Calendar fails in the daemon**: the service must run inside your desktop session (it does when started with `systemctl --user` after login).
- **Config errors**: the **Sources** dialog shows the message, naming the file, source and stack.
- **`tasks-pending upgrade` refuses**: it only upgrades the executable the release installer linked, `~/.local/lib/tasks-pending/bin/tasks-pending`; a package install upgrades with pacman, a source install with `make deploy`.
