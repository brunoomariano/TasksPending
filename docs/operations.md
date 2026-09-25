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

The bootstrap has config structs in `pending-core`, but no config file is loaded yet. The expected direction is an XDG config file plus environment overrides for tokens and one-shot source lists.

## Observability

The API initializes `tracing_subscriber` and logs startup information. Future source refresh jobs should include source name, refresh duration, item count, and failure reason without logging secrets.
