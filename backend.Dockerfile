# Stage 1: Build
FROM rust AS builder

# Set the working directory inside the container
WORKDIR /usr/src/app/backend

# Copy only Cargo files first (to leverage Docker caching)
COPY backend/Cargo.toml backend/Cargo.lock ./

RUN cargo fetch

# Copy the rest of the source code
COPY backend .

# Compile the Rust application
RUN cargo build

# Stage 2: Runtime
FROM rust

# Set working directory in the final container
WORKDIR /usr/src/app/backend

# Copy the compiled binary from the builder stage
COPY --from=builder /usr/src/app/backend/target/debug/backend .

# Expose the backend service port (should match docker-compose.yml)
EXPOSE 3000

# Run the compiled Rust backend
CMD ["./backend"]
