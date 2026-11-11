use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "qdrant-data-mcp",
    about = "Qdrant data MCP server (from OpenAPI)",
    disable_version_flag = true
)]
pub struct Cli {
    /// Qdrant REST URL
    #[arg(long, env = "QDRANT_URL", default_value = "http://localhost:6333")]
    pub url: String,

    /// Qdrant API key
    #[arg(long, env = "QDRANT_API_KEY", hide_env_values = true)]
    pub api_key: Option<String>,

    /// OpenAPI spec path or URL (default: the spec for this server's Qdrant version)
    #[arg(long, env = "QDRANT_OPENAPI")]
    pub openapi: Option<String>,

    /// Expose insert/update/delete tools
    #[arg(long, env = "QDRANT_WRITES_ENABLED")]
    pub enable_writes: bool,

    /// Send OPENAI_API_KEY, COHERE_API_KEY, JINA_API_KEY, and OPENROUTER_API_KEY from the env
    /// to Qdrant Cloud Inference as headers (requires an https --url)
    #[arg(long)]
    pub qci_external_providers: bool,

    /// OpenAI-compatible embeddings endpoint, for example Ollama's http://localhost:11434/v1.
    /// Texts sent with a "local/<model>" model are embedded there before reaching Qdrant
    #[arg(long, env = "QDRANT_EMBEDDINGS_URL")]
    pub embeddings_url: Option<String>,

    /// Bearer token for the embeddings endpoint
    #[arg(long, env = "QDRANT_EMBEDDINGS_API_KEY", hide_env_values = true)]
    pub embeddings_api_key: Option<String>,

    /// Serve streamable HTTP instead of stdio
    #[arg(long)]
    pub http: bool,

    /// HTTP host
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,

    /// HTTP port
    #[arg(long, default_value_t = 8000)]
    pub port: u16,

    /// Print the exposed tools and exit
    #[arg(long)]
    pub list: bool,

    /// Print the version and exit
    #[arg(short = 'v', long)]
    pub version: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let cli = Cli::try_parse_from(["qdrant-data-mcp"]).unwrap();
        assert_eq!(cli.host, "127.0.0.1");
        assert_eq!(cli.port, 8000);
        assert!(!cli.http && !cli.list && !cli.enable_writes && !cli.qci_external_providers);
    }

    #[test]
    fn flags() {
        let cli = Cli::try_parse_from([
            "qdrant-data-mcp",
            "--url",
            "https://x.cloud.qdrant.io:6333",
            "--api-key",
            "k",
            "--enable-writes",
            "--http",
            "--port",
            "9000",
            "--embeddings-url",
            "http://localhost:11434/v1",
        ])
        .unwrap();
        assert_eq!(cli.url, "https://x.cloud.qdrant.io:6333");
        assert_eq!(cli.api_key.as_deref(), Some("k"));
        assert!(cli.enable_writes && cli.http);
        assert_eq!(cli.port, 9000);
        assert_eq!(
            cli.embeddings_url.as_deref(),
            Some("http://localhost:11434/v1")
        );
    }

    #[test]
    fn a_port_out_of_range_is_rejected() {
        assert!(Cli::try_parse_from(["qdrant-data-mcp", "--port", "70000"]).is_err());
    }
}
