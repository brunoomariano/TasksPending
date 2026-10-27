# Repository Layers

TasksPending is organized around one shared model, a daemon that keeps it fresh, and a window that shows it.

## Layers

- `pending-core`: domain objects, config, refresh/source contracts, release helpers, and testable invariants.
- `pending-http`: HTTP helpers shared by source adapters (error text without URLs, the https check for addresses that receive a token, the time budget of a refresh).
- `pending-todoist`: Todoist source adapter.
- `pending-google`: Google Calendar source adapter (GNOME Online Accounts).
- `pending-ical`: iCal/Google Calendar source adapter.
- `pending-plane`: Plane source adapter.
- `pending-jira`: Jira source adapter (Cloud and Data Center).
- `pending-gitlab`: GitLab source adapter (gitlab.com and self-managed).
- `pending-linear`: Linear source adapter.
- `pending-github`: GitHub source adapter; provider details stay here.
- `pending-runtime`: refresh scheduling, the in-memory state of every source, and the marks, snoozes and looks the user makes.
- `pending-api`: the daemon's HTTP routes, logging, the weather beside the clock and the daily release check.
- `pending-client`: the window's calls to those routes, over loopback only.
- `pending-app`: the GTK4 window: a page model worked out without widgets, and the widgets that show it.
- `tasks-pending`: the executable: the window (`app`, the default), the daemon (`serve`), the sandbox, `setup` and `upgrade`.

## Boundary Rule

Provider-specific code should enter through source adapters and leave as core model values. UI code should not know whether a card came from GitHub, a command, a local file, or another future source. The window talks to the daemon only through `pending-client`; when it has to run the sources itself, it starts the same API on a private loopback port rather than reaching into the runtime.

## Verification

The local verification gate is:

```sh
make ci-check
```

It checks Rust formatting, Clippy, the Rust tests, the changelog's sections, and the install scripts (the source installer, the public release installer and the release asset aliases) against fakes.
