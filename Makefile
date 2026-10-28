.DEFAULT_GOAL := help

.PHONY: help bootstrap doctor ci ci-check test fmt lint build up app sandbox screenshots changelog-check install-test release-installer-test release-assets-test dependabot-issues-test release-local release-smoke install uninstall deploy

help: ## Show this list of targets
	@awk 'BEGIN {FS = ":.*## "} /^[a-zA-Z_-]+:.*## / {printf "  \033[36m%-22s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)

bootstrap: ## Fetch the Rust dependencies
	cargo fetch --locked

doctor: ## Check that the required tools and libraries are installed
	@command -v rustc >/dev/null || (echo "missing rustc" && exit 1)
	@command -v cargo >/dev/null || (echo "missing cargo" && exit 1)
	@command -v pkg-config >/dev/null || (echo "missing pkg-config" && exit 1)
	@pkg-config --exists 'gtk4 >= 4.14' || (echo "missing gtk4 development files (>= 4.14)" && exit 1)
	@pkg-config --exists 'libadwaita-1 >= 1.5' || (echo "missing libadwaita development files (>= 1.5)" && exit 1)
	@rustc --version
	@cargo --version
	@echo "gtk4 $$(pkg-config --modversion gtk4), libadwaita $$(pkg-config --modversion libadwaita-1)"

ci: fmt ci-check ## Format, then run the verification gate

ci-check: lint test changelog-check install-test release-installer-test release-assets-test dependabot-issues-test ## Verification gate: lint, tests and the scripts (read-only)

fmt: ## Format the Rust code
	cargo fmt --all

lint: ## Check formatting and clippy
	cargo fmt --all --check
	cargo clippy --workspace --all-targets --locked -- -D warnings

test: ## Run the Rust tests
	cargo test --workspace --locked

build: ## Build the workspace (debug)
	cargo build --workspace --locked

up: ## Run the daemon on 127.0.0.1:61000
	cargo run -p tasks-pending -- serve --listen 127.0.0.1:61000

app: ## Open the window (over the daemon when it runs, else over the sources)
	cargo run -p tasks-pending -- app

sandbox: ## Open the window over simulated cards, without config, credentials or network
	cargo run -p tasks-pending -- sandbox

# Needs a display that draws frames; headless: `gtk4-broadwayd :5 &` then
# `GDK_BACKEND=broadway BROADWAY_DISPLAY=:5 make screenshots` (1024x768).
screenshots: ## Regenerate the README pictures from the sandbox window
	cargo build -p tasks-pending
	./target/debug/tasks-pending sandbox --screenshot docs/assets/tasks-pending-sandbox.png
	./target/debug/tasks-pending sandbox --screenshot docs/assets/tasks-pending-sandbox-light.png --scene light
	./target/debug/tasks-pending sandbox --screenshot docs/assets/tasks-pending-sources.png --scene sources
	./target/debug/tasks-pending sandbox --screenshot docs/assets/tasks-pending-settings.png --scene settings
	./target/debug/tasks-pending sandbox --screenshot docs/assets/tasks-pending-snoozed.png --scene snoozed

changelog-check: ## Check CHANGELOG.md (one section per version, newest first) and the check itself
	./scripts/test-check-changelog.sh
	./scripts/check-changelog.sh

install-test: ## Check installer behavior without changing the local system
	./scripts/test-install.sh

release-installer-test: ## Check the public release installer without network access
	./scripts/test-release-installer.sh

release-assets-test: ## Check the stable release download aliases and checksums
	./scripts/test-release-assets.sh

dependabot-issues-test: ## Check the script that keeps Dependabot alerts as issues, against a stand-in gh
	./scripts/test-dependabot-issues.sh

release-local: ## Build a release bundle for this machine under dist/
	./scripts/release-local.sh

release-smoke: release-local ## Build a release bundle and smoke test it
	./scripts/test-release-bundle.sh

# Build a release and install it under PREFIX (default ~/.local): the
# executable, the desktop entry, the icon and the user service. Config and
# tokens are left alone; see docs/install.md.
install: ## Build a release and install it under PREFIX (default ~/.local)
	./scripts/install.sh

uninstall: ## Remove what install put in place (your config stays)
	./scripts/install.sh --uninstall

deploy: ## Build, install under ~/.local and (re)start the daemon; fails unless it answers as this build
	./scripts/deploy.sh
