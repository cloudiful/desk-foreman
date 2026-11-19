FROM dockerhub.cloud1ful.com/library/debian:trixie-slim AS workspace-runner

COPY docker/apt-cloud1ful-insecure.conf /etc/apt/apt.conf.d/99apt-cloud1ful-insecure
COPY docker/debian.sources /etc/apt/sources.list.d/debian.sources

RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt/lists,sharing=locked \
    apt-get update \
    && apt-get install -y --no-install-recommends bash ca-certificates curl fd-find git python3 python3-venv ripgrep unzip \
    && ln -s /usr/bin/fdfind /usr/local/bin/fd \
    && git config --system user.name "Desk Foreman" \
    && git config --system user.email "desk-foreman@cloud1ful.com" \
    && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_DIST_SERVER=https://rustup.cloud1ful.com
ENV RUSTUP_UPDATE_ROOT=https://rustup.cloud1ful.com/rustup
ENV BUN_CONFIG_REGISTRY=https://npm.cloud1ful.com
ENV UV_DEFAULT_INDEX=https://pypi.cloud1ful.com

RUN git config --system \
    url."https://github.cloud1ful.com/".insteadOf \
    https://github.com/ \
    && curl -fsSL "https://github.cloud1ful.com/astral-sh/uv/releases/latest/download/uv-x86_64-unknown-linux-gnu.tar.gz" \
    | tar -xz -C /tmp \
    && install /tmp/uv-*/uv /usr/local/bin/uv \
    && install /tmp/uv-*/uvx /usr/local/bin/uvx \
    && rm -rf /tmp/uv-* \
    && curl -fsSL "https://github.cloud1ful.com/oven-sh/bun/releases/latest/download/bun-linux-x64.zip" -o /tmp/bun.zip \
    && unzip /tmp/bun.zip -d /tmp \
    && install /tmp/bun-linux-x64/bun /usr/local/bin/bun \
    && rm -rf /tmp/bun.zip /tmp/bun-linux-x64 \
    && curl --proto '=https' --tlsv1.2 -fsSL https://rustup.cloud1ful.com \
    | RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo sh -s -- -y --profile minimal --default-toolchain stable --no-modify-path

# /etc/profile resets PATH for login shells; keep the toolchain visible.
RUN printf 'export PATH="/usr/local/cargo/bin:/usr/local/bin:${PATH}"\n' > /etc/profile.d/desk-foreman.sh

WORKDIR /workspace

# The container root filesystem is read-only at runtime; the Rust toolchain
# lives in image layers while cargo's writable home lives on the workspace
# mount. Runner containers are started with --user matching the workspace
# directory owner (see runner-manager docker backend).
ENV RUSTUP_HOME=/usr/local/rustup
ENV CARGO_HOME=/workspace/.cargo-home
ENV HOME=/workspace
ENV PATH=/usr/local/cargo/bin:/usr/local/bin:/usr/bin:/bin

ENTRYPOINT ["/bin/bash", "-lc", "sleep infinity"]
