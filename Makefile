.DEFAULT_GOAL := ci-check

.PHONY: bootstrap doctor ci ci-check test fmt lint build up down logs tui frontend-dev frontend-build release-local

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

ci-check: lint test frontend-build

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

down:
	@echo "No background services are managed yet."

logs:
	@echo "The API logs to stdout while make up is running."

tui:
	cargo run -p pending-tui

frontend-dev:
	npm --prefix frontend run dev

frontend-build:
	npm --prefix frontend run build

release-local:
	./scripts/release-local.sh
