# ── Stage 1: Build ────────────────────────────────────────────
FROM rust:1.85-slim as builder

# Install system dependencies needed to compile
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Set working directory
WORKDIR /app

# Copy Cargo files first (for dependency caching)
COPY backend/Cargo.toml backend/Cargo.lock ./

# Create a dummy main.rs to pre-compile dependencies
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release
RUN rm src/main.rs

# Copy the real source code
COPY backend/src ./src

# Build the actual application
RUN touch src/main.rs && cargo build --release

# ── Stage 2: Runtime ──────────────────────────────────────────
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    libssl3 \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the compiled binary from builder stage
COPY --from=builder /app/target/release/backend ./backend

# Copy the frontend files
COPY frontend ./frontend

# Expose port (Railway overrides this with PORT env var)
EXPOSE 8080

# Start the server
CMD ["./backend"]
