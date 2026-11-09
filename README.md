# Qdrant Data MCP

A prototype Model Context Protocol (MCP) server that exposes Qdrant's data operations as tools. It's generated at startup from Qdrant's OpenAPI specification with [fastmcp-ts](https://fastmcp-ts.docs.prefect.io/servers/openapi/), so there are no hand-written handlers. Cluster, snapshot, and health endpoints are filtered out, and write operations are off unless you turn them on.

It runs on stdio or streamable HTTP, optionally protected with bearer token auth, and compiles to a single binary with Deno.

## Install

The installer downloads the binary for your platform from the latest GitHub release and puts it in `~/.local/bin`. It supports Linux and macOS on x86_64 and arm64.

```sh
curl -fsSL https://raw.githubusercontent.com/qdrant-labs/qdrant-data-mcp/main/scripts/install.sh | sh
```

You can read the script first at [`scripts/install.sh`](scripts/install.sh). To pin a version or change where the binary goes, set `VERSION` or `INSTALL_DIR`. Set `QDRANT_MCP_REPO` to install from a fork.

```sh
curl -fsSL https://raw.githubusercontent.com/qdrant-labs/qdrant-data-mcp/main/scripts/install.sh \
  | VERSION=v1.20.0 INSTALL_DIR=/usr/local/bin sh
```

On Windows, download the `.exe.gz` from the [releases page](https://github.com/qdrant-labs/qdrant-data-mcp/releases).

Then check that it runs, and see what it would expose against your Qdrant instance:

```sh
qdrant-data-mcp --help
qdrant-data-mcp --version
qdrant-data-mcp --list
```

Point the server at your cluster with `--url` and `--api-key`, or with the `QDRANT_URL` and `QDRANT_API_KEY` environment variables, which keep the key out of your process list. Without them it connects to `http://localhost:6333`. On startup, it fetches (from GitHub) the Qdrant OpenAPI spec corresponding to the MCP server version.

## Docker

The image is published to the GitHub Container Registry. Pass your Qdrant Cloud URL and API key as environment variables. A bare `-e QDRANT_API_KEY` forwards the value from your shell, so the key never appears in the command.

```sh
export QDRANT_URL=https://xyz-example.cloud.qdrant.io:6333
export QDRANT_API_KEY=your-api-key

# stdio
docker run -i --rm -e QDRANT_URL -e QDRANT_API_KEY ghcr.io/qdrant-labs/qdrant-data-mcp

# HTTP
docker run --rm -p 127.0.0.1:8000:8000 -e QDRANT_URL -e QDRANT_API_KEY \
  ghcr.io/qdrant-labs/qdrant-data-mcp --http --host 0.0.0.0
```

Binding to `0.0.0.0` inside the container is fine as long as you publish the port on `127.0.0.1` only, or turn on auth. To pin a release, add its tag, for example `ghcr.io/qdrant-labs/qdrant-data-mcp:v1.20.0`.

For a self-hosted Qdrant on your machine, use `--url http://host.docker.internal:6333` instead, because inside a container `localhost` is the container itself. On Linux, also add `--add-host host.docker.internal:host-gateway` to `docker run`.

To build the image yourself, run `docker build -t qdrant-data-mcp .` and use that name in place of the registry path.

## Development Setup

To build and run from source you need [Deno](https://deno.com) 2.x and a running Qdrant instance.

```sh
git clone https://github.com/qdrant-labs/qdrant-data-mcp.git
cd qdrant-data-mcp

deno task start --list                # print the tools the server would expose
deno task start                       # run over stdio, read-only
deno task start --http --port 8000    # run over streamable HTTP
deno task dev                         # same as start, with --watch
```

Everything after the task name goes to the server, so all the options in the next section work the same way from source and from the binary. To check your changes:

```sh
deno check main.ts
deno test --allow-net --allow-env --allow-read
```

The tests build servers from a small inline spec and check which tools survive in read-only and write modes, how the `writer` tag is applied, and how auth options are resolved.

## Options

Every option has an environment variable fallback, so an MCP client can pass either. A flag always wins over its variable.

| Flag | Environment Variable | Description |
|---|---|---|
| `--url` | `QDRANT_URL` | Qdrant REST URL. Defaults to `http://localhost:6333`. |
| `--api-key` | `QDRANT_API_KEY` | Sent to Qdrant as the `api-key` header. |
| `--openapi` | `QDRANT_OPENAPI` | Path or URL of the OpenAPI spec. Defaults to the spec on GitHub corresponding to the current server version. |
| `--enable-writes` | `QDRANT_WRITES_ENABLED=1` | Expose tools that insert, update, or delete. |
| `--http` | none | Serve streamable HTTP instead of stdio. |
| `--host`, `--port` | none | HTTP bind address. Default `127.0.0.1:8000`. |
| `--list` | none | Print the exposed tools and exit. |

## Which Tools Are Exposed

Every tool is tagged `reader` or `writer`. Readers are always on. Writers only exist when you pass `--enable-writes`, so by default you get 16 tools, and with writes you get 33.

**Reader tools (16):**

| Area | Tools |
|---|---|
| Collections | `get_collections`, `get_collection`, `collection_exists`, `get_collection_aliases`, `get_collections_aliases`, `get_optimizations` |
| Points | `get_point`, `get_points`, `scroll_points`, `count_points` |
| Search | `query_points`, `query_batch_points`, `query_points_groups`, `facet`, `search_matrix_pairs`, `search_matrix_offsets` |

**Writer tools (17, with `--enable-writes`):**

| Area | Tools |
|---|---|
| Collections | `create_collection`, `update_collection`, `delete_collection`, `update_aliases` |
| Indexes and vectors | `create_field_index`, `delete_field_index`, `create_vector_name`, `delete_vector_name` |
| Points | `upsert_points`, `delete_points`, `batch_update` |
| Vectors and payloads | `update_vectors`, `delete_vectors`, `set_payload`, `overwrite_payload`, `delete_payload`, `clear_payload` |

Cluster, snapshot, and health endpoints are left out (the Distributed, Service, Snapshots, Beta, and Quotas groups in the OpenAPI spec). The route maps are in `buildRouteMaps()` in `main.ts`. Change them there to expose any of these.

## Using It With an MCP Client

For stdio, point the client at the installed binary, or at `deno task start` when running from source. In Claude Code:

```sh
claude mcp add \
  --env QDRANT_URL=https://xyz-example.cloud.qdrant.io:6333 \
  --env QDRANT_API_KEY=your-api-key \
  qdrant-data -- qdrant-data-mcp
```

For a local Qdrant, drop the `--env` lines. The default is `http://localhost:6333`. Add `--enable-writes` after the binary to include the write tools. For HTTP:

```sh
claude mcp add --transport http qdrant-data http://127.0.0.1:8000/mcp
```

## Securing the HTTP Server

With `--http` the server can verify JWT bearer tokens against a JWKS endpoint and publish [protected resource metadata](https://fastmcp-ts.docs.prefect.io/servers/auth/token-verification/), so MCP clients can discover your authorization server on their own. Setting any auth option turns auth on.

```sh
deno task start --http \
  --jwks https://idp.example.com/.well-known/jwks.json \
  --issuer https://idp.example.com/ \
  --audience http://127.0.0.1:8000/mcp \
  --auth-server https://idp.example.com \
  --public-url http://127.0.0.1:8000
```

| Flag | Environment Variable | Description |
|---|---|---|
| `--jwks` | `AUTH_JWKS_URI` | JWKS endpoint used to verify token signatures. Required. |
| `--public-url` | `AUTH_PUBLIC_URL` | The URL clients use to reach this server, without `/mcp`. Required. |
| `--auth-server` | `AUTH_AUTHORIZATION_SERVERS` | Issuer URL of an authorization server. Repeat the flag or comma-separate. Required. |
| `--issuer` | `AUTH_ISSUER` | Required token issuer. Must match the token's `iss` exactly, including any trailing slash. |
| `--audience` | `AUTH_AUDIENCE` | Required token audience. Repeat or comma-separate. |

A few behaviors to know about:

- A partial auth configuration is an error. The server never falls back to running unauthenticated.
- Auth options without `--http` are also an error. A leftover `AUTH_*` variable will stop a stdio run.
- Without `--issuer` and `--audience`, the server warns at startup, because it would accept tokens from any issuer sharing the keys or minted for other services.
- Clients that follow the MCP authorization flow request a token for the MCP URL itself (the RFC 8707 `resource` parameter), so the audience is usually your public URL plus `/mcp`.
- Run HTTP without auth only on loopback.


## Limitations

- Tools are generated from the spec as is. Names and descriptions come straight from the OpenAPI operations, and there's no curation for LLM use yet.
- The write tools include destructive operations such as `delete_collection` and `delete_points`. Enable writes only against data you can lose.
- fastmcp-ts doesn't forward inbound HTTP headers to the upstream API, so Qdrant credentials come from `--api-key`, not from the calling client.
- The default spec is pinned to Qdrant v1.20.0. Pass `--openapi` to match a different server version.
- The macOS and Windows binaries are cross-compiled on Linux and aren't signed. macOS may block an unsigned binary on first run.
