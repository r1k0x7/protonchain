.PHONY: build test bench clean install docker node validator

# Default target
all: build

# Build debug
build:
	cargo build

# Build release
release:
	cargo build --release

# Run tests
test:
	cargo test --all

# Run benchmarks
bench:
	cargo bench

# Clean build artifacts
clean:
	cargo clean

# Install locally
install: release
	cargo install --path . --force

# Run devnet node
node:
	cargo run -- node --network devnet

# Run validator
validator:
	cargo run --release -- node --validator --network devnet

# Format code
fmt:
	cargo fmt

# Lint
lint:
	cargo clippy --all-targets --all-features

# Generate documentation
docs:
	cargo doc --no-deps --open

# Docker build
docker:
	docker build -t proton-chain:latest .

# Docker run
docker-run:
	docker run -p 30333:30333 -v proton-data:/data proton-chain:latest

# Security audit
audit:
	cargo audit

# Check dependencies
deps:
	cargo tree
