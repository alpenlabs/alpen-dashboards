# Default list of recipes
default:
	@just --list

# Run all checks
check:
	cd backend && cargo fmt --all -- --check
	cd backend && cargo clippy --all-targets --all-features -- -D warnings
	cd backend && cargo test --all --locked
	taplo fmt --check
	cd frontend && npm run format:check
	cd frontend && npm run lint

# Format code
format:
	cd backend && cargo fmt --all
	taplo fmt
	cd frontend && npm run format

# Run tests
test:
	cd backend && cargo test --all
	cd frontend && npm test

# Build the project
build:
	cd backend && cargo build --release
	cd frontend && npm run build

# Run development environment
dev:
	docker-compose up

# Clean build artifacts
clean:
	cd backend && cargo clean
	cd frontend && rm -rf node_modules dist

# Generate documentation
docs:
	cd backend && cargo doc --no-deps --open

# Generate documentation (for CI)
docs-check:
	cd backend && cargo doc --no-deps
