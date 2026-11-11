use std::sync::Arc;

use actix_web::{App, HttpServer, web};
use clap::Parser;
use qdrant_data_mcp::{
    QdrantServer, ServerOptions, VERSION, build_server,
    cli::Cli,
    embeddings::{EmbeddingsConfig, LOCAL_PREFIX},
    load_spec, qci,
};
use rmcp::{
    ServiceExt, transport::stdio,
    transport::streamable_http_server::session::local::LocalSessionManager,
};
use rmcp_actix_web::transport::StreamableHttpService;
use url::Url;

#[actix_web::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let cli = Cli::parse();
    if cli.version {
        println!("v{VERSION}");
        return Ok(());
    }
    init_logging();

    let base_url = Url::parse(&cli.url)
        .map_err(|e| format!("--url must be a valid URL, got \"{}\": {e}", cli.url))?;
    let embeddings = EmbeddingsConfig::resolve(cli.embeddings_url, cli.embeddings_api_key)?;
    let provider_keys = if cli.qci_external_providers && !cli.list {
        qci::resolve(&base_url, |name| std::env::var(name).ok())?
    } else {
        Vec::new()
    };

    if !cli.list {
        if !provider_keys.is_empty() {
            let providers: Vec<&str> = provider_keys.iter().map(|k| k.provider).collect();
            eprintln!(
                "forwarding {} API keys to {}",
                providers.join(", "),
                base_url.authority()
            );
        }
        if let Some(config) = &embeddings {
            eprintln!(
                "embedding \"{LOCAL_PREFIX}<model>\" texts via {}",
                config.url.authority()
            );
        }
        if cli.http {
            let loopback = matches!(cli.host.as_str(), "127.0.0.1" | "::1" | "localhost");
            if loopback {
                eprintln!("warning: HTTP server is running WITHOUT auth (loopback only).");
            } else {
                eprintln!(
                    "warning: HTTP server on {} is exposed WITHOUT auth.",
                    cli.host
                );
            }
        }
    }

    let server = build_server(ServerOptions {
        base_url,
        spec: load_spec(cli.openapi.as_deref()).await?,
        enable_writes: cli.enable_writes,
        headers: qci::build_headers(cli.api_key.as_deref(), &provider_keys)?,
        embeddings,
    })?;

    if cli.list {
        let tools = server.tool_listing();
        let mode = if cli.enable_writes {
            "read+write"
        } else {
            "read-only"
        };
        println!("{} tools exposed ({mode}):", tools.len());
        for (name, route) in tools {
            println!("  {name:<28} {}", route.label());
        }
        return Ok(());
    }

    if cli.http {
        serve_http(server, &cli.host, cli.port).await
    } else {
        serve_stdio(server).await
    }
}

fn init_logging() {
    // stdout belongs to the MCP protocol over stdio, so logs go to stderr
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("error"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
}

async fn serve_stdio(server: QdrantServer) -> Result<(), String> {
    let service = server
        .serve(stdio())
        .await
        .map_err(|e| format!("Could not start the stdio transport: {e}"))?;
    service
        .waiting()
        .await
        .map_err(|e| format!("The stdio transport stopped with an error: {e}"))?;
    Ok(())
}

async fn serve_http(server: QdrantServer, host: &str, port: u16) -> Result<(), String> {
    let service = StreamableHttpService::builder()
        .service_factory(Arc::new(move || Ok(server.clone())))
        .session_manager(LocalSessionManager::default().into())
        .stateful_mode(true)
        .build();
    let http_server = HttpServer::new(move || {
        App::new().service(web::scope("/mcp").service(service.clone().scope()))
    })
    .bind((host, port))
    .map_err(|e| format!("Could not bind {host}:{port}: {e}"))?
    .run();
    eprintln!("listening on http://{host}:{port}/mcp");
    http_server
        .await
        .map_err(|e| format!("The HTTP server stopped with an error: {e}"))
}
