# Stage 1: Build
FROM rust AS builder

# Set the working directory inside the container
WORKDIR /usr/src/app/backend

# Copy only Cargo manifests first (to leverage Docker caching of fetched deps)
COPY backend/Cargo.toml backend/Cargo.lock ./
COPY backend/bin/dashboard/Cargo.toml ./bin/dashboard/
COPY backend/crates/bridge/Cargo.toml ./crates/bridge/
COPY backend/crates/config/Cargo.toml ./crates/config/
COPY backend/crates/network/Cargo.toml ./crates/network/
COPY backend/crates/utils/Cargo.toml ./crates/utils/
COPY backend/crates/wallets/Cargo.toml ./crates/wallets/

RUN cargo fetch

# Copy the rest of the source code
COPY backend .

# Compile the Rust application
RUN cargo build -p status-dashboard-backend

# Stage 2: Runtime
FROM rust

# Set working directory in the final container
WORKDIR /usr/src/app

# Copy the compiled binary from the builder stage
COPY --from=builder /usr/src/app/backend/target/debug/status-dashboard-backend ./status-dashboard-backend

# Copy the example config as the runtime config; deployments can mount over this.
COPY backend/example-config.toml ./config.toml

# Expose the backend service port (should match docker-compose.yml)
EXPOSE 3000

# Run the compiled Rust backend
CMD ["./status-dashboard-backend"]
