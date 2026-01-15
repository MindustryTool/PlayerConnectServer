# ---------- Build stage ----------
FROM rust:1.85-slim AS build

# Create app user (optional but good practice)
RUN useradd -m rustuser

WORKDIR /app

# Cache dependencies first
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release
RUN rm -rf src

# Copy real source
COPY . .
RUN cargo build --release

# ---------- Runtime stage ----------
FROM gcr.io/distroless/cc-debian12

WORKDIR /app

# Copy compiled binary
COPY --from=build /app/target/release/server /app/server

# Run as non-root
USER nonroot

ENTRYPOINT ["/app/server"]
