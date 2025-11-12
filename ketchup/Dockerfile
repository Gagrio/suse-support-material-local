# Dockerfile for ketchup - Kubernetes Config Collector
# Optimized version with dependency caching
# Using SUSE BCI Rust stable image (1.90)

# Build stage
FROM registry.suse.com/bci/rust:1.90 as builder

WORKDIR /build

# Copy manifests first
COPY Cargo.toml Cargo.lock ./

# Create a dummy main to cache dependencies
RUN mkdir src && \
    echo "fn main() {}" > src/main.rs && \
    cargo build --release && \
    rm -rf src

# Now copy the real source code
COPY src ./src

# Touch main.rs to force rebuild of the app only
RUN touch src/main.rs

# Build the actual release binary
RUN cargo build --release

# Runtime stage - use minimal SUSE BCI micro
FROM registry.suse.com/bci/bci-micro:latest

# Copy CA certificates for HTTPS connections to Kubernetes API
COPY --from=builder /etc/ssl/certs /etc/ssl/certs

# Copy the binary from builder
COPY --from=builder /build/target/release/ketchup /usr/local/bin/ketchup

# Create output directory (ketchup uses /tmp by default)
RUN mkdir -p /tmp && chmod 1777 /tmp

# Set entrypoint
ENTRYPOINT ["/usr/local/bin/ketchup"]

# Default command (show help)
CMD ["--help"]
