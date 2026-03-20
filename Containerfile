# NexiBot Containerfile (Podman / OCI)
# Builds a rootless server image for NexiBot headless deployment.
#
# Build:
#   podman build -t nexibot:latest -f Containerfile .
#
# Run:
#   podman run --rm -p 18790:18790 -p 18791:18791 -p 18792:18792 \
#     -e NEXIBOT_HEADLESS=1 \
#     -e ANTHROPIC_API_KEY=sk-ant-... \
#     -v nexibot-config:/home/nexibot/.config/ai \
#     nexibot:latest
#
# Podman compose:
#   podman compose -f compose.yaml up -d
#
# Ports:
#   18790  Anthropic bridge (Node.js SDK proxy)
#   18791  Webhook / HTTP REST API
#   18792  Gateway WebSocket (enable in config: gateway.enabled=true, gateway.port=18792)

# ============================================================================
# Stage 1: Dependency planner (cargo-chef for layer caching)
# ============================================================================
FROM docker.io/rust:1.81-bookworm AS chef
RUN cargo install cargo-chef --locked
WORKDIR /build

# ============================================================================
# Stage 2: Build recipe (captures dependency manifest without source)
# ============================================================================
FROM chef AS planner
COPY src-tauri/Cargo.toml src-tauri/Cargo.lock ./src-tauri/
COPY ../k2k-common/ k2k-common/ 2>/dev/null || true
WORKDIR /build/src-tauri
RUN cargo chef prepare --recipe-path recipe.json

# ============================================================================
# Stage 3: Build dependencies (cached as a separate layer)
# ============================================================================
FROM chef AS dependency-builder

# Tauri 2.x Linux build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    libclang-dev \
    cmake \
    libgtk-3-dev \
    libwebkit2gtk-4.1-dev \
    libayatana-appindicator3-dev \
    librsvg2-dev \
    libxdo-dev \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build/src-tauri
COPY --from=planner /build/src-tauri/recipe.json recipe.json

# Build only dependencies (this layer is cached until Cargo.toml changes)
RUN cargo chef cook --release --recipe-path recipe.json

# ============================================================================
# Stage 4: Build the application
# ============================================================================
FROM dependency-builder AS app-builder

# Copy source
COPY src-tauri/ /build/src-tauri/
COPY k2k-common/ /build/k2k-common/ 2>/dev/null || mkdir -p /build/k2k-common

WORKDIR /build/src-tauri

# Build the release binary
# NEXIBOT_HEADLESS is a runtime env var — no compile-time feature needed.
RUN cargo build --release

# ============================================================================
# Stage 5: Build the Node.js Anthropic bridge
# ============================================================================
FROM docker.io/node:20-slim AS bridge-builder

WORKDIR /build/bridge
COPY anthropic-bridge/package.json anthropic-bridge/package-lock.json* ./
RUN npm ci --omit=dev

COPY anthropic-bridge/ .

# ============================================================================
# Stage 6: Runtime image (rootless, non-root user)
# ============================================================================
FROM docker.io/debian:bookworm-slim

LABEL org.opencontainers.image.title="NexiBot Server"
LABEL org.opencontainers.image.description="NexiBot AI assistant — headless server mode"
LABEL org.opencontainers.image.source="https://github.com/jaredcluff/nexibot"

# Runtime dependencies (only what the binary dynamically links against)
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    nodejs \
    libgtk-3-0 \
    libwebkit2gtk-4.1-0 \
    libayatana-appindicator3-1 \
    librsvg2-2 \
    libxdo3 \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user for rootless operation (Podman default)
RUN groupadd --gid 1000 nexibot && \
    useradd --uid 1000 --gid nexibot --shell /bin/sh --create-home nexibot

WORKDIR /app

# Copy binary
COPY --from=app-builder /build/src-tauri/target/release/nexibot-tauri /app/nexibot

# Copy Anthropic bridge
COPY --from=bridge-builder /build/bridge /app/bridge

# Copy bundled skill resources
COPY src-tauri/resources/bundled-skills /app/bundled-skills

# Own everything to the nexibot user
RUN chown -R nexibot:nexibot /app

# Switch to non-root user
USER nexibot

# Config, skills, memory, and model data directories
# These should be mounted as volumes in production.
RUN mkdir -p \
    /home/nexibot/.config/nexibot \
    /home/nexibot/.config/nexibot/skills \
    /home/nexibot/.config/nexibot/memory \
    /home/nexibot/.config/nexibot/models

# ── Environment variables ──────────────────────────────────────────────────
# Required: at least one LLM API key
ENV ANTHROPIC_API_KEY=""
ENV OPENAI_API_KEY=""

# Optional: channel integrations
ENV TELEGRAM_BOT_TOKEN=""
ENV DISCORD_BOT_TOKEN=""
ENV SLACK_BOT_TOKEN=""
ENV WHATSAPP_PHONE_NUMBER_ID=""
ENV WHATSAPP_ACCESS_TOKEN=""
ENV WHATSAPP_VERIFY_TOKEN=""

# Runtime mode (must be set — tells NexiBot to skip the Tauri GUI)
ENV NEXIBOT_HEADLESS="1"

# Logging
ENV RUST_LOG="nexibot_tauri=info,warn"

# Bridge port (Anthropic Node.js SDK proxy)
ENV BRIDGE_PORT="18790"

# ── Ports ──────────────────────────────────────────────────────────────────
# 18790  Anthropic bridge
# 18791  Webhook / HTTP REST API
# 18792  Gateway WebSocket (enable via config: gateway.enabled=true, gateway.port=18792)
EXPOSE 18790 18791 18792

# ── Health check ───────────────────────────────────────────────────────────
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD curl -f http://localhost:18791/webhook/health || exit 1

# ── Startup ────────────────────────────────────────────────────────────────
# Start the Anthropic bridge (background) then NexiBot server (foreground).
# BRIDGE_PORT must match config.yaml's bridge_url setting.
CMD ["sh", "-c", \
    "BRIDGE_PORT=${BRIDGE_PORT} node /app/bridge/index.js & \
     exec /app/nexibot"]
