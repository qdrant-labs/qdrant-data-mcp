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

const VERSION = "v1.20.0"
const DEFAULT_SPEC =
  `https://raw.githubusercontent.com/qdrant/qdrant/${VERSION}/docs/redoc/master/openapi.json`;
const WRITER_TAG = "writer";
const READER_TAG = "reader";

const HELP = `qdrant-data-mcp: Qdrant data MCP server (from OpenAPI)

Options:
  --url <url>        Qdrant REST URL             (env: QDRANT_URL, default http://localhost:6333)
  --api-key <key>    Qdrant API key              (env: QDRANT_API_KEY)
  --openapi <src>    OpenAPI spec path or URL    (env: QDRANT_OPENAPI)
  --enable-writes    Expose insert/update/delete (env: QDRANT_WRITES_ENABLED=1)
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

export interface ServerConfig {
  spec: string;
  url: string;
  apiKey?: string;
  enableWrites: boolean;
  auth?: AuthConfig;
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
  const server = createOpenAPIServer({
    spec: cfg.spec,
    name: "Qdrant Data MCP",
    version: VERSION,
    client: {
      baseUrl: cfg.url,
      headers: cfg.apiKey ? { "api-key": cfg.apiKey } : {},
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
    ],
    collect: ["audience", "auth-server"],
    boolean: ["enable-writes", "http", "list", "help", "version"],
    alias: { h: "help", v: "version" },
  });

  if (args.help) {
    console.log(HELP);
    return;
  }

  if (args.version) {
    console.log(VERSION);
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
    url: args.url ?? Deno.env.get("QDRANT_URL") ?? "http://localhost:6333",
    apiKey: args["api-key"] ?? Deno.env.get("QDRANT_API_KEY"),
    enableWrites,
    auth,
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
