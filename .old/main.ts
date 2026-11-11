/**
 * Data-only MCP server generated from Qdrant's OpenAPI spec.
 *
 *   deno task dev                      # stdio, read-only
 *   deno task dev --enable-writes      # also expose "writer" tools
 *   deno task compile                  # -> ./qdrant-data-mcp binary
 *
 * Every option falls back to an env var; a flag always wins:
 *   --url            QDRANT_URL              (default http://localhost:6333)
 *   --api-key        QDRANT_API_KEY
 *   --openapi        QDRANT_OPENAPI          (path or URL; default v1.20.0 spec on GitHub)
 *   --enable-writes  QDRANT_WRITES_ENABLED=1
 *
 * --qci-external-providers sends the provider keys found in the env
 * (OPENAI_/COHERE_/JINA_/OPENROUTER_API_KEY) as headers to Qdrant Cloud Inference.
 *
 * HTTP auth (bearer JWTs verified against a JWKS, see resolveAuth below) is
 * enabled by setting --jwks, --public-url and --auth-server, or their AUTH_*
 * env vars. Auth options without --http are an error, never silently ignored.
 */
import {
  createOpenAPIServer,
  jwtVerifier,
  remoteAuth,
} from "@prefecthq/fastmcp-ts/server";
import type { RouteMap } from "@prefecthq/fastmcp-ts/server";
import { parseArgs } from "@std/cli/parse-args";
import denoJson from "./deno.json" with { type: "json" };

// Versions look like "1.20.0-server.1": the Qdrant version this targets,
// plus the revision of the server itself.
export const MCP_VERSION = `v${denoJson.version}`;

export function qdrantVersion(version: string): string {
  const match = version.match(/^v?(\d+\.\d+\.\d+)(-|$)/);
  if (!match) throw new Error(`Cannot read a Qdrant version from "${version}"`);
  return `v${match[1]}`;
}

const DEFAULT_SPEC = `https://raw.githubusercontent.com/qdrant/qdrant/${
  qdrantVersion(MCP_VERSION)
}/docs/redoc/master/openapi.json`;
const WRITER_TAG = "writer";
const READER_TAG = "reader";

const HELP = `qdrant-data-mcp: Qdrant data MCP server (from OpenAPI)

Options:
  --url <url>        Qdrant REST URL             (env: QDRANT_URL, default http://localhost:6333)
  --api-key <key>    Qdrant API key              (env: QDRANT_API_KEY)
  --openapi <src>    OpenAPI spec path or URL    (env: QDRANT_OPENAPI)
  --enable-writes    Expose insert/update/delete (env: QDRANT_WRITES_ENABLED=1)
  --qci-external-providers
                     Send OPENAI_API_KEY, COHERE_API_KEY, JINA_API_KEY, and
                     OPENROUTER_API_KEY from the env to Qdrant Cloud Inference
                     as headers (requires an https --url)
  --embeddings-url <url>
                     OpenAI-compatible embeddings endpoint (for example Ollama's
                     http://localhost:11434/v1). Texts sent with a "local/<model>"
                     model are embedded there before reaching Qdrant
                     (env: QDRANT_EMBEDDINGS_URL)
  --embeddings-api-key <key>
                     Bearer token for that endpoint (env: QDRANT_EMBEDDINGS_API_KEY)
  --http             Serve streamable HTTP instead of stdio
  --host <host>      HTTP host (default 127.0.0.1)
  --port <port>      HTTP port (default 8000)
  --list             Print exposed tools and exit
  -h, --help         Show this help

HTTP auth (requires --http; setting any of these turns auth on):
  --jwks <url>           JWKS endpoint used to verify tokens   (env: AUTH_JWKS_URI)
  --public-url <url>     Public URL clients use to reach this
                         server, without the /mcp path         (env: AUTH_PUBLIC_URL)
  --auth-server <url>    Issuer URL of an authorization server
                         clients should use; repeat or comma-
                         separate for several                  (env: AUTH_AUTHORIZATION_SERVERS)
  --issuer <url>         Required token issuer (recommended)   (env: AUTH_ISSUER)
  --audience <aud>       Required token audience (recommended);
                         repeat or comma-separate              (env: AUTH_AUDIENCE)

Without auth, --http serves unauthenticated: keep it on 127.0.0.1.
`;

// Order matters: the first matching RouteMap wins. Patterns are searched
// unanchored, so every pattern below is anchored explicitly.
export function buildRouteMaps(enableWrites: boolean): RouteMap[] {
  const maps: RouteMap[] = [
    // --- Excluded: not data access. By tag first... ---
    { tags: ["Distributed"], mcpType: "exclude" }, // cluster, peers, shards
    { tags: ["Service"], mcpType: "exclude" }, // telemetry, metrics, k8s probes
    { tags: ["Snapshots"], mcpType: "exclude" }, // backup / restore
    { tags: ["Beta"], mcpType: "exclude" }, // /issues
    { tags: ["Quotas"], mcpType: "exclude" }, // usage accounting
    // ...then by path, in case tags change between versions.
    {
      pattern:
        "^/(cluster|telemetry|metrics|healthz|livez|readyz|snapshots|issues)(/.*)?$",
      mcpType: "exclude",
    },
    {
      pattern: "^/collections/\\{[^}]+\\}/(cluster|shards|snapshots)(/.*)?$",
      mcpType: "exclude",
    },
    { pattern: "^/$", mcpType: "exclude" },
    // --- Read-only POSTs (Qdrant uses POST for queries with a body) ---
    {
      methods: ["POST"],
      pattern: "^/collections/\\{[^}]+\\}/(points|points/scroll|points/count" +
        "|facet|points/query|points/query/batch|points/query/groups" +
        "|points/search/matrix/(pairs|offsets))$",
      mcpType: "tool",
      mcpTags: [READER_TAG],
    },
  ];

  // --- Everything else that is not a GET mutates state ---
  // When writes are off these routes are excluded outright. A FilterTransform
  // would only hide them from tools/list; they would stay callable by name.
  maps.push({
    methods: ["POST", "PUT", "PATCH", "DELETE"],
    mcpType: enableWrites ? "tool" : "exclude",
    mcpTags: [WRITER_TAG],
  });

  // --- Remaining GETs ---
  maps.push({ methods: ["GET"], mcpType: "tool", mcpTags: [READER_TAG] });
  return maps;
}

export async function loadSpec(source: string): Promise<string> {
  if (source.startsWith("http://") || source.startsWith("https://")) {
    const res = await fetch(source);
    if (!res.ok) throw new Error(`Fetching ${source} failed: ${res.status}`);
    return await res.text();
  }
  return await Deno.readTextFile(source);
}

export interface AuthConfig {
  jwksUri: string;
  issuer?: string;
  audience?: string | string[];
  authorizationServers: string[];
  productionUrl: string;
}

export interface AuthInputs {
  jwks?: string;
  publicUrl?: string;
  authServers: string[];
  issuer?: string;
  audience: string[];
}

/** Split repeated and/or comma-separated values into a clean list. */
export function toList(values: string | string[] | undefined): string[] {
  return (Array.isArray(values) ? values : values ? [values] : [])
    .flatMap((v) => v.split(","))
    .map((v) => v.trim())
    .filter(Boolean);
}

/**
 * Turn the auth inputs into a config. Returns undefined only when NO auth
 * option was given. A partial config throws: starting an unauthenticated
 * server because one flag was forgotten is the worst failure mode here.
 */
export function resolveAuth(i: AuthInputs): AuthConfig | undefined {
  const anySet = i.jwks || i.publicUrl || i.issuer || i.authServers.length ||
    i.audience.length;
  if (!anySet) return undefined;

  const missing = [
    !i.jwks && "--jwks (AUTH_JWKS_URI)",
    !i.publicUrl && "--public-url (AUTH_PUBLIC_URL)",
    !i.authServers.length && "--auth-server (AUTH_AUTHORIZATION_SERVERS)",
  ].filter(Boolean);
  if (missing.length) {
    throw new Error(
      `Incomplete auth configuration, missing: ${missing.join(", ")}`,
    );
  }
  for (
    const [label, value] of [
      ["--jwks", i.jwks!],
      ["--public-url", i.publicUrl!],
      ...i.authServers.map((u) => ["--auth-server", u] as const),
    ] as const
  ) {
    if (!URL.canParse(value) || !/^https?:/.test(value)) {
      throw new Error(`${label} must be an http(s) URL, got "${value}"`);
    }
  }
  return {
    jwksUri: i.jwks!,
    productionUrl: i.publicUrl!.replace(/\/+$/, ""),
    authorizationServers: i.authServers,
    issuer: i.issuer,
    audience: i.audience.length ? i.audience : undefined,
  };
}

export interface QCIExternConfig {
  openaiKey?: string;
  openrouterKey?: string;
  cohereKey?: string;
  jinaKey?: string;
}

export function resolveQCIExternConfig(): QCIExternConfig | undefined {
  const cfg: QCIExternConfig = {
    openrouterKey: Deno.env.get("OPENROUTER_API_KEY"),
    openaiKey: Deno.env.get("OPENAI_API_KEY"),
    cohereKey: Deno.env.get("COHERE_API_KEY"),
    jinaKey: Deno.env.get("JINA_API_KEY"),
  };
  const anyset = cfg.openaiKey || cfg.cohereKey || cfg.jinaKey ||
    cfg.openrouterKey;
  if (anyset) {
    return cfg;
  }
}

// The keys go out on every request, so refuse to send them over plain HTTP
// to a remote host.
export function assertSafeQCIUrl(url: string): void {
  const { protocol, hostname, host } = new URL(url);
  const local = ["localhost", "127.0.0.1", "[::1]"].includes(hostname);
  if (protocol !== "https:" && !local) {
    throw new Error(
      "--qci-external-providers sends provider API keys to Qdrant, so --url " +
        `must use https (got ${protocol}//${host})`,
    );
  }
}

const LOCAL_PREFIX = "local/";

export interface EmbeddingsConfig {
  url: string;
  apiKey?: string;
}

type DocRef = {
  parent: Record<string, unknown>;
  key: string;
  text: string;
  model: string;
};

// Finds every {"text": ..., "model": "local/<name>"} object in a request body.
function collectLocalDocs(node: unknown, refs: DocRef[]): void {
  if (!node || typeof node !== "object") return;
  for (const [key, value] of Object.entries(node)) {
    const doc = value as { text?: unknown; model?: unknown } | null;
    if (
      typeof doc?.text === "string" && typeof doc.model === "string" &&
      doc.model.startsWith(LOCAL_PREFIX)
    ) {
      refs.push({
        parent: node as Record<string, unknown>,
        key,
        text: doc.text,
        model: doc.model.slice(LOCAL_PREFIX.length),
      });
    } else {
      collectLocalDocs(value, refs);
    }
  }
}

async function embed(
  cfg: EmbeddingsConfig,
  fetchImpl: typeof fetch,
  model: string,
  input: string[],
): Promise<number[][]> {
  const res = await fetchImpl(`${cfg.url.replace(/\/+$/, "")}/embeddings`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      ...(cfg.apiKey ? { authorization: `Bearer ${cfg.apiKey}` } : {}),
    },
    body: JSON.stringify({ model, input }),
  });
  if (!res.ok) {
    throw new Error(
      `Embeddings request failed (${res.status}): ${await res.text()}`,
    );
  }
  const { data } = await res.json() as {
    data?: { index?: number; embedding: number[] }[];
  };
  if (data?.length !== input.length) {
    throw new Error(
      `Embeddings endpoint returned ${data?.length} vectors for ${input.length} inputs`,
    );
  }
  return data.toSorted((a, b) => (a.index ?? 0) - (b.index ?? 0))
    .map((d) => d.embedding);
}

// Wraps fetch so that "local/<model>" text objects in request bodies are
// replaced by vectors from an OpenAI-compatible embeddings endpoint.
export function withLocalEmbeddings(
  cfg: EmbeddingsConfig,
  fetchImpl: typeof fetch = fetch,
): typeof fetch {
  return async (input, init) => {
    if (
      typeof init?.body !== "string" || !init.body.includes(`"${LOCAL_PREFIX}`)
    ) {
      return fetchImpl(input, init);
    }
    const json = JSON.parse(init.body);
    const refs: DocRef[] = [];
    collectLocalDocs(json, refs);
    for (const model of new Set(refs.map((r) => r.model))) {
      const group = refs.filter((r) => r.model === model);
      const vectors = await embed(
        cfg,
        fetchImpl,
        model,
        group.map((r) => r.text),
      );
      group.forEach((r, i) => {
        r.parent[r.key] = vectors[i];
      });
    }
    return fetchImpl(input, { ...init, body: JSON.stringify(json) });
  };
}

export interface ServerConfig {
  spec: string;
  url: string;
  apiKey?: string;
  enableWrites: boolean;
  auth?: AuthConfig;
  cloudInferenceExt?: QCIExternConfig;
  embeddings?: EmbeddingsConfig;
}

export function resolveHeaders(cfg: ServerConfig): Record<string, string> {
  const headers: Record<string, string> = {};

  if (cfg.apiKey) {
    headers["api-key"] = cfg.apiKey;
  }

  if (cfg.cloudInferenceExt) {
    if (cfg.cloudInferenceExt.cohereKey) {
      headers["cohere-api-key"] = cfg.cloudInferenceExt.cohereKey;
    }
    if (cfg.cloudInferenceExt.jinaKey) {
      headers["jina-api-key"] = cfg.cloudInferenceExt.jinaKey;
    }
    if (cfg.cloudInferenceExt.openaiKey) {
      headers["openai-api-key"] = cfg.cloudInferenceExt.openaiKey;
    }
    if (cfg.cloudInferenceExt.openrouterKey) {
      headers["openrouter-api-key"] = cfg.cloudInferenceExt.openrouterKey;
    }
  }

  return headers;
}

export interface ToolInfo {
  name: string;
  tags: string[];
}

export function buildServer(cfg: ServerConfig) {
  const tools: ToolInfo[] = [];
  const auth = cfg.auth && remoteAuth({
    verifier: jwtVerifier({
      jwksUri: cfg.auth.jwksUri,
      issuer: cfg.auth.issuer,
      audience: cfg.auth.audience,
    }),
    baseUrl: cfg.auth.productionUrl,
    authorizationServers: cfg.auth.authorizationServers,
  });
  const headers = resolveHeaders(cfg);
  const server = createOpenAPIServer({
    spec: cfg.spec,
    name: "Qdrant Data MCP",
    version: denoJson.version,
    client: {
      baseUrl: cfg.url,
      headers,
      fetch: cfg.embeddings ? withLocalEmbeddings(cfg.embeddings) : undefined,
    },
    routeMaps: buildRouteMaps(cfg.enableWrites),
    // Called for every component that gets registered, so it doubles as the
    // source for --list (and the tests).
    componentFn: (_route, component) => {
      if (component.kind === "tool") {
        tools.push({ name: component.name, tags: component.tags });
      }
    },
    serverOptions: auth ? { auth } : undefined,
  });
  return { server, tools };
}

async function main() {
  const args = parseArgs(Deno.args, {
    string: [
      "url",
      "api-key",
      "openapi",
      "host",
      "port",
      "jwks",
      "public-url",
      "issuer",
      "embeddings-url",
      "embeddings-api-key",
    ],
    collect: ["audience", "auth-server"],
    boolean: [
      "enable-writes",
      "http",
      "list",
      "help",
      "version",
      "qci-external-providers",
    ],
    alias: { h: "help", v: "version" },
  });

  if (args.help) {
    console.log(HELP);
    return;
  }

  if (args.version) {
    console.log(MCP_VERSION);
    return;
  }

  const env = (k: string) => Deno.env.get(k) || undefined;
  const enableWrites = args["enable-writes"] ||
    env("QDRANT_WRITES_ENABLED") === "1";
  const flagOrEnv = (flag: string[], envValue: string | undefined) =>
    flag.length ? flag : toList(envValue);

  const auth = resolveAuth({
    jwks: args.jwks ?? env("AUTH_JWKS_URI"),
    publicUrl: args["public-url"] ?? env("AUTH_PUBLIC_URL"),
    authServers: flagOrEnv(
      toList(args["auth-server"] as string[]),
      env("AUTH_AUTHORIZATION_SERVERS"),
    ),
    issuer: args.issuer ?? env("AUTH_ISSUER"),
    audience: flagOrEnv(
      toList(args.audience as string[]),
      env("AUTH_AUDIENCE"),
    ),
  });

  const url = args.url ?? Deno.env.get("QDRANT_URL") ?? "http://localhost:6333";
  const qciExtern = args["qci-external-providers"]
    ? resolveQCIExternConfig()
    : undefined;
  if (args["qci-external-providers"] && !args.list) {
    if (!qciExtern) {
      throw new Error(
        "--qci-external-providers is set, but none of OPENAI_API_KEY, " +
          "COHERE_API_KEY, JINA_API_KEY, or OPENROUTER_API_KEY is in the environment",
      );
    }
    assertSafeQCIUrl(url);
    const names = Object.entries(qciExtern)
      .filter(([, key]) => key)
      .map(([name]) => name.replace(/Key$/, ""));
    console.error(
      `forwarding ${names.join(", ")} API keys to ${new URL(url).host}`,
    );
  }

  const embeddingsUrl = args["embeddings-url"] ?? env("QDRANT_EMBEDDINGS_URL");
  if (
    embeddingsUrl &&
    !(URL.canParse(embeddingsUrl) && /^https?:/.test(embeddingsUrl))
  ) {
    throw new Error(
      `--embeddings-url must be an http(s) URL, got "${embeddingsUrl}"`,
    );
  }
  const embeddings = embeddingsUrl
    ? {
      url: embeddingsUrl,
      apiKey: args["embeddings-api-key"] ?? env("QDRANT_EMBEDDINGS_API_KEY"),
    }
    : undefined;
  if (embeddings && !args.list) {
    console.error(
      `embedding "${LOCAL_PREFIX}<model>" texts via ${
        new URL(embeddings.url).host
      }`,
    );
  }

  const host = args.host ?? "127.0.0.1";
  const port = Number(args.port ?? 8000);
  if (!Number.isInteger(port) || port < 0 || port > 65535) {
    throw new Error(`--port must be 0-65535, got "${args.port}"`);
  }

  if (auth && !args.http && !args.list) {
    throw new Error(
      "Auth options only apply to the HTTP transport. Add --http, or remove " +
        "the auth options (and unset AUTH_* env vars) to run over stdio.",
    );
  }
  if (args.http && !args.list) {
    if (!auth) {
      const loopback = ["127.0.0.1", "::1", "localhost"].includes(host);
      console.error(
        loopback
          ? "warning: HTTP server is running WITHOUT auth (loopback only)."
          : `warning: HTTP server on ${host} is exposed WITHOUT auth. ` +
            "Set --jwks, --public-url and --auth-server.",
      );
    } else {
      if (!auth.issuer) {
        console.error(
          "warning: no --issuer set, tokens from any issuer sharing these keys are accepted.",
        );
      }
      if (!auth.audience) {
        console.error(
          "warning: no --audience set, tokens minted for other services are accepted.",
        );
      }
      console.error(
        `auth: verifying JWTs against ${auth.jwksUri}; discovery document at ` +
          `${auth.productionUrl}/.well-known/oauth-protected-resource/mcp`,
      );
    }
  }

  const { server, tools } = buildServer({
    spec: await loadSpec(
      args.openapi ?? Deno.env.get("QDRANT_OPENAPI") ?? DEFAULT_SPEC,
    ),
    url,
    apiKey: args["api-key"] ?? Deno.env.get("QDRANT_API_KEY"),
    enableWrites,
    auth,
    cloudInferenceExt: qciExtern,
    embeddings,
  });

  if (args.list) {
    console.log(
      `${tools.length} tools exposed (${
        enableWrites ? "read+write" : "read-only"
      }):`,
    );
    for (const t of tools.toSorted((a, b) => a.name.localeCompare(b.name))) {
      const kind = t.tags.includes(WRITER_TAG) ? "writer" : "reader";
      console.log(`  ${t.name.padEnd(28)} ${kind}`);
    }
    return;
  }

  if (args.http) {
    await server.run({
      transport: "http",
      host,
      port,
    });
  } else {
    await server.run({ transport: "stdio" });
  }
}

if (import.meta.main) {
  main().catch((err) => {
    console.error(err instanceof Error ? err.message : err);
    Deno.exit(1);
  });
}
