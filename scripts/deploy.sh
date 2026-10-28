#!/usr/bin/env bash
# Deploys this checkout to this machine, the same way every time: builds a
# release, installs it under PREFIX (default ~/.local) and hands over to
# `tasks-pending setup`, which starts or restarts the daemon and fails
# unless it answers as this build. Safe to run again; config and tokens are
# left alone.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"

step() { printf '\033[36m==>\033[0m %s\n' "$*"; }

step "building a release"
cargo build --release --locked --manifest-path "$ROOT/Cargo.toml" -p tasks-pending

step "installing under $PREFIX and (re)starting the daemon"
PREFIX="$PREFIX" SKIP_BUILD=1 "$ROOT/scripts/install.sh" --start

step "deployed $("$PREFIX/bin/tasks-pending" --version)"
