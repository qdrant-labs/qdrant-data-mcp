use std::{sync::Arc, time::Duration};

use reqwest::header::HeaderMap;
use rmcp::{
    handler::server::ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, ErrorData, InitializeResult, ListToolsResult,
        PaginatedRequestParams, Tool as McpTool,
    },
    service::{RequestContext, RoleServer},
};
use rmcp_openapi::Server;
use serde_json::Value;
use url::Url;

use crate::{
    VERSION,
    compact::compact,
    embeddings::{self, EmbeddingsConfig},
    guide, refs,
    routes::{self, Route},
};

pub struct ServerOptions {
    pub base_url: Url,
    pub spec: Value,
    pub enable_writes: bool,
    pub headers: HeaderMap,
    pub embeddings: Option<EmbeddingsConfig>,
    /// Model prefixes (such as `openai/`) of the providers whose keys are sent to Qdrant.
    pub provider_prefixes: Vec<&'static str>,
}

struct Embedder {
    client: reqwest::Client,
    config: EmbeddingsConfig,
}

impl Embedder {
    async fn rewrite(&self, arguments: &mut Value) -> Result<(), String> {
        for group in embeddings::plan(arguments) {
            let vectors = embeddings::request_embeddings(
                &self.client,
                &self.config,
                &group.model,
                &group.texts,
            )
            .await?;
            embeddings::apply(arguments, &group, vectors);
        }
        Ok(())
    }
}

/// The generated OpenAPI server, plus the `local/` embeddings rewrite.
#[derive(Clone)]
pub struct QdrantServer {
    inner: Server,
    tools: Arc<Vec<McpTool>>,
    instructions: Arc<String>,
    embedder: Option<Arc<Embedder>>,
}

pub fn build_server(options: ServerOptions) -> Result<QdrantServer, String> {
    let mut spec = options.spec;
    routes::prune_spec(&mut spec, options.enable_writes);
    refs::break_cycles(&mut spec);

    let mut inner = Server::builder()
        .openapi_spec(spec)
        .base_url(options.base_url)
        .default_headers(options.headers)
        .name("Qdrant Data MCP".to_string())
        .version(VERSION.to_string())
        .build();
    inner
        .load_openapi_spec()
        .map_err(|e| format!("Error while loading OpenAPI spec into MCP: {e}"))?;
    inner
        .validate_registry()
        .map_err(|e| format!("Invalid tool registry: {e}"))?;

    let instructions = guide::instructions(
        options.enable_writes,
        options.embeddings.is_some(),
        &options.provider_prefixes,
    );
    let embedder = options
        .embeddings
        .map(|config| {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|e| format!("Could not build the embeddings HTTP client: {e}"))?;
            Ok::<_, String>(Arc::new(Embedder { client, config }))
        })
        .transpose()?;

    let tools = inner
        .tool_collection
        .to_mcp_tools()
        .into_iter()
        .map(|mut tool| {
            tool.input_schema = Arc::new(compact(&tool.input_schema));
            tool.output_schema = tool.output_schema.map(|schema| Arc::new(compact(&schema)));
            tool
        })
        .collect();

    Ok(QdrantServer {
        inner,
        tools: Arc::new(tools),
        instructions: Arc::new(instructions),
        embedder,
    })
}

impl QdrantServer {
    /// Tool names with their kind, sorted by name.
    pub fn tool_listing(&self) -> Vec<(String, Route)> {
        let mut tools: Vec<(String, Route)> = self
            .inner
            .get_tool_names()
            .into_iter()
            .map(|name| {
                let route = self
                    .inner
                    .get_tool_metadata(&name)
                    .map(|meta| routes::classify(&meta.method, &meta.path, &[]))
                    .unwrap_or(Route::Reader);
                (name, route)
            })
            .collect();
        tools.sort_by(|a, b| a.0.cmp(&b.0));
        tools
    }
}

impl ServerHandler for QdrantServer {
    fn get_info(&self) -> InitializeResult {
        let mut info = self.inner.get_info();
        info.instructions = Some(self.instructions.to_string());
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(self.tools.to_vec()))
    }

    async fn call_tool(
        &self,
        mut request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if let Some(embedder) = &self.embedder
            && let Some(arguments) = request.arguments.take()
        {
            let mut value = Value::Object(arguments);
            embedder
                .rewrite(&mut value)
                .await
                .map_err(|e| ErrorData::internal_error(e, None))?;
            if let Value::Object(map) = value {
                request.arguments = Some(map);
            }
        }
        self.inner.call_tool(request, context).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_server_sends_usage_guidance_in_its_instructions() {
        let spec = json!({
            "openapi": "3.0.1",
            "info": { "title": "t", "version": "1" },
            "paths": { "/collections": { "get": {
                "operationId": "get_collections",
                "tags": ["Collections"],
                "responses": { "200": { "description": "ok" } }
            }}}
        });
        let server = build_server(ServerOptions {
            base_url: Url::parse("http://localhost:6333").unwrap(),
            spec,
            enable_writes: false,
            headers: HeaderMap::new(),
            embeddings: None,
            provider_prefixes: Vec::new(),
        })
        .unwrap();

        let info = server.get_info();
        assert!(info.capabilities.tools.is_some());
        assert!(info.capabilities.resources.is_none());
        let instructions = info.instructions.unwrap();
        assert!(instructions.contains("request_body") && instructions.contains("read-only"));
    }
}
