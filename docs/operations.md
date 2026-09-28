# Operations

## Local Commands

- API: `make up`
- API + web dashboard: `make serve`
- TUI: `make tui`
- Frontend: `make frontend-dev`
- Verification: `make ci-check`

## Source Cache

After every successful refresh, each source's cards are saved to `$XDG_STATE_HOME/tasks-pending/cache.json` (default `~/.local/state/tasks-pending/cache.json`). A restart shows them immediately, marked as data from the previous run, while sources refresh. Delete the file to start empty; a corrupt file is ignored and rewritten.

## TUI

`pending-tui` runs the same aggregator as the API in-process, so it works without the API running. It reads the same config (`--config` or the default locations) and redraws every 250 ms.

Keys: `j`/`k` or arrows move the selection (lanes scroll to keep it visible), `Enter` opens the selected card's link (`xdg-open`, or `open` on macOS), `r` refreshes every source now (at most once every 10 s, to respect provider rate limits), `q`/`Esc`/`Ctrl-C` quit. The Sources panel shows one line per source with its status and failure reason; feedback and failures show in the footer.

## Web Dashboard

The frontend polls `/api/v1/snapshot` every 15 seconds while the tab is visible and redraws only when cards or source health change. It shows each source's status and failure reason. If the API stops answering after a successful load, the last cards stay on screen under a warning with the time of the last good response.

`pending-api` serves the built frontend at `/` from `--static-dir <dir>`, or from `../frontend` next to the binary when it has an `index.html` (release bundle layout). Without either it serves only the API. An explicit `--static-dir` without `index.html` fails at startup.

## HTTP Endpoints

- `GET /healthz`: process health and version.
- `GET /api/v1/snapshot`: dashboard snapshot.
- `GET /*`: built frontend, when a static directory is available.

## Configuration

The API reads a TOML file (see `config.example.toml`). The path is the first of these that is set (it does not fall through to the next one when the file is missing):

1. `--config <path>`;
2. `$TASKS_PENDING_CONFIG`;
3. `$XDG_CONFIG_HOME/tasks-pending/config.toml`;
4. `~/.config/tasks-pending/config.toml`.

An explicit path (1 or 2) that does not exist is an error. When no file exists at the default location (3 or 4), the API logs a warning and serves the built-in `sample` source. Syntax errors, unknown keys (top level or inside a source), duplicate source names and zero intervals fail at startup, naming the file. Empty variables and relative `XDG_CONFIG_HOME`/`HOME` values are ignored.

Tokens are read from the environment by each source, never from the config file.

## GitHub Source

`kind = "github"` searches the authenticated user's open work: review requests (section "Review requested", warning severity), open pull requests ("My pull requests") and assigned issues ("Assigned issues"). An item found by two searches shows once, in the first section.

- Token: `GITHUB_TOKEN`, then `GH_TOKEN`, then `gh auth token` (given up after 5 s; the API logs before running it, the TUI starts silently until then), resolved once at startup for all GitHub sources. Without a token the source shows as failed with a setup hint; restart after logging in.
- The three searches run in parallel, each limited to 10 s, so a hanging search becomes a warning for its section instead of failing the refresh. Keep `timeout_seconds` above 10, or the aggregator timeout fails the whole refresh first.
- One failed search keeps the other sections and makes the source degraded; all searches failing makes it failed. Rate limiting reports the reset time.
- Each search loads up to 50 items; more than that, or GitHub reporting incomplete results, shows as a warning. Draft pull requests are marked in the card.
- Error messages never include the token or request URLs.

## Observability

The API initializes `tracing_subscriber` (filter via `RUST_LOG`). Every source refresh logs the source name, duration, item and warning counts, or the failure reason. Sources must never put secrets in error messages, because those reach logs and the dashboard.
