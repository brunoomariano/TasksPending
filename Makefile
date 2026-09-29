.DEFAULT_GOAL := help

.PHONY: help bootstrap doctor ci ci-check test fmt lint build up serve down logs tui contract frontend-dev frontend-build frontend-test release-local install uninstall webapp

help: ## Show this list of targets
	@awk 'BEGIN {FS = ":.*## "} /^[a-zA-Z_-]+:.*## / {printf "  \033[36m%-16s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)

bootstrap: ## Fetch Rust and frontend dependencies
	cargo fetch --locked
	npm --prefix frontend install

doctor: ## Check that the required tools are installed
	@command -v rustc >/dev/null || (echo "missing rustc" && exit 1)
	@command -v cargo >/dev/null || (echo "missing cargo" && exit 1)
	@command -v node >/dev/null || (echo "missing node" && exit 1)
	@command -v npm >/dev/null || (echo "missing npm" && exit 1)
	@rustc --version
	@cargo --version
	@node --version
	@npm --version

ci: fmt ci-check ## Format, then run the verification gate

ci-check: lint test frontend-test frontend-build ## Verification gate: lint, tests and frontend build (read-only)

fmt: ## Format Rust and frontend code
	cargo fmt --all
	npm --prefix frontend run format

lint: ## Check formatting, clippy and frontend types
	cargo fmt --all --check
	cargo clippy --workspace --all-targets --locked -- -D warnings
	npm --prefix frontend run typecheck

test: ## Run the Rust tests
	cargo test --workspace --locked

build: ## Build the workspace (debug)
	cargo build --workspace --locked

up: ## Run the API on 127.0.0.1:8080
	cargo run -p tasks-pending -- serve --listen 127.0.0.1:8080

serve: frontend-build ## Build the frontend and run the API serving it on 127.0.0.1:8080
	cargo run -p tasks-pending -- serve --listen 127.0.0.1:8080 --static-dir frontend/dist

down: ## Placeholder: no background services are managed
	@echo "No background services are managed yet."

logs: ## Placeholder: the API logs to stdout
	@echo "The API logs to stdout while make up is running."

tui: ## Run the TUI
	cargo run -p tasks-pending -- tui

contract: ## Regenerate frontend/src/contract.gen.ts from the core model
	UPDATE_CONTRACT=1 cargo test -p pending-core --test contract --locked

frontend-dev: ## Run the frontend dev server
	npm --prefix frontend run dev

frontend-build: ## Build the frontend
	npm --prefix frontend run build

frontend-test: ## Run the frontend tests
	npm --prefix frontend test

release-local: ## Build a release bundle for this machine under dist/
	./scripts/release-local.sh

# Build a release and install it under PREFIX (default ~/.local), plus the
# user service (systemd on Linux, launchd on macOS). Config and tokens are
# left alone; see docs/install.md.
install: ## Build a release and install it under PREFIX (default ~/.local)
	./scripts/install.sh

uninstall: ## Remove what install put in place (your config stays)
	./scripts/install.sh --uninstall

# Omarchy: a launcher that opens the dashboard as a web app.
webapp: ## Omarchy: create the web app launcher for the dashboard
	./scripts/omarchy-webapp.sh
