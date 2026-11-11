FROM denoland/deno:2.9.6 AS build
WORKDIR /app
COPY deno.json deno.lock main.ts ./
RUN deno compile --allow-env --allow-read --allow-net -o qdrant-data-mcp main.ts

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home mcp
COPY --from=build /app/qdrant-data-mcp /usr/local/bin/qdrant-data-mcp
USER mcp
ENTRYPOINT ["qdrant-data-mcp"]
