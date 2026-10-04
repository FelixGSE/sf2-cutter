#!/usr/bin/env bash
# Runs once after the container is created, as the unprivileged user.
set -euo pipefail

rustc --version
cargo --version
gh --version | head -n 1

# Claude Code via the official installer: user-local (~/.local, volume-backed),
# so `claude update` works without sudo and survives container rebuilds.
if ! command -v claude >/dev/null 2>&1; then
    curl -fsSL https://claude.ai/install.sh | bash
fi
claude --version

# Let git use gh's credentials (GH_TOKEN or `gh auth login`) for HTTPS remotes.
if [[ -n "${GH_TOKEN:-}" ]] || gh auth status >/dev/null 2>&1; then
    gh auth setup-git
    # The container has no SSH keys; rewrite SSH remotes to HTTPS so the
    # token-based credential helper handles them (container-global only).
    git config --global url."https://github.com/".insteadOf "git@github.com:"
else
    echo "==> gh: not authenticated (set GH_TOKEN on the host or run 'gh auth login')"
fi

# Warm the build cache so the first edit/compile is fast.
cargo fetch
cargo build
