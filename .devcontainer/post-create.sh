#!/usr/bin/env bash
# Runs once after the container is created, as the unprivileged user.
set -euo pipefail

rustc --version
cargo --version
gh --version | head -n 1
claude --version

# Let git use gh's credentials (GH_TOKEN or `gh auth login`) for HTTPS remotes.
if [[ -n "${GH_TOKEN:-}" ]] || gh auth status >/dev/null 2>&1; then
    gh auth setup-git
else
    echo "==> gh: not authenticated (set GH_TOKEN on the host or run 'gh auth login')"
fi

# Warm the build cache so the first edit/compile is fast.
cargo fetch
cargo build
