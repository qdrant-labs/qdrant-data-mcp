pub mod auth;
pub mod cli;
pub mod compact;
pub mod embeddings;
pub mod qci;
pub mod refs;
pub mod routes;
pub mod server;

use regex::Regex;
use serde_json::Value;

pub use server::{QdrantServer, ServerOptions, build_server};

// "1.20.0-server.1": the Qdrant version this targets, plus the server revision.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn qdrant_version(version: &str) -> Result<String, String> {
    let re = Regex::new(r"^v?(\d+\.\d+\.\d+)(-|$)").expect("Version regex should compile");
    re.captures(version)
        .and_then(|captures| captures.get(1))
        .map(|m| format!("v{}", m.as_str()))
        .ok_or_else(|| format!("Cannot read a Qdrant version from \"{version}\""))
}

pub fn default_spec_url() -> Result<String, String> {
    let version = qdrant_version(VERSION)?;
    Ok(format!(
        "https://raw.githubusercontent.com/qdrant/qdrant/{version}/docs/redoc/master/openapi.json"
    ))
}

async fn fetch_spec(url: &str) -> Result<Value, String> {
    let response = reqwest::get(url)
        .await
        .map_err(|e| format!("Error while fetching the OpenAPI spec from {url}: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        let details = response.text().await.unwrap_or_default();
        return Err(format!(
            "Fetching the OpenAPI spec from {url} failed ({}): {details}",
            status.as_u16()
        ));
    }
    response
        .json()
        .await
        .map_err(|e| format!("The OpenAPI spec from {url} is not valid JSON: {e}"))
}

/// Loads the spec from a URL or a file, or the default one for this version.
pub async fn load_spec(source: Option<&str>) -> Result<Value, String> {
    match source {
        Some(url) if url.starts_with("http://") || url.starts_with("https://") => {
            fetch_spec(url).await
        }
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| format!("Could not read the OpenAPI spec at {path}: {e}"))?;
            serde_json::from_str(&text)
                .map_err(|e| format!("The OpenAPI spec at {path} is not valid JSON: {e}"))
        }
        None => fetch_spec(&default_spec_url()?).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_qdrant_version_is_the_version_without_the_server_revision() {
        assert_eq!(qdrant_version("1.20.0-server.3").unwrap(), "v1.20.0");
        assert_eq!(qdrant_version("v1.21.0-server.1").unwrap(), "v1.21.0");
        assert_eq!(qdrant_version("1.20.0-server.2-rc.1").unwrap(), "v1.20.0");
        assert_eq!(qdrant_version("1.20.0").unwrap(), "v1.20.0");
        assert!(qdrant_version("server.1").is_err());
    }

    #[test]
    fn the_package_version_is_qdrant_version_dash_server_revision() {
        let re = Regex::new(r"^\d+\.\d+\.\d+-server\.\d+$").unwrap();
        assert!(re.is_match(VERSION), "{VERSION}");
        assert!(default_spec_url().unwrap().contains("/qdrant/v"));
    }
}
