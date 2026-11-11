use std::sync::Arc;

use actix_cors::Cors;
use actix_web::{App, HttpServer, middleware::from_fn, web};
use clap::Parser;
use qdrant_data_mcp::{
    QdrantServer, ServerOptions, VERSION,
    auth::{self, Auth, AuthInputs},
    build_server,
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
    let auth_config = auth::resolve(AuthInputs {
        jwks: cli.jwks,
        public_url: cli.public_url,
        auth_servers: cli.auth_server,
        issuer: cli.issuer,
        audience: cli.audience,
    })?;
    if auth_config.is_some() && !cli.http && !cli.list {
        return Err(
            "Auth options only apply to the HTTP transport. Add --http, or remove the \
            auth options (and unset AUTH_* env vars) to run over stdio."
                .to_string(),
        );
    }
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
            match &auth_config {
                None if matches!(cli.host.as_str(), "127.0.0.1" | "::1" | "localhost") => {
                    eprintln!("warning: HTTP server is running WITHOUT auth (loopback only).")
                }
                None => eprintln!(
                    "warning: HTTP server on {} is exposed WITHOUT auth. Set --jwks, \
                     --public-url and --auth-server.",
                    cli.host
                ),
                Some(config) => {
                    if config.issuer.is_none() {
                        eprintln!(
                            "warning: no --issuer set, tokens from any issuer sharing these keys are accepted."
                        );
                    }
                    if config.audience.is_empty() {
                        eprintln!(
                            "warning: no --audience set, tokens minted for other services are accepted."
                        );
                    }
                    eprintln!(
                        "auth: verifying JWTs against {}; discovery document at {}",
                        config.jwks_uri,
                        config.metadata_url()
                    );
                }
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
        let auth = auth_config
            .map(|config| Auth::new(config).map(web::Data::new))
            .transpose()?;
        serve_http(server, &cli.host, cli.port, auth).await
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

async fn serve_http(
    server: QdrantServer,
    host: &str,
    port: u16,
    auth: Option<web::Data<Auth>>,
) -> Result<(), String> {
    let service = StreamableHttpService::builder()
        .service_factory(Arc::new(move || Ok(server.clone())))
        .session_manager(LocalSessionManager::default().into())
        .stateful_mode(true)
        .build();
    let http_server = HttpServer::new(move || {
        App::new()
            .configure(|cfg| {
                if let Some(auth) = &auth {
                    cfg.app_data(auth.clone()).route(
                        "/.well-known/oauth-protected-resource/mcp",
                        web::get().to(auth::metadata),
                    );
                }
            })
            .service(
                web::scope("/mcp")
                    .wrap(from_fn(auth::require_token))
                    .service(service.clone().scope()),
            )
            .wrap(
                Cors::default()
                    .allow_any_origin()
                    .allow_any_method()
                    .allow_any_header()
                    .expose_any_header()
                    .max_age(3600),
            )
    })
    .bind((host, port))
    .map_err(|e| format!("Could not bind {host}:{port}: {e}"))?
    .run();
    eprintln!("listening on http://{host}:{port}/mcp");
    http_server
        .await
        .map_err(|e| format!("The HTTP server stopped with an error: {e}"))
}
