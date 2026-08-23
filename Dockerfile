# Build stage: compile the binary for the target platform.
#
# TARGETPLATFORM comes from BuildKit (e.g. "linux/arm64") and is NOT a valid
# rustc triple; map it explicitly. Copy the result to `/out/` so the runtime
# stage does not hardcode aarch64 vs amd64 paths.
FROM --platform=$BUILDPLATFORM rust:1-bookworm AS builder
ARG TARGETPLATFORM=linux/arm64
WORKDIR /app
COPY . .
RUN case "$TARGETPLATFORM" in \
      linux/arm64)  TARGET=aarch64-unknown-linux-gnu ;; \
      linux/amd64)  TARGET=x86_64-unknown-linux-gnu ;; \
      *) echo "unsupported TARGETPLATFORM: $TARGETPLATFORM" && exit 1 ;; \
    esac && \
    echo "Building for $TARGET" && \
    rustup target add "$TARGET" && \
    cargo build --release --target "$TARGET" && \
    mkdir -p /out && \
    cp "target/${TARGET}/release/reddit-rss-mcp" /out/reddit-rss-mcp

# Runtime stage: minimal image, Streamable HTTP transport.
#
# NOTE: when binding to 0.0.0.0, REDDIT_RSS_MCP_KEY must be set at runtime,
# otherwise the server refuses to start. The /health endpoint never requires
# the key, so the HEALTHCHECK below works without it. The healthcheck does
# NOT call Reddit.
FROM debian:bookworm-slim
RUN apt-get update && \
    apt-get install -y --no-install-recommends curl && \
    rm -rf /var/lib/apt/lists/*
RUN useradd --create-home mcp
USER mcp
COPY --from=builder /out/reddit-rss-mcp /usr/local/bin/reddit-rss-mcp
EXPOSE 8085
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s \
  CMD curl -fsS http://127.0.0.1:8085/health || exit 1
CMD ["reddit-rss-mcp", "--http", "--bind", "0.0.0.0:8085"]
