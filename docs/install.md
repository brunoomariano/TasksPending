# Install and run TasksPending

TasksPending is one executable. `tasks-pending serve` runs the local daemon: it refreshes your sources in the background and serves the dashboard at <http://127.0.0.1:61000>, on this machine only. `tasks-pending tui` is the same dashboard in the terminal; it reads the same config and does not need the daemon. Release builds include the web page inside the executable.

## Platform focus

Arch Linux is the supported operational target right now. The release workflow also builds generic Linux and macOS bundles to catch portability regressions, but those artifacts do not have the same installation and operational coverage as the Arch package.

## 1. Install and run

The supported path is one command. It installs the latest compatible release, validates the downloaded SHA-256, and starts the dashboard at <http://127.0.0.1:61000>.

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/brunoomariano/TasksPending/releases/latest/download/tasks-pending-install.sh | sh
```

It uses the validated Arch package on x86_64 Arch Linux and the matching bundle everywhere else. Run the same command to update: your configuration, tokens and service override stay untouched, and an active daemon restarts with the new version. With no configuration file, the dashboard starts with the built-in sample source.

### Manual options

Use these when you prefer to download an artifact yourself or build from source. They install `tasks-pending` and its user service but leave starting it as an explicit step.

**Arch Linux** (x86_64): download `tasks-pending-bin-<version>-1-x86_64.pkg.tar.zst` from the [releases](https://github.com/brunoomariano/TasksPending/releases) and run

```sh
sudo pacman -U tasks-pending-bin-*.pkg.tar.zst
```

The executable goes to `/usr/bin`; its user service goes to `/usr/lib/systemd/user/tasks-pending.service`.

**Linux or macOS, from a release bundle**: download `tasks-pending-<version>-<target>.tar.gz` for your machine (`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `aarch64-apple-darwin` for Apple silicon, `x86_64-apple-darwin` for Intel Macs), check it against its `.sha256`, unpack it and run its installer:

```sh
sha256sum -c tasks-pending-*.tar.gz.sha256   # macOS: shasum -a 256 -c …
tar xzf tasks-pending-*.tar.gz
cd tasks-pending-*/ && ./scripts/install.sh
```

It installs under `~/.local` (set `PREFIX` to change it). The Linux binaries need glibc 2.39 or newer (Ubuntu 24.04, Debian 13, Fedora 40, Arch); on older systems, build from source. The bundle also runs in place: `./bin/tasks-pending serve` starts the dashboard without a separate web page folder.

macOS may block binaries downloaded by a browser; if it does, run `xattr -d com.apple.quarantine bin/*` in the unpacked folder before installing.

**From source** (Rust 1.95 and Node 26, see `.mise.toml`):

```sh
make install          # build a release and install it under ~/.local
make uninstall        # remove it again (your config stays)
```

`make install` never touches your config or tokens and never starts the service; it prints the next steps.

## 2. Config file

The daemon and the TUI read `~/.config/tasks-pending/config.toml` (or `$XDG_CONFIG_HOME/tasks-pending/config.toml`; `--config` and `TASKS_PENDING_CONFIG` override both). Start from the example:

```sh
mkdir -p ~/.config/tasks-pending
cp -n ~/.local/share/tasks-pending/config.example.toml ~/.config/tasks-pending/config.toml
# Arch package: /usr/share/tasks-pending/config.example.toml
```

The file lists your sources. The page follows its order: each `[[sources]]` is a group, left to right in file order, and each `[[sources.stacks]]` is a stack of cards inside it, top to bottom in file order. To reorder, move whole blocks.

```toml
refresh_seconds = 300        # how often each source refreshes
timeout_seconds = 60         # a slower refresh counts as a failure

[[sources]]
name = "agenda"              # unique; shown in the page and Sources dialog
kind = "google"              # github, plane, google, ical, todoist, sample
board = "Personal"           # area: web filter and TUI tab
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

Every key, per kind, is in [`config.example.toml`](../config.example.toml) and [operations.md](operations.md). Saved changes are picked up within a couple of seconds, without restarting; a file with an error is reported in the page's **Sources** dialog while the previous config keeps running. Icons can come from [Dashboard Icons](https://dashboardicons.com) (`icon`, plus `icon_dark` for dark themes).

## 3. Tokens

Tokens never go in `config.toml`. The daemon reads them from its environment. On Linux, put them in `~/.config/tasks-pending/env`, one `KEY=value` per line, readable only by you:

```sh
cp -n ~/.local/share/tasks-pending/env.example ~/.config/tasks-pending/env
# Arch package: /usr/share/tasks-pending/env.example
chmod 600 ~/.config/tasks-pending/env
$EDITOR ~/.config/tasks-pending/env
```

On Linux, the installer warns when this file is absent. It does not create it or import values from other applications; create it yourself and restart the user service after changing it.

| Source | Variables |
|---|---|
| `github` | `GITHUB_TOKEN` or `GH_TOKEN`; else `gh auth token` (the service's PATH includes `~/.local/bin` and mise shims) |
| `gitlab` | `GITLAB_TOKEN` (personal access token with `read_api`); `GITLAB_BASE_URL` for a self-managed instance (default `https://gitlab.com`) |
| `plane` | `PLANE_BASE_URL`, `PLANE_WORKSPACE` (or `PLANE_WORKSPACE_SLUG`), `PLANE_TOKEN` (or `PLANE_API_KEY`) |
| `todoist` | `TODOIST_API_TOKEN` |
| `google` | none: uses your GNOME Online Accounts login over the session bus (Linux desktop only) |
| `ical` | `TASKS_PENDING_ICAL_URL` (the feed URL is a secret) |

On macOS, put them in the `EnvironmentVariables` block of `~/Library/LaunchAgents/com.github.brunoomariano.tasks-pending.plist` (the installer makes it readable only by you).

The TUI, run from your shell, uses your shell's environment instead.

## 4. Manage the daemon

The one-command installer has already started the daemon. Use these commands to inspect it, or to start a manual installation.

**Linux (systemd user service)**

```sh
systemctl --user enable --now tasks-pending   # start now and at login
systemctl --user status tasks-pending
journalctl --user -u tasks-pending -f         # logs
systemctl --user restart tasks-pending        # after editing the env file
```

**macOS (launchd)**

```sh
launchctl bootout gui/$(id -u)/com.github.brunoomariano.tasks-pending   # stop
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.github.brunoomariano.tasks-pending.plist
tail -f ~/Library/Logs/tasks-pending.log
```

Then open <http://127.0.0.1:61000>. The server only accepts loopback listen addresses, and the API only answers requests addressed to `localhost`, `127.0.0.1` or `[::1]`.

To use another port on Linux, override the command with a drop-in, which survives reinstalls (the installer rewrites the unit file itself):

```sh
systemctl --user edit tasks-pending
# [Service]
# ExecStart=
# ExecStart=/usr/bin/tasks-pending serve --listen 127.0.0.1:8090
```

For a local install, replace `/usr/bin` with the absolute path to your own `tasks-pending` executable.

On macOS, edit `--listen` in the plist. Reinstalling leaves your plist untouched (it holds your tokens) and writes the current template next to it as `.plist.new`, for you to compare. Use the new port in the web app URL too.

Upgrading from 0.1: the `pending-api` and `pending-tui` commands no longer exist, and the installer no longer rewrites services that call them. Change such a service or drop-in to `tasks-pending serve --listen <address>`, and delete the old `pending-api`, `pending-tui` and `share/tasks-pending/frontend` by hand.

## 5. Omarchy web app

With the daemon running:

```sh
make webapp            # from the repo, or:
omarchy-webapp-install "TasksPending" http://127.0.0.1:61000 ~/.local/share/tasks-pending/tasks-pending.png
```

It adds **TasksPending** to the app launcher, opening the dashboard in its own window. Remove it with `omarchy-webapp-remove TasksPending`. Elsewhere, open the URL in a Chromium-based browser and install it as an app from the address bar.

## 6. Update and uninstall

- Arch: `sudo pacman -U` the new package, then `systemctl --user restart tasks-pending`.
- Bundle or source: run the new bundle's `./scripts/install.sh` (or `make install`) again, then restart the service.
- Uninstall: `./scripts/install.sh --uninstall` or `make uninstall` (stops and removes the service; your config, env file and cache stay). Arch: `sudo pacman -R tasks-pending-bin`.

The last successful data of each source is cached in `$XDG_STATE_HOME/tasks-pending/` (`~/.local/state/tasks-pending/`), so the page has cards right after a restart.

## Troubleshooting

- **The page says the API is unavailable**: `systemctl --user status tasks-pending`; if port 61000 is taken, use another port (see "Run the daemon").
- **A source shows "not configured"**: its variables are missing from the env file; fix it and restart the service.
- **GitHub works in the terminal but not in the daemon**: `gh` is not on the service's PATH; set `GITHUB_TOKEN` in the env file.
- **No weather beside the clock on Omarchy**: the widget needs `omarchy-weather-status` to work for the daemon. Run it in a terminal first: it fails when offline or when the place is unknown (`omarchy-weather-location --set <name>`). The systemd service's PATH includes `/usr/share/omarchy/bin` and `~/.local/share/omarchy/bin`; a service file installed before that keeps the old PATH until you install again. A failure is retried after a minute, and the widget is hidden in windows narrower than 1440 px.
- **Google Calendar fails in the daemon**: the service must run inside your desktop session (it does when started with `systemctl --user` after login).
- **Config errors**: the **Sources** dialog shows the message, naming the file, source and stack.
