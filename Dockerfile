# Build stage: compile the static binary.
FROM --platform=$BUILDPLATFORM rust:1-bookworm AS builder
ARG TARGETPLATFORM
WORKDIR /app
COPY . .
RUN rustup target add $(echo $TARGETPLATFORM | sed 's|linux/||') && \
    cargo build --release --target $(echo $TARGETPLATFORM | sed 's|linux/||')

# Runtime stage: minimal image, Streamable HTTP transport.
FROM debian:bookworm-slim
RUN useradd --create-home mcp
USER mcp
COPY --from=builder /app/target/*/release/reddit-rss-mcp /usr/local/bin/reddit-rss-mcp
EXPOSE 8085
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s \
  CMD wget -qO- http://127.0.0.1:8085/health || exit 1
CMD ["reddit-rss-mcp", "--http", "--bind", "0.0.0.0:8085"]
