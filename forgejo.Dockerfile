FROM dockerhub.cloud1ful.com/library/debian:trixie-slim AS runtime-base

COPY docker/apt-cloud1ful-insecure.conf /etc/apt/apt.conf.d/99apt-cloud1ful-insecure
COPY docker/debian.sources /etc/apt/sources.list.d/debian.sources

RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt/lists,sharing=locked \
    apt-get update \
    && apt-get install -y --no-install-recommends bash ca-certificates curl gnupg nginx tini \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --uid 10001 desk-foreman \
    && install -d -o desk-foreman -g desk-foreman /workspace /var/lib/desk-foreman /usr/share/nginx/html

WORKDIR /workspace

COPY docker/docker.asc /etc/apt/keyrings/docker.asc
COPY docker/docker.sources /etc/apt/sources.list.d/docker.sources

RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt/lists,sharing=locked \
    chmod a+r /etc/apt/keyrings/docker.asc \
    && apt-get update \
    && apt-get install -y --no-install-recommends docker-ce-cli \
    && rm -rf /var/lib/apt/lists/*

FROM runtime-base AS runtime

COPY docker/nginx.conf /etc/nginx/nginx.conf
COPY docker/start-desk-foreman.sh /usr/local/bin/start-desk-foreman
COPY --chmod=755 ci-image-input/desk-foreman /usr/local/bin/desk-foreman
COPY ci-image-input/frontend-dist /usr/share/nginx/html

RUN chmod +x /usr/local/bin/start-desk-foreman \
    && chown -R desk-foreman:desk-foreman /usr/share/nginx/html /var/lib/desk-foreman

USER desk-foreman

ENV MCP_BIND_ADDR=0.0.0.0:3000
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

COPY --chmod=755 ci-image-input/desk-foreman-runner-manager /usr/local/bin/desk-foreman-runner-manager

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

FROM dockerhub.cloud1ful.com/library/debian:trixie-slim AS workspace-runner

COPY docker/apt-cloud1ful-insecure.conf /etc/apt/apt.conf.d/99apt-cloud1ful-insecure
COPY docker/debian.sources /etc/apt/sources.list.d/debian.sources

RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt/lists,sharing=locked \
    apt-get update \
    && apt-get install -y --no-install-recommends bash ca-certificates curl fd-find git python3 python3-venv ripgrep unzip \
    && ln -s /usr/bin/fdfind /usr/local/bin/fd \
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
