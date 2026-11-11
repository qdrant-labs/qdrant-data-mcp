use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use url::Url;

// (provider, environment variable, header Qdrant Cloud Inference reads it from)
const PROVIDERS: [(&str, &str, &str); 4] = [
    ("openrouter", "OPENROUTER_API_KEY", "openrouter-api-key"),
    ("openai", "OPENAI_API_KEY", "openai-api-key"),
    ("cohere", "COHERE_API_KEY", "cohere-api-key"),
    ("jina", "JINA_API_KEY", "jina-api-key"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderKey {
    pub provider: &'static str,
    pub header: &'static str,
    pub key: String,
}

pub fn provider_keys(lookup: impl Fn(&str) -> Option<String>) -> Vec<ProviderKey> {
    PROVIDERS
        .iter()
        .filter_map(|(provider, var, header)| {
            lookup(var)
                .filter(|key| !key.is_empty())
                .map(|key| ProviderKey {
                    provider,
                    header,
                    key,
                })
        })
        .collect()
}

// The keys go out on every request, so refuse to send them over plain HTTP
// to a remote host.
pub fn assert_safe_url(url: &Url) -> Result<(), String> {
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if url.scheme() != "https" && !local {
        return Err(format!(
            "--qci-external-providers sends provider API keys to Qdrant, so --url must use https (got {}://{})",
            url.scheme(),
            url.authority()
        ));
    }
    Ok(())
}

/// Resolves the provider keys for `--qci-external-providers`, or fails loudly
/// so a typo'd variable doesn't become a silent no-op.
pub fn resolve(
    base_url: &Url,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<Vec<ProviderKey>, String> {
    let keys = provider_keys(lookup);
    if keys.is_empty() {
        let vars: Vec<&str> = PROVIDERS.iter().map(|(_, var, _)| *var).collect();
        return Err(format!(
            "--qci-external-providers is set, but none of {} is in the environment",
            vars.join(", ")
        ));
    }
    assert_safe_url(base_url)?;
    Ok(keys)
}

pub fn build_headers(
    api_key: Option<&str>,
    provider_keys: &[ProviderKey],
) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    let mut insert = |name: &'static str, value: &str| -> Result<(), String> {
        let mut value = HeaderValue::from_str(value)
            .map_err(|_| format!("The value for the {name} header contains invalid characters"))?;
        value.set_sensitive(true);
        headers.insert(HeaderName::from_static(name), value);
        Ok(())
    };
    if let Some(key) = api_key.filter(|key| !key.is_empty()) {
        insert("api-key", key)?;
    }
    for provider_key in provider_keys {
        insert(provider_key.header, &provider_key.key)?;
    }
    Ok(headers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup<'a>(pairs: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn reads_the_keys_that_are_set() {
        let keys = provider_keys(lookup(&[
            ("OPENROUTER_API_KEY", "or"),
            ("JINA_API_KEY", "jn"),
        ]));
        let providers: Vec<_> = keys.iter().map(|k| k.provider).collect();
        assert_eq!(providers, ["openrouter", "jina"]);
    }

    #[test]
    fn empty_values_count_as_unset() {
        assert!(provider_keys(lookup(&[("OPENAI_API_KEY", "")])).is_empty());
    }

    #[test]
    fn no_keys_is_an_error_naming_the_variables() {
        let url = Url::parse("https://q.example.com").unwrap();
        let err = resolve(&url, lookup(&[])).unwrap_err();
        assert!(err.contains("OPENAI_API_KEY") && err.contains("OPENROUTER_API_KEY"));
    }

    #[test]
    fn keys_are_only_sent_over_https_or_to_localhost() {
        for ok in [
            "https://xyz.cloud.qdrant.io:6333",
            "http://localhost:6333",
            "http://127.0.0.1:6333",
            "http://[::1]:6333",
        ] {
            assert!(assert_safe_url(&Url::parse(ok).unwrap()).is_ok(), "{ok}");
        }
        let err = assert_safe_url(&Url::parse("http://qdrant.internal:6333").unwrap()).unwrap_err();
        assert!(err.contains("must use https"));
    }

    #[test]
    fn each_provider_key_maps_to_its_header() {
        let keys = provider_keys(lookup(&[
            ("OPENAI_API_KEY", "o"),
            ("COHERE_API_KEY", "c"),
            ("JINA_API_KEY", "j"),
            ("OPENROUTER_API_KEY", "r"),
        ]));
        let headers = build_headers(Some("q"), &keys).unwrap();
        let get = |name: &str| headers.get(name).unwrap().to_str().unwrap().to_string();
        assert_eq!(get("api-key"), "q");
        assert_eq!(get("openai-api-key"), "o");
        assert_eq!(get("cohere-api-key"), "c");
        assert_eq!(get("jina-api-key"), "j");
        assert_eq!(get("openrouter-api-key"), "r");
        assert!(headers.get("api-key").unwrap().is_sensitive());
    }

    #[test]
    fn only_the_api_key_without_provider_keys() {
        let headers = build_headers(Some("q"), &[]).unwrap();
        assert_eq!(headers.len(), 1);
        assert!(build_headers(None, &[]).unwrap().is_empty());
    }
}
