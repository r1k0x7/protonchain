#!/usr/bin/env bash
set -e

echo "🔧 Building Proton Chain..."

# Check Rust version
RUST_VERSION=$(rustc --version | awk '{print $2}')
echo "Rust version: $RUST_VERSION"

# Install dependencies if needed
if ! command -v cargo &> /dev/null; then
    echo "Installing Rust..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
fi

# Build
if [ "$1" == "release" ]; then
    echo "Building release..."
    cargo build --release
    echo "✅ Release build complete: target/release/proton"
else
    echo "Building debug..."
    cargo build
    echo "✅ Debug build complete: target/debug/proton"
fi

# Run tests
echo "🧪 Running tests..."
cargo test --all

echo "✅ Build complete!"
echo ""
echo "Usage:"
echo "  ./target/release/proton --help"
echo "  ./target/release/proton node --validator"
echo "  ./target/release/proton send --to proton_xxx --amount 1000"
