# Repository Layers

TasksPending is organized around one shared model and several delivery surfaces.

## Layers

- `pending-core`: domain objects, config, refresh/source contracts, and testable invariants.
- `pending-http`: HTTP helpers shared by source adapters (error text without URLs, the https check for addresses that receive a token, the time budget of a refresh).
- `pending-todoist`: Todoist source adapter.
- `pending-google`: Google Calendar source adapter (GNOME Online Accounts).
- `pending-ical`: iCal/Google Calendar source adapter.
- `pending-plane`: Plane source adapter.
- `pending-jira`: Jira source adapter (Cloud and Data Center).
- `pending-gitlab`: GitLab source adapter (gitlab.com and self-managed).
- `pending-linear`: Linear source adapter.
- `pending-github`: GitHub source adapter; provider details stay here.
- `pending-runtime`: refresh scheduling and the in-memory state of every source.
- `pending-api`: HTTP routes, logging, and embedded frontend delivery.
- `pending-tui`: terminal rendering and keyboard interaction.
- `tasks-pending`: released CLI that starts either delivery surface.
- `frontend`: browser rendering over the API contract.

## Boundary Rule

Provider-specific code should enter through source adapters and leave as core model values. UI code should not know whether a card came from GitHub, a command, a local file, or another future source.

## Verification

The local verification gate is:

```sh
make ci-check
```

It checks Rust formatting, Clippy, Rust tests (including the TypeScript contract drift check), TypeScript type checking, frontend tests, and frontend build.
