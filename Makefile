.DEFAULT_GOAL := ci-check

.PHONY: bootstrap doctor ci ci-check test fmt lint build up serve down logs tui contract frontend-dev frontend-build frontend-test release-local install uninstall webapp

bootstrap:
	cargo fetch --locked
	npm --prefix frontend install

doctor:
	@command -v rustc >/dev/null || (echo "missing rustc" && exit 1)
	@command -v cargo >/dev/null || (echo "missing cargo" && exit 1)
	@command -v node >/dev/null || (echo "missing node" && exit 1)
	@command -v npm >/dev/null || (echo "missing npm" && exit 1)
	@rustc --version
	@cargo --version
	@node --version
	@npm --version

ci: fmt ci-check

ci-check: lint test frontend-test frontend-build

fmt:
	cargo fmt --all
	npm --prefix frontend run format

lint:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets --locked -- -D warnings
	npm --prefix frontend run typecheck

test:
	cargo test --workspace --locked

build:
	cargo build --workspace --locked

up:
	cargo run -p pending-api -- --listen 127.0.0.1:8080

serve: frontend-build
	cargo run -p pending-api -- --listen 127.0.0.1:8080 --static-dir frontend/dist

down:
	@echo "No background services are managed yet."

logs:
	@echo "The API logs to stdout while make up is running."

tui:
	cargo run -p pending-tui

contract:
	UPDATE_CONTRACT=1 cargo test -p pending-core --test contract --locked

frontend-dev:
	npm --prefix frontend run dev

frontend-build:
	npm --prefix frontend run build

frontend-test:
	npm --prefix frontend test

release-local:
	./scripts/release-local.sh

# Build a release and install it under PREFIX (default ~/.local), plus the
# user service (systemd on Linux, launchd on macOS). Config and tokens are
# left alone; see docs/install.md.
install:
	./scripts/install.sh

uninstall:
	./scripts/install.sh --uninstall

# Omarchy: a launcher that opens the dashboard as a web app.
webapp:
	./scripts/omarchy-webapp.sh
