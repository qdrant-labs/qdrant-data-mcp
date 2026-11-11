FROM rust:1.99-bookworm AS build
WORKDIR /app

# Build the dependencies first, so source changes don't rebuild them
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && touch src/lib.rs && echo "fn main() {}" > src/main.rs \
    && cargo build --release \
    && rm -r src

COPY src ./src
RUN touch src/lib.rs src/main.rs && cargo build --release

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /app/target/release/qdrant-data-mcp /usr/local/bin/qdrant-data-mcp
ENTRYPOINT ["qdrant-data-mcp"]
