# Manta AI Assistant - Dockerfile
# Multi-stage build for minimal production image

# Stage 1: Build
FROM rust:1.75-slim-bookworm as builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libsqlite3-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /usr/src/manta

# Copy Cargo files first for better layer caching
COPY Cargo.toml Cargo.lock ./

# Copy source code
COPY src ./src

# Build release binary
RUN cargo build --release --locked

# Stage 2: Runtime
FROM debian:bookworm-slim as runtime

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libsqlite3-0 \
    python3 \
    python3-pip \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -m -u 1000 -s /bin/bash manta

# Set up working directory
WORKDIR /app

# Copy binary from builder
COPY --from=builder /usr/src/manta/target/release/manta /usr/local/bin/manta

# Copy example skills
COPY --from=builder /usr/src/manta/examples/skills /app/examples/skills

# Create data directory
RUN mkdir -p /app/data /app/.config/manta/skills && \
    chown -R manta:manta /app

# Switch to non-root user
USER manta

# Set environment variables
ENV MANTA_CONFIG_DIR=/app/.config/manta
ENV MANTA_DATA_DIR=/app/data

# Expose no ports by default (add if needed for HTTP API)
# EXPOSE 8080

# Default command
CMD ["manta", "chat"]

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD manta --version || exit 1
