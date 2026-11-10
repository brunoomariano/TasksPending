# TasksPending

![TasksPending dashboard banner](docs/assets/tasks-pending-banner.svg)

TasksPending is a local-first dashboard of your pending work: pull requests, issues, tasks and events from the tools you use, in one native Linux window. A small daemon refreshes the sources in the background; the window reads it.

![TasksPending dropped down from Omarchy's bar: the window floats over an empty workspace, centred at 80% of the screen, on the Activity tab with simulated cards in Picked, In Progress and Done / to mark](docs/assets/tasks-pending-dropdown.png)

<p align="center">
  <img src="docs/assets/tasks-pending-bar.png" alt="Close-up of Omarchy's bar: TasksPending's icon, ringed, next to the tray, with the dropped-down window below it" width="590">
</p>

A click on the icon (ringed) drops the window down, and the next sends it away. Every picture here comes from `tasks-pending sandbox`, which shows simulated cards without any configuration or credentials: `make screenshots` regenerates the window's own, and `scripts/dropdown-picture.sh` the two on Omarchy's desktop, run there by hand.

## What it shows

- **One group per source, in your order.** GitHub, GitLab, Jira, Linear, Plane, Todoist, Google Calendar and iCal feeds, each with its logo. Inside a group, every stack is a filter you declare in the config; stacks collapse, and long ones show their first cards.
- **What to do next.** GitHub pull requests are split by next action (review requested, returned to you, ready to merge, not approved yet, drafts), with review state, checks and comment counts on the card.
- **Activity.** Send the cards you are working on to the Activity tab, a board of three stages: Picked, In Progress, Done / to mark. The window opens on it; the Board tab holds the groups, where the picked cards stay highlighted.
- **One menu per card.** Every action sits behind a three-dot button on the card: send to Activity, move between stages, snooze, alert, and for a todo edit and delete.
- **The keyboard, too.** What the mouse does has a key, and the mouse keeps working: **?** or **Ctrl+K** lists the keys, searched by what each does. **j**/**k** and **h**/**l**, or the arrows, move between the cards, and only the cards, **Tab** switches between the Activity and the Board, **Enter** sends one to the Activity or takes it back, **Shift+→** and **Shift+←** move it along the stages, **o** opens its link and **Ctrl+.** its menu.
- **Snooze.** Hide a card for an hour, until tomorrow, until next week, or until it changes. It always comes back when the item changes, and nothing is written to the provider.
- **Alerts.** Ask to be alerted about a card in 15 minutes, 30 minutes, an hour or three hours: when the time comes, a desktop notification names it, and clicking it brings the window up.
- **Todos.** Write down something that comes from no source: **New** in the Picked column of the Activity puts it there, it moves through the stages like the other cards, and it is still there after a restart.
- **What changed.** A small dot marks the cards that changed since you last looked.
- **Omarchy's theme.** The window has its own palette, dark unless the system asks for light. On Omarchy, Settings can switch it to the theme Omarchy applied, and it changes with it.
- **On Omarchy's bar.** Settings can put an icon on Omarchy's bar: a click drops the window down over whatever you are doing, on the page it was on, and the next click, or Esc, sends it away. `tasks-pending toggle` does the same from a key. An alert that rang while the window was away leaves a dot on the icon until the window comes to the front.
- **Local only.** The daemon listens on this machine, reads tokens from the environment, and keeps your marks and snoozes in a state folder. The window reads it over loopback; it reaches the network itself only for the sources' logos, and for the providers when no daemon is running and it runs the sources in its own process.

<p align="center">
  <img src="docs/assets/tasks-pending-board.png" alt="The Board tab: one group of stacks per source, with the picked cards highlighted" width="49%">
  <img src="docs/assets/tasks-pending-todo.png" alt="The New todo dialog: a title, optional details, and the note that todos are kept while alerts last as long as TasksPending runs" width="49%">
</p>

<p align="center">
  <img src="docs/assets/tasks-pending-keys.png" alt="The Keys dialog: a search by what a key does over the keys of the window, of moving between cards and of the card in focus" width="49%">
  <img src="docs/assets/tasks-pending-settings.png" alt="The Settings dialog: the sources first, each with its board, status and last fetch, then the board filter" width="49%">
</p>

<p align="center">
  <img src="docs/assets/tasks-pending-snoozed.png" alt="The Snoozed dialog: a snoozed card with a Wake button" width="49%">
  <img src="docs/assets/tasks-pending-sandbox-light.png" alt="The same window in the light scheme" width="49%">
</p>

<p align="center">
  <img src="docs/assets/tasks-pending-sandbox.png" alt="The window on its own, tiled as the launcher opens it: a clock and weather, and the Activity tab with the picked cards in three stages, Picked, In Progress and Done / to mark" width="49%">
  <img src="docs/assets/tasks-pending-omarchy.png" alt="The same window following an Omarchy theme: its background, text and accent, the rest mixed from them" width="49%">
</p>

The window is dark unless the system asks for light, with its own palette either way, or follows Omarchy's theme when Settings says so.

## Install

Install the latest release and start everything:

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/brunoomariano/TasksPending/releases/latest/download/tasks-pending-install.sh | sh
```

On Arch Linux (and distributions built on it, such as Omarchy) it installs the release's package; elsewhere on Linux x86_64 it unpacks the release under `~/.local/lib/tasks-pending` and links `~/.local/bin/tasks-pending`. Every download is checked against the checksum published beside it, which proves it arrived whole. It ends with `tasks-pending setup`, which creates the config files if they are missing, installs the launcher entry and starts the user service. To update, run the same command again. Outside Arch, `tasks-pending upgrade` does the same; the Arch package is pacman's, so there `upgrade` prints this command instead. [docs/install.md](docs/install.md) covers sources, tokens, the Omarchy shortcut and manual installs.

Then open **TasksPending** from the launcher, or run `tasks-pending`.

## Platform

Linux x86_64, with Arch Linux and Omarchy as the operational target. The window is GTK4 and libadwaita (4.14 and 1.5 or newer).

## Quick start (development)

```sh
make doctor           # Rust, pkg-config, gtk4 and libadwaita development files
make bootstrap
make ci-check
make sandbox          # the window over simulated cards
```

Run the daemon and the window over it:

```sh
make up               # the daemon on 127.0.0.1:61000
make app              # the window; without a daemon it runs the sources itself
curl http://127.0.0.1:61000/healthz
```

Configure sources by copying `config.example.toml` to `~/.config/tasks-pending/config.toml`; without it the daemon serves a built-in sample source. See [docs/operations.md](docs/operations.md).

`make deploy` builds a release, installs it under `~/.local` and restarts the daemon on this machine.

## Repository shape

- `crates/pending-core`: shared domain model, config types, source contracts and release helpers.
- `crates/pending-runtime`: refresh scheduling, the in-memory source state, marks, snoozes and looks.
- `crates/pending-http`: HTTP helpers shared by the source adapters.
- `crates/pending-github`, `pending-gitlab`, `pending-jira`, `pending-linear`, `pending-plane`, `pending-google`, `pending-ical`, `pending-todoist`: source adapters.
- `crates/pending-api`: the daemon's local API, the weather beside the clock and the daily release check.
- `crates/pending-client`: the window's side of that API.
- `crates/pending-app`: the GTK4 window.
- `crates/tasks-pending`: the executable: `app`, `serve`, `sandbox`, `setup` and `upgrade`.
- `packaging`: desktop entry, icons, systemd unit and the Arch `PKGBUILD`.
- `docs`: architecture and operation notes.

## Current state

The source contract (`PendingSource`) and the snapshot rules live in `pending-core`; see [docs/architecture.md](docs/architecture.md). The daemon loads sources from the config file and refreshes them through `pending-runtime`. Each source is a group of stacks, and each stack is a filter declared in the config; groups and stacks follow the file order, and boards (Work, Personal, …) filter the window. Sources available: `github` (search queries), `gitlab` (merge requests, issues and to-dos), `jira` (JQL queries), `linear` (issue filters), `plane` (work item filters), `google` (Google Calendar via GNOME Online Accounts), `ical` (any iCal feed), `todoist` (Todoist filters) and the built-in `sample`. When no daemon is running, the window starts the same runtime inside itself.

## Inspiration

- [`akitaonrails/ghpending`](https://github.com/akitaonrails/ghpending): GitHub pending issues, pull requests, alerts, token fallback, and terminal digest.
- [`akitaonrails/clock-tui`](https://github.com/akitaonrails/clock-tui): the block clock, independent refresh widgets, config-first operation, and release packaging.
- [`akitaonrails/ai-memory`](https://github.com/akitaonrails/ai-memory): the install and upgrade flow (checksummed release bundles, a versioned install directory, `setup` and `upgrade`).
