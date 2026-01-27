# ---------- Build stage ----------
FROM rust:1.85-slim AS build
WORKDIR /app

# Cache dependencies
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && touch src/main.rs
RUN cargo fetch

# Copy real source
COPY . .
RUN cargo build --release

# ---------- Runtime stage ----------
FROM gcr.io/distroless/cc-debian12
WORKDIR /app

COPY --from=build /app/target/release/server /app/server

USER nonroot
ENTRYPOINT ["/app/server"]
