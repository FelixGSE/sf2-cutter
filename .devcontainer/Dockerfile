# Rust development container, runs as a non-root user.
FROM rust:1-bookworm

ARG USERNAME=dev
ARG USER_UID=1000
ARG USER_GID=$USER_UID

# System deps: git, the GitHub CLI, and fluidsynth (headless smoke tests load
# extracted soundfonts through a real synthesiser).
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates curl git gnupg pkg-config libssl-dev fluidsynth \
    && install -d -m 0755 /etc/apt/keyrings \
    && curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
        -o /etc/apt/keyrings/githubcli-archive-keyring.gpg \
    && chmod go+r /etc/apt/keyrings/githubcli-archive-keyring.gpg \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
        > /etc/apt/sources.list.d/github-cli.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends gh \
    && rm -rf /var/lib/apt/lists/*

# Extra toolchain components for IDE support (as root, before user setup, so
# this layer stays cached when the user config changes).
RUN rustup component add rustfmt clippy rust-analyzer rust-src llvm-tools

# Quality-gate tooling, baked into the image: binaries land in
# /usr/local/cargo/bin, which is an image layer (only the registry is
# volume-backed), so installing here survives container rebuilds.
# Compiling the two tools adds ~10 minutes to a cold image build.
RUN cargo install --locked cargo-llvm-cov cargo-mutants cargo-fuzz

# Create the unprivileged user (no sudo). Pre-create the cargo registry dir so a
# named volume mounted there inherits dev ownership instead of root.
RUN groupadd --gid "$USER_GID" "$USERNAME" \
    && useradd --uid "$USER_UID" --gid "$USER_GID" -m -s /bin/bash "$USERNAME" \
    && mkdir -p /usr/local/cargo/registry \
    && chown -R "$USERNAME:$USERNAME" /usr/local/cargo /usr/local/rustup

# Pre-create dirs backed by named volumes so they inherit dev ownership.
# target/ lives in a volume both for speed and because rustc's incremental
# hard links fail on virtiofs workspace mounts.
RUN mkdir -p /home/$USERNAME/.claude /home/$USERNAME/.config/gh /home/$USERNAME/.local \
    && mkdir -p /workspaces/sf2-cutter/target \
    && chown -R "$USERNAME:$USERNAME" /home/$USERNAME /workspaces

USER $USERNAME
WORKDIR /workspaces
