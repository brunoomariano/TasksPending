# Repository Layers

TasksPending is organized around one shared model and several delivery surfaces.

## Layers

- `pending-core`: domain objects, config, refresh/source contracts, and testable invariants.
- `pending-api`: HTTP routes, process startup, logging, and future static frontend serving.
- `pending-tui`: terminal rendering and keyboard interaction.
- `frontend`: browser rendering over the API contract.

## Boundary Rule

Provider-specific code should enter through source adapters and leave as core model values. UI code should not know whether a card came from GitHub, a command, a local file, or another future source.

## Verification

The local verification gate is:

```sh
make ci-check
```

It checks Rust formatting, Clippy, Rust tests, TypeScript type checking, and frontend build.
