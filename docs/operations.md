# Operations

## Local Commands

- API: `make up`
- TUI: `make tui`
- Frontend: `make frontend-dev`
- Verification: `make ci-check`

## HTTP Endpoints

- `GET /healthz`: process health and version.
- `GET /api/v1/snapshot`: dashboard snapshot.

## Configuration

The API reads a TOML file (see `config.example.toml`) from the first of:

1. `--config <path>`;
2. `$TASKS_PENDING_CONFIG`;
3. `$XDG_CONFIG_HOME/tasks-pending/config.toml`;
4. `~/.config/tasks-pending/config.toml`.

An explicit path (1 or 2) that does not exist is an error. When no file exists at the default location (3 or 4), the API logs a warning and serves the built-in `sample` source. Syntax errors, unknown top-level keys, duplicate source names and zero intervals fail at startup, naming the file.

Tokens are read from the environment by each source, never from the config file.

## Observability

The API initializes `tracing_subscriber` (filter via `RUST_LOG`). Every source refresh logs the source name, duration, item and warning counts, or the failure reason. Sources must never put secrets in error messages, because those reach logs and the dashboard.
