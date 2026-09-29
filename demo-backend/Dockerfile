FROM rust:1-bookworm AS builder
WORKDIR /app
COPY demo-backend/Cargo.toml ./Cargo.toml
COPY demo-backend/src ./src
RUN cargo build --release

FROM debian:bookworm-slim
COPY --from=builder /app/target/release/demo-backend /usr/local/bin/demo-backend
EXPOSE 8001
ENTRYPOINT ["/usr/local/bin/demo-backend"]
