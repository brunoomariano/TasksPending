# Contributing to TasksPending

Thanks for taking the time to improve TasksPending.

## Before you start

Search existing issues and pull requests before opening a new one. For a bug, include the TasksPending version, platform, source kind, expected behavior and actual behavior. Do not include tokens, private feed URLs or source data in an issue.

## Local checks

Install the repository dependencies and run the read-only verification gate:

```sh
make bootstrap
make ci-check
```

`make ci` also formats Rust and frontend files. Add focused tests for behavioral changes. When changing a `pending-core` model with TypeScript bindings, run `make contract` and include the regenerated frontend contract.

Keep code, comments, tests and documentation in English. Source-specific API behavior belongs in its source crate; shared contracts belong in `pending-core`. See [docs/architecture.md](docs/architecture.md) for the repository layout and [docs/operations.md](docs/operations.md) for user-facing behavior.

## Pull requests

Keep pull requests scoped to one problem. Explain the behavior change, update affected documentation and report the checks you ran. The CI workflow runs the same verification gate for pull requests.
