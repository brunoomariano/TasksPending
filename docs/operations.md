# Operations

## Local Commands

- API: `make up`
- TUI: `make tui`
- Frontend: `make frontend-dev`
- Verification: `make ci-check`

## TUI

`pending-tui` runs the same aggregator as the API in-process, so it works without the API running. It reads the same config (`--config` or the default locations) and redraws every 250 ms.

Keys: `j`/`k` or arrows move the selection, `Enter` opens the selected card's link (`xdg-open`, or `open` on macOS), `r` refreshes every source now, `q`/`Esc`/`Ctrl-C` quit. The Sources panel shows each source's status and failure reason; action failures show in the footer.

## HTTP Endpoints

- `GET /healthz`: process health and version.
- `GET /api/v1/snapshot`: dashboard snapshot.

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

- Token: `GITHUB_TOKEN`, then `GH_TOKEN`, then `gh auth token` (logged before it runs, given up after 5 s), resolved once at startup for all GitHub sources. Without a token the source shows as failed with a setup hint; restart after logging in.
- The three searches run in parallel, each limited to 10 s, so a hanging search becomes a warning for its section instead of failing the refresh.
- One failed search keeps the other sections and makes the source degraded; all searches failing makes it failed. Rate limiting reports the reset time.
- Each search loads up to 50 items; more than that, or GitHub reporting incomplete results, shows as a warning. Draft pull requests are marked in the card.
- Error messages never include the token or request URLs.

## Observability

The API initializes `tracing_subscriber` (filter via `RUST_LOG`). Every source refresh logs the source name, duration, item and warning counts, or the failure reason. Sources must never put secrets in error messages, because those reach logs and the dashboard.
