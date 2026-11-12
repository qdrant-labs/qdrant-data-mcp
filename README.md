# Qdrant Data MCP

A prototype Model Context Protocol (MCP) server that exposes Qdrant's data
operations as tools. It's written in Rust and generated at startup from
Qdrant's OpenAPI specification with
[rmcp-openapi](https://github.com/lx-industries/rmcp-openapi), so there are no
hand-written handlers. Cluster, snapshot, and health endpoints are filtered out,
and write operations are off unless you turn them on.

It runs on stdio or streamable HTTP, and ships as a single binary of about
15 MB. The HTTP transport can be protected with bearer token auth (see
[Securing the HTTP Server](#securing-the-http-server)).

## Install

The installer downloads the binary for your platform from the latest GitHub
release and puts it in `~/.local/bin`. It supports Linux and macOS on x86_64 and
arm64.

```sh
curl -fsSL https://raw.githubusercontent.com/qdrant-labs/qdrant-data-mcp/main/scripts/install.sh | sh
```

You can read the script first at [`scripts/install.sh`](scripts/install.sh). To
pin a version or change where the binary goes, set `VERSION` or `INSTALL_DIR`.
Set `QDRANT_MCP_REPO` to install from a fork.

```sh
curl -fsSL https://raw.githubusercontent.com/qdrant-labs/qdrant-data-mcp/main/scripts/install.sh \
  | VERSION=v1.20.0-server.3 INSTALL_DIR=/usr/local/bin sh
```

Versions look like `v1.20.0-server.3`: the Qdrant version the server targets,
then the server's own revision. The default OpenAPI spec is the one for that
Qdrant version.

On Windows, download the `.exe.gz` from the
[releases page](https://github.com/qdrant-labs/qdrant-data-mcp/releases).

Then check that it runs, and see what it would expose against your Qdrant
instance:

```sh
qdrant-data-mcp --help
qdrant-data-mcp --version
qdrant-data-mcp --list
```

Point the server at your cluster with `--url` and `--api-key`, or with the
`QDRANT_URL` and `QDRANT_API_KEY` environment variables, which keep the key out
of your process list. Without them it connects to `http://localhost:6333`. On
startup, it fetches (from GitHub) the Qdrant OpenAPI spec corresponding to the
MCP server version.

## Docker

The image is published to the GitHub Container Registry for linux/amd64 and
linux/arm64. Pass your Qdrant Cloud URL and API key as environment variables. A
bare `-e QDRANT_API_KEY` forwards the value from your shell, so the key never
appears in the command.

```sh
export QDRANT_URL=https://xyz-example.cloud.qdrant.io:6333
export QDRANT_API_KEY=your-api-key

# stdio
docker run -i --rm -e QDRANT_URL -e QDRANT_API_KEY ghcr.io/qdrant-labs/qdrant-data-mcp

# HTTP
docker run --rm -p 127.0.0.1:8000:8000 -e QDRANT_URL -e QDRANT_API_KEY \
  ghcr.io/qdrant-labs/qdrant-data-mcp --http --host 0.0.0.0
```

Binding to `0.0.0.0` inside the container is fine as long as you publish the
port on `127.0.0.1` only, or turn on auth. To pin
a release, add its tag, for example
`ghcr.io/qdrant-labs/qdrant-data-mcp:v1.20.0-server.3`. The tag `v1.20.0` always
points at the latest server revision for that Qdrant version.

For a self-hosted Qdrant on your machine, use
`--url http://host.docker.internal:6333` instead, because inside a container
`localhost` is the container itself. On Linux, also add
`--add-host host.docker.internal:host-gateway` to `docker run`.

To build the image yourself, run `docker build -t qdrant-data-mcp .` and use
that name in place of the registry path. The image is based on a distroless
runtime, so it has no shell.

## Development Setup

To build and run from source you need [Rust](https://rustup.rs) (a recent stable
toolchain, since the crate uses the 2024 edition) and a running Qdrant instance.

```sh
git clone https://github.com/qdrant-labs/qdrant-data-mcp.git
cd qdrant-data-mcp

cargo run -- --list                  # print the tools the server would expose
cargo run                            # run over stdio, read-only
cargo run -- --http --port 8000      # run over streamable HTTP
cargo build --release                # build target/release/qdrant-data-mcp
```

Everything after `--` goes to the server, so all the options in the next
section work the same way from source and from the binary. To check your
changes:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

The tests don't need a network or a Qdrant instance. They cover which
operations become tools in read-only and write modes, how recursive schemas are
broken and repeated ones are shared, how headers and provider keys are built,
how `local/` texts are rewritten, how bearer tokens are verified, and how
options and versions are parsed.

The server's version comes from `Cargo.toml`, so bumping it there also changes
which Qdrant spec is fetched by default.

## Options

Every option has an environment variable fallback, so an MCP client can pass
either. A flag always wins over its variable.

| Flag                       | Environment Variable        | Description                                                                                                  |
| -------------------------- | --------------------------- | ------------------------------------------------------------------------------------------------------------ |
| `--url`                    | `QDRANT_URL`                | Qdrant REST URL. Defaults to `http://localhost:6333`.                                                        |
| `--api-key`                | `QDRANT_API_KEY`            | Sent to Qdrant as the `api-key` header.                                                                      |
| `--openapi`                | `QDRANT_OPENAPI`            | Path or URL of the OpenAPI spec. Defaults to the spec on GitHub corresponding to the current server version. |
| `--enable-writes`          | `QDRANT_WRITES_ENABLED=1`   | Expose tools that insert, update, or delete.                                                                 |
| `--qci-external-providers` | none                        | Send external embedding provider keys from the environment to Qdrant Cloud Inference. See below.             |
| `--embeddings-url`         | `QDRANT_EMBEDDINGS_URL`     | OpenAI-compatible embeddings endpoint for `local/<model>` texts. See below.                                  |
| `--embeddings-api-key`     | `QDRANT_EMBEDDINGS_API_KEY` | Bearer token for that endpoint.                                                                              |
| `--http`                   | none                        | Serve streamable HTTP instead of stdio.                                                                      |
| `--host`, `--port`         | none                        | HTTP bind address. Default `127.0.0.1:8000`.                                                                 |
| `--list`                   | none                        | Print the exposed tools and exit.                                                                            |
| `-v`, `--version`          | none                        | Print the version and exit.                                                                                  |

Logs go to stderr, and only errors are shown by default. Set `RUST_LOG=info` to
see more.

### External Embedding Providers

Qdrant Cloud Inference can call OpenAI, Cohere, Jina AI, and OpenRouter models
for you, but it needs your key for that provider on every request, as a header.
The MCP tools can't set headers, so `--qci-external-providers` does it for you:
it reads whichever of `OPENAI_API_KEY`, `COHERE_API_KEY`, `JINA_API_KEY`, and
`OPENROUTER_API_KEY` are set and sends them as the `openai-api-key`,
`cohere-api-key`, `jina-api-key`, and `openrouter-api-key` headers. The keys
then never pass through the conversation with your MCP client.

```sh
export OPENROUTER_API_KEY=sk-or-...
qdrant-data-mcp --url https://xyz-example.cloud.qdrant.io:6333 --qci-external-providers
```

An upsert or query can then use a model such as
`openrouter/google/gemini-embedding-2` with no `options` object. Things to know:

- The flag fails at startup if none of the four variables is set, so a typo
  doesn't turn into a silent no-op.
- The keys are sent on every request to Qdrant, not only inference calls. For
  that reason `--url` must use `https`, except for `localhost`.
- Every provider key in the environment is forwarded, whether or not you use
  that provider. Startup prints which providers are forwarded and to which
  host. It never prints the key values.

### Embeddings for Self-Hosted Qdrant

Qdrant Cloud Inference isn't available on self-hosted Qdrant. To get the same
`{"text": ..., "model": ...}` convenience there, point `--embeddings-url` at any
OpenAI-compatible embeddings endpoint, such as Ollama, LM Studio, vLLM, or Text
Embeddings Inference. Use a model named `local/<model>`, and the server replaces
the text with a vector from that endpoint before the request reaches Qdrant.
Your MCP client never sees the vectors.

```sh
ollama pull nomic-embed-text
claude mcp add qdrant-data -- qdrant-data-mcp --enable-writes --embeddings-url http://localhost:11434/v1
```

The collection's vector size must match the model's output size. Run
`ollama show nomic-embed-text` to see its embedding length, which is 768. Then
create a collection, and give the model name as `local/nomic-embed-text`
wherever a vector goes. A `create_collection` call:

```json
{
  "collection_name": "articles",
  "request_body": {
    "vectors": { "dense": { "size": 768, "distance": "Cosine" } }
  }
}
```

An `upsert_points` call, with the text where a vector would normally be:

```json
{
  "collection_name": "articles",
  "request_body": {
    "points": [{
      "id": 1,
      "payload": { "text": "Recipe for baking chocolate chip cookies" },
      "vector": {
        "dense": {
          "text": "Recipe for baking chocolate chip cookies",
          "model": "local/nomic-embed-text"
        }
      }
    }]
  }
}
```

And a `query_points` call:

```json
{
  "collection_name": "articles",
  "request_body": {
    "using": "dense",
    "query": {
      "text": "a sweet dessert made with chocolate",
      "model": "local/nomic-embed-text"
    },
    "limit": 3
  }
}
```

Things to know:

- Only models starting with `local/` are handled this way. Any other model goes
  to Qdrant unchanged, so Cloud Inference keeps working.
- The prefix is removed before the call, so Ollama receives `nomic-embed-text`.
- Unlike Cloud Inference, the server doesn't add the query or passage prefixes
  some models expect. Put them in the text yourself.
- If the endpoint fails, the tool call fails and nothing is sent to Qdrant.
- Your texts go to the endpoint, and the API key is sent as a bearer token, so
  use `https` for any remote endpoint.

## Which Tools Are Exposed

Every tool is either a reader or a writer, and `--list` shows which. Readers are
always on. Writers only exist when you pass `--enable-writes`, so by default you
get 16 tools, and with writes you get 33. When writes are off, the writer
operations are removed from the spec before the tools are generated, so those
tools don't exist at all.

**Reader tools (16):**

| Area        | Tools                                                                                                                              |
| ----------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| Collections | `get_collections`, `get_collection`, `collection_exists`, `get_collection_aliases`, `get_collections_aliases`, `get_optimizations` |
| Points      | `get_point`, `get_points`, `scroll_points`, `count_points`                                                                         |
| Search      | `query_points`, `query_batch_points`, `query_points_groups`, `facet`, `search_matrix_pairs`, `search_matrix_offsets`               |

**Writer tools (17, with `--enable-writes`):**

| Area                 | Tools                                                                                                     |
| -------------------- | --------------------------------------------------------------------------------------------------------- |
| Collections          | `create_collection`, `update_collection`, `delete_collection`, `update_aliases`                           |
| Indexes and vectors  | `create_field_index`, `delete_field_index`, `create_vector_name`, `delete_vector_name`                    |
| Points               | `upsert_points`, `delete_points`, `batch_update`                                                          |
| Vectors and payloads | `update_vectors`, `delete_vectors`, `set_payload`, `overwrite_payload`, `delete_payload`, `clear_payload` |

Qdrant uses POST for queries that take a body, so `get_points`, `scroll_points`,
`count_points`, `facet`, and the query and search matrix tools are readers even
though they're POST requests.

Cluster, snapshot, and health endpoints are left out (the Distributed, Service,
Snapshots, Beta, and Quotas groups in the OpenAPI spec). The rules are in
`classify()` in `src/routes.rs`. Change them there to expose any of these.

### Arguments and Results

Path and query parameters are top-level arguments, such as `collection_name`.
The JSON body of a request goes under a single `request_body` argument, as in the
examples above. A tool result has the HTTP status and the response body:
`{"status": 200, "body": {...}}`.

## Using It With an MCP Client

For stdio, point the client at the installed binary, or at
`target/release/qdrant-data-mcp` after building from source. In Claude Code:

```sh
claude mcp add \
  --env QDRANT_URL=https://xyz-example.cloud.qdrant.io:6333 \
  --env QDRANT_API_KEY=your-api-key \
  qdrant-data -- qdrant-data-mcp
```

For a local Qdrant, drop the `--env` lines. The default is
`http://localhost:6333`. Add `--enable-writes` after the binary to include the
write tools. For HTTP:

```sh
claude mcp add --transport http qdrant-data http://127.0.0.1:8000/mcp
```

### Usage Instructions

The server sends usage guidance to the model when a client connects, in the
`instructions` field of the initialize response: how arguments and results are
shaped (`request_body`), how to look up collections, how to write filters,
how Qdrant Cloud Inference works, and what this server's setup allows (read-only
or not, whether `local/` models work, and which external providers' models are
available). The text is in [`src/guide.md`](src/guide.md), and the setup lines at
the end are generated from the flags you started the server with.

## Securing the HTTP Server

With `--http` the server can verify JWT bearer tokens against a JWKS endpoint
and publish protected resource metadata (RFC 9728), so MCP clients can discover
your authorization server on their own. Setting any auth option turns auth on.

```sh
qdrant-data-mcp --http \
  --jwks https://idp.example.com/.well-known/jwks.json \
  --issuer https://idp.example.com/ \
  --audience http://127.0.0.1:8000/mcp \
  --auth-server https://idp.example.com \
  --public-url http://127.0.0.1:8000
```

| Flag            | Environment Variable         | Description                                                                                |
| --------------- | ---------------------------- | ------------------------------------------------------------------------------------------ |
| `--jwks`        | `AUTH_JWKS_URI`              | JWKS endpoint used to verify token signatures. Required.                                   |
| `--public-url`  | `AUTH_PUBLIC_URL`            | The URL clients use to reach this server, without `/mcp`. Required.                        |
| `--auth-server` | `AUTH_AUTHORIZATION_SERVERS` | Issuer URL of an authorization server. Repeat the flag or comma-separate. Required.        |
| `--issuer`      | `AUTH_ISSUER`                | Required token issuer. Must match the token's `iss` exactly, including any trailing slash. |
| `--audience`    | `AUTH_AUDIENCE`              | Required token audience. Repeat or comma-separate.                                         |

Requests to `/mcp` without a valid token get a `401` with a `WWW-Authenticate`
header that points at `/.well-known/oauth-protected-resource/mcp`. That
document needs no token. A few behaviors to know about:

- A partial auth configuration is an error. The server never falls back to
  running unauthenticated.
- Auth options without `--http` are also an error. A leftover `AUTH_*` variable
  will stop a stdio run.
- Without `--issuer` and `--audience`, the server warns at startup, because it
  would accept tokens from any issuer sharing the keys or minted for other
  services.
- Clients that follow the MCP authorization flow request a token for the MCP URL
  itself (the RFC 8707 `resource` parameter), so the audience is usually your
  public URL plus `/mcp`.
- Only asymmetric signatures are accepted (RS, PS, ES, and EdDSA). Tokens must
  have an `exp` and a `kid`, and the signing keys are cached and refetched when
  an unknown `kid` shows up, at most every 30 seconds.
- Only JWT access tokens work. If your provider issues opaque tokens, configure
  it to issue JWTs for this audience.
- Run HTTP without auth only on loopback.

## Limitations

- Tools are generated from the spec as is. Names and descriptions come straight
  from the OpenAPI operations, and there's no curation for LLM use yet.
- The write tools include destructive operations such as `delete_collection` and
  `delete_points`. Enable writes only against data you can lose.
- Qdrant's filters can nest inside each other without limit, which the tool
  generator can't describe. A filter nested inside a condition is described as
  "any value", so clients don't get a schema for the inner levels. Qdrant still
  validates the request.
- Tool annotations come from the HTTP method. The POST readers, such as
  `query_points` and `scroll_points`, are marked as not read-only, so some MCP
  clients may ask for confirmation before running them.
- The default spec is the one for the Qdrant version in the server's version
  (`v1.20.0` for `v1.20.0-server.3`). Pass `--openapi` to match a different
  Qdrant version.
- The Linux binaries need glibc 2.35 or newer. The macOS and Windows binaries
  aren't signed, and macOS may warn about an unsigned binary on first run.
