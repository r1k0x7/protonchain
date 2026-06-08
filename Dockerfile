# Build stage
FROM rust:1.75-slim as builder

WORKDIR /app
COPY . .

RUN apt-get update && apt-get install -y     pkg-config     libssl-dev     llvm     libclang-dev     cmake     && rm -rf /var/lib/apt/lists/*

RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y     ca-certificates     libssl3     && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/proton /usr/local/bin/proton

# Create data directory
RUN mkdir -p /data

# Expose ports
EXPOSE 30333/tcp
EXPOSE 30333/udp
EXPOSE 9944/tcp
EXPOSE 9615/tcp

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=60s --retries=3     CMD proton info || exit 1

ENTRYPOINT ["proton"]
CMD ["node", "--network", "mainnet"]
