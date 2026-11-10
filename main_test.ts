import { assertEquals } from "@std/assert";
import { buildServer } from "./main.ts";

// Minimal spec that mirrors the shapes Qdrant uses: tagged service/cluster
// routes, read-only POSTs, and real writes.
const op = (operationId: string, tags: string[]) => ({
  operationId,
  tags,
  responses: { "200": { description: "ok" } },
});
const SPEC = JSON.stringify({
  openapi: "3.0.1",
  info: { title: "t", version: "1" },
  paths: {
    "/healthz": { get: op("healthz", ["Service"]) },
    "/cluster": { get: op("cluster_status", ["Distributed"]) },
    "/collections": { get: op("get_collections", ["Collections"]) },
    "/collections/{collection_name}": {
      delete: {
        ...op("delete_collection", ["Collections"]),
        parameters: [{
          name: "collection_name",
          in: "path",
          required: true,
          schema: { type: "string" },
        }],
      },
    },
    "/collections/{collection_name}/points": {
      put: op("upsert_points", ["Points"]),
      post: op("get_points", ["Points"]),
    },
    "/collections/{collection_name}/points/query": {
      post: op("query_points", ["Search"]),
    },
    "/collections/{collection_name}/snapshots": {
      post: op("create_snapshot", ["Snapshots"]),
    },
  },
});

const names = (enableWrites: boolean) =>
  buildServer({ spec: SPEC, url: "http://localhost:6333", enableWrites })
    .tools.map((t) => t.name).sort();

Deno.test("read-only: only readers, no ops routes", () => {
  assertEquals(names(false), ["get_collections", "get_points", "query_points"]);
});

Deno.test("writes enabled: writers appear, ops routes stay excluded", () => {
  assertEquals(names(true), [
    "delete_collection",
    "get_collections",
    "get_points",
    "query_points",
    "upsert_points",
  ]);
});

Deno.test("writer tag is applied only to mutating tools", () => {
  const { tools } = buildServer({
    spec: SPEC,
    url: "http://localhost:6333",
    enableWrites: true,
  });
  const writers = tools.filter((t) => t.tags.includes("writer")).map((t) =>
    t.name
  ).sort();
  assertEquals(writers, ["delete_collection", "upsert_points"]);
});

import { assertThrows } from "@std/assert";
import {
  assertSafeQCIUrl,
  MCP_VERSION,
  qdrantVersion,
  resolveAuth,
  resolveHeaders,
  resolveQCIExternConfig,
  toList,
} from "./main.ts";

const NO_AUTH = { authServers: [], audience: [] };
const FULL = {
  jwks: "https://idp.example.com/jwks.json",
  publicUrl: "https://mcp.example.com/",
  authServers: ["https://idp.example.com"],
  audience: [],
};

Deno.test("auth: nothing set means no auth", () => {
  assertEquals(resolveAuth(NO_AUTH), undefined);
});

Deno.test("auth: partial config fails closed", () => {
  assertThrows(
    () => resolveAuth({ ...NO_AUTH, jwks: FULL.jwks }),
    Error,
    "--public-url",
  );
  assertThrows(
    () => resolveAuth({ ...FULL, authServers: [] }),
    Error,
    "--auth-server",
  );
});

Deno.test("auth: bad URLs rejected, trailing slash trimmed, empty audience unset", () => {
  assertThrows(
    () => resolveAuth({ ...FULL, jwks: "not a url" }),
    Error,
    "--jwks",
  );
  const cfg = resolveAuth(FULL)!;
  assertEquals(cfg.productionUrl, "https://mcp.example.com");
  assertEquals(cfg.audience, undefined);
});

Deno.test("toList splits commas and repeats", () => {
  assertEquals(toList(["a, b", "c", ""]), ["a", "b", "c"]);
  assertEquals(toList("x,y"), ["x", "y"]);
  assertEquals(toList(undefined), []);
});

Deno.test("server builds with auth configured", () => {
  const cfg = resolveAuth({ ...FULL, audience: ["qdrant-mcp"] })!;
  const { tools } = buildServer({
    spec: SPEC,
    url: "http://localhost:6333",
    enableWrites: false,
    auth: cfg,
  });
  assertEquals(tools.length, 3);
});

const QCI_ENV = [
  "OPENAI_API_KEY",
  "COHERE_API_KEY",
  "JINA_API_KEY",
  "OPENROUTER_API_KEY",
];

function withEnv(values: Record<string, string>, fn: () => void) {
  const saved = QCI_ENV.map((k) => [k, Deno.env.get(k)] as const);
  QCI_ENV.forEach((k) => Deno.env.delete(k));
  Object.entries(values).forEach(([k, v]) => Deno.env.set(k, v));
  try {
    fn();
  } finally {
    for (const [k, v] of saved) {
      if (v === undefined) Deno.env.delete(k);
      else Deno.env.set(k, v);
    }
  }
}

const BASE = { spec: "", url: "https://q.example.com", enableWrites: false };

Deno.test("qci: no provider keys in env resolves to undefined", () => {
  withEnv({}, () => assertEquals(resolveQCIExternConfig(), undefined));
});

Deno.test("qci: provider keys are read from env", () => {
  withEnv({ OPENROUTER_API_KEY: "or", JINA_API_KEY: "jn" }, () => {
    const cfg = resolveQCIExternConfig()!;
    assertEquals(cfg.openrouterKey, "or");
    assertEquals(cfg.jinaKey, "jn");
    assertEquals(cfg.openaiKey, undefined);
  });
});

Deno.test("headers: only api-key without the qci config", () => {
  assertEquals(resolveHeaders({ ...BASE, apiKey: "q" }), { "api-key": "q" });
  assertEquals(resolveHeaders(BASE), {});
});

Deno.test("headers: each provider key maps to its header", () => {
  const headers = resolveHeaders({
    ...BASE,
    apiKey: "q",
    cloudInferenceExt: {
      openaiKey: "o",
      cohereKey: "c",
      jinaKey: "j",
      openrouterKey: "r",
    },
  });
  assertEquals(headers, {
    "api-key": "q",
    "openai-api-key": "o",
    "cohere-api-key": "c",
    "jina-api-key": "j",
    "openrouter-api-key": "r",
  });
});

Deno.test("qci: keys are only sent over https or to localhost", () => {
  assertSafeQCIUrl("https://xyz.cloud.qdrant.io:6333");
  assertSafeQCIUrl("http://localhost:6333");
  assertSafeQCIUrl("http://127.0.0.1:6333");
  assertThrows(
    () => assertSafeQCIUrl("http://qdrant.internal:6333"),
    Error,
    "must use https",
  );
});

Deno.test("version: the Qdrant version is the MCP version without the server revision", () => {
  assertEquals(qdrantVersion("v1.20.0-server.3"), "v1.20.0");
  assertEquals(qdrantVersion("1.21.0-server.1"), "v1.21.0");
  assertEquals(qdrantVersion("v1.20.0-server.2-rc.1"), "v1.20.0");
  assertEquals(qdrantVersion("v1.20.0"), "v1.20.0");
  assertThrows(() => qdrantVersion("server.1"), Error, "Cannot read");
});

Deno.test("version: the version in deno.json is v<qdrant>-server.<n>", () => {
  assertEquals(/^v\d+\.\d+\.\d+-server\.\d+$/.test(MCP_VERSION), true);
});
