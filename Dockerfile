FROM oven/bun:1.3.14 AS frontend-deps

WORKDIR /app/frontend

COPY frontend/package.json frontend/bunfig.toml ./
RUN --mount=type=cache,target=/root/.cache/bun,sharing=locked \
    --mount=type=cache,target=/root/.bun/install/cache,sharing=locked \
    bun install --minimum-release-age 604800

FROM rust:1.97-trixie AS builder

WORKDIR /app

COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo build --release -p desk-foreman --bin desk-foreman --bin export-openapi \
    && cargo build --release -p desk-foreman-runner-manager --bin desk-foreman-runner-manager \
    && mkdir -p /app/dist \
    && cp /app/target/release/desk-foreman /app/dist/desk-foreman \
    && cp /app/target/release/export-openapi /app/dist/export-openapi \
    && cp /app/target/release/desk-foreman-runner-manager /app/dist/desk-foreman-runner-manager

FROM frontend-deps AS frontend-builder

COPY frontend/ .
COPY --from=builder /app/dist/export-openapi /usr/local/bin/export-openapi
RUN export-openapi openapi.json \
    && bun run gen:client \
    && bun run build

FROM debian:trixie-slim AS runtime-base

RUN apt-get update \
    && apt-get install -y --no-install-recommends bash ca-certificates curl gnupg nginx tini \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --uid 10001 desk-foreman \
    && install -d -o desk-foreman -g desk-foreman /workspace /var/lib/desk-foreman /usr/share/nginx/html

WORKDIR /workspace

RUN install -d /etc/apt/keyrings \
    && curl -fsSL https://download.docker.com/linux/debian/gpg -o /etc/apt/keyrings/docker.asc \
    && chmod a+r /etc/apt/keyrings/docker.asc \
    && . /etc/os-release \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/debian ${VERSION_CODENAME} stable" > /etc/apt/sources.list.d/docker.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends docker-ce-cli \
    && rm -rf /var/lib/apt/lists/*

FROM runtime-base AS runtime

COPY docker/nginx.conf /etc/nginx/nginx.conf
COPY docker/start-desk-foreman.sh /usr/local/bin/start-desk-foreman
COPY --from=builder /app/dist/desk-foreman /usr/local/bin/desk-foreman
COPY --from=frontend-builder /app/frontend/dist /usr/share/nginx/html

RUN chmod +x /usr/local/bin/start-desk-foreman \
    && chown -R desk-foreman:desk-foreman /usr/share/nginx/html /var/lib/desk-foreman

USER desk-foreman

ENV MCP_BIND_ADDR=127.0.0.1:3000
ENV WORKSPACE_ROOT=/workspace
ENV DEFAULT_SHELL=/bin/bash
ENV SESSION_IDLE_TTL_SEC=1800
ENV WEB_SESSION_TTL_SEC=604800
ENV WEB_COOKIE_NAME=desk_foreman_session
ENV WEB_COOKIE_SECURE=false
ENV MAX_OUTPUT_BYTES=262144
ENV PATCH_SURFACE_MODE=function_compat
ENV FRONTEND_DIST=/usr/share/nginx/html

EXPOSE 80

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/start-desk-foreman"]

FROM runtime-base AS runner-manager

COPY --from=builder /app/dist/desk-foreman-runner-manager /usr/local/bin/desk-foreman-runner-manager

USER desk-foreman

ENV RUNNER_MANAGER_BIND_ADDR=0.0.0.0:3001
ENV WORKSPACE_ROOT=/workspace
ENV RUNNER_WORKDIR=/workspace
ENV RUNNER_NETWORK_ENABLED=false
ENV RUNNER_MAX_OUTPUT_BYTES=262144
ENV RUNNER_MAX_SESSIONS=32
ENV RUNNER_PIDS_LIMIT=256
ENV RUNNER_MEMORY_LIMIT=1g
ENV RUNNER_CPU_LIMIT=2
ENV RUNNER_IDLE_TTL_SEC=1800
ENV DOCKER_CLI=docker

EXPOSE 3001

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/desk-foreman-runner-manager"]

FROM debian:trixie-slim AS workspace-runner

ARG TARGETARCH

RUN apt-get update \
    && apt-get install -y --no-install-recommends bash ca-certificates curl fd-find git python3 python3-venv ripgrep unzip \
    && ln -s /usr/bin/fdfind /usr/local/bin/fd \
    && rm -rf /var/lib/apt/lists/*

RUN case "${TARGETARCH:-amd64}" in \
        amd64) UV_ARCH="x86_64-unknown-linux-gnu"; BUN_ARCH="x64"; RUST_ARCH="x86_64-unknown-linux-gnu" ;; \
        arm64) UV_ARCH="aarch64-unknown-linux-gnu"; BUN_ARCH="aarch64"; RUST_ARCH="aarch64-unknown-linux-gnu" ;; \
        *) echo "unsupported TARGETARCH: ${TARGETARCH:-unknown}" >&2; exit 1 ;; \
    esac \
    && curl -fsSL "https://github.com/astral-sh/uv/releases/latest/download/uv-${UV_ARCH}.tar.gz" \
    | tar -xz -C /tmp \
    && install /tmp/uv-*/uv /usr/local/bin/uv \
    && install /tmp/uv-*/uvx /usr/local/bin/uvx \
    && rm -rf /tmp/uv-* \
    && curl -fsSL "https://github.com/oven-sh/bun/releases/latest/download/bun-linux-${BUN_ARCH}.zip" -o /tmp/bun.zip \
    && unzip /tmp/bun.zip -d /tmp \
    && install /tmp/bun-linux-${BUN_ARCH}/bun /usr/local/bin/bun \
    && rm -rf /tmp/bun.zip /tmp/bun-linux-${BUN_ARCH} \
    && curl -fsSL "https://static.rust-lang.org/rustup/dist/${RUST_ARCH}/rustup-init" -o /tmp/rustup-init \
    && chmod +x /tmp/rustup-init \
    && RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo /tmp/rustup-init -y --profile minimal --default-toolchain stable --no-modify-path \
    && rm -rf /tmp/rustup-init

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
