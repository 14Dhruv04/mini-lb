# Build stage
FROM rust:1-bookworm AS builder
WORKDIR /app

COPY Cargo.toml ./
COPY minilb/Cargo.toml ./minilb/Cargo.toml
COPY demo-backend/Cargo.toml ./demo-backend/Cargo.toml

# Build the actual application.
# We intentionally compile the real source directly here; the project is small,
# and this avoids accidentally shipping the temporary cache binary.
COPY . .
RUN cargo build --release -p minilb

# Runtime stage
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/minilb /usr/local/bin/minilb

WORKDIR /etc/minilb
COPY minilb.yml ./minilb.yml

ENV MINILB_CONFIG=/etc/minilb/minilb.yml
EXPOSE 8080

ENTRYPOINT ["/usr/local/bin/minilb"]
