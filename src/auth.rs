use std::time::{Duration, Instant};

use actix_web::{
    Error, HttpResponse,
    body::{EitherBody, MessageBody},
    dev::ServiceRequest,
    dev::ServiceResponse,
    http::header::{AUTHORIZATION, WWW_AUTHENTICATE},
    middleware::Next,
    web,
};
use jsonwebtoken::{
    Algorithm, DecodingKey, Validation, decode, decode_header,
    jwk::{Jwk, JwkSet},
};
use serde_json::{Value, json};
use tokio::sync::RwLock;
use url::Url;

// Only asymmetric algorithms: HS* and "none" are never accepted.
const ALLOWED: [Algorithm; 9] = [
    Algorithm::RS256,
    Algorithm::RS384,
    Algorithm::RS512,
    Algorithm::PS256,
    Algorithm::PS384,
    Algorithm::PS512,
    Algorithm::ES256,
    Algorithm::ES384,
    Algorithm::EdDSA,
];
const KEYS_MAX_AGE: Duration = Duration::from_secs(600);
const REFETCH_COOLDOWN: Duration = Duration::from_secs(30);

#[derive(Debug, Default)]
pub struct AuthInputs {
    pub jwks: Option<String>,
    pub public_url: Option<String>,
    pub auth_servers: Vec<String>,
    pub issuer: Option<String>,
    pub audience: Vec<String>,
}

#[derive(Debug)]
pub struct AuthConfig {
    pub jwks_uri: String,
    pub public_url: String,
    pub authorization_servers: Vec<String>,
    pub issuer: Option<String>,
    pub audience: Vec<String>,
}

impl AuthConfig {
    pub fn metadata_url(&self) -> String {
        format!(
            "{}/.well-known/oauth-protected-resource/mcp",
            self.public_url
        )
    }
}

fn clean(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

/// Returns None only when no auth option was given. A partial configuration is
/// an error, so a forgotten flag never starts an unauthenticated server.
pub fn resolve(inputs: AuthInputs) -> Result<Option<AuthConfig>, String> {
    let jwks = inputs.jwks.filter(|v| !v.is_empty());
    let public_url = inputs.public_url.filter(|v| !v.is_empty());
    let issuer = inputs.issuer.filter(|v| !v.is_empty());
    let auth_servers = clean(inputs.auth_servers);
    let audience = clean(inputs.audience);
    if jwks.is_none()
        && public_url.is_none()
        && issuer.is_none()
        && auth_servers.is_empty()
        && audience.is_empty()
    {
        return Ok(None);
    }

    let missing: Vec<&str> = [
        jwks.is_none().then_some("--jwks (AUTH_JWKS_URI)"),
        public_url
            .is_none()
            .then_some("--public-url (AUTH_PUBLIC_URL)"),
        auth_servers
            .is_empty()
            .then_some("--auth-server (AUTH_AUTHORIZATION_SERVERS)"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !missing.is_empty() {
        return Err(format!(
            "Incomplete auth configuration, missing: {}",
            missing.join(", ")
        ));
    }

    let (jwks, public_url) = (jwks.unwrap(), public_url.unwrap());
    let urls = [("--jwks", &jwks), ("--public-url", &public_url)]
        .into_iter()
        .chain(auth_servers.iter().map(|url| ("--auth-server", url)));
    for (label, value) in urls {
        let valid = Url::parse(value).is_ok_and(|url| matches!(url.scheme(), "http" | "https"));
        if !valid {
            return Err(format!("{label} must be an http(s) URL, got \"{value}\""));
        }
    }
    Ok(Some(AuthConfig {
        jwks_uri: jwks,
        public_url: public_url.trim_end_matches('/').to_string(),
        authorization_servers: auth_servers,
        issuer,
        audience,
    }))
}

#[derive(Default)]
struct Keys {
    set: JwkSet,
    fetched: Option<Instant>,
    attempted: Option<Instant>,
}

pub struct Auth {
    pub config: AuthConfig,
    client: reqwest::Client,
    keys: RwLock<Keys>,
}

impl Auth {
    pub fn new(config: AuthConfig) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| format!("Could not build the JWKS HTTP client: {e}"))?;
        Ok(Self {
            config,
            client,
            keys: RwLock::default(),
        })
    }

    async fn fetch_keys(&self) -> Result<JwkSet, String> {
        let body: Value = self
            .client
            .get(&self.config.jwks_uri)
            .send()
            .await
            .and_then(|response| response.error_for_status())
            .map_err(|e| format!("Could not fetch the JWKS: {e}"))?
            .json()
            .await
            .map_err(|e| format!("The JWKS is not valid JSON: {e}"))?;
        // keep the keys we understand, so one exotic key doesn't break the rest
        let keys = body["keys"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|key| serde_json::from_value::<Jwk>(key.clone()).ok())
            .collect();
        Ok(JwkSet { keys })
    }

    // Uses the cached key, and refetches the set when the key is missing or
    // the cache is old, at most once per cooldown.
    async fn key_for(&self, kid: &str) -> Result<Jwk, String> {
        {
            let keys = self.keys.read().await;
            let fresh = keys.fetched.is_some_and(|t| t.elapsed() < KEYS_MAX_AGE);
            if fresh && let Some(jwk) = keys.set.find(kid) {
                return Ok(jwk.clone());
            }
        }
        let mut keys = self.keys.write().await;
        if keys
            .attempted
            .is_none_or(|t| t.elapsed() >= REFETCH_COOLDOWN)
        {
            keys.attempted = Some(Instant::now());
            match self.fetch_keys().await {
                Ok(set) => {
                    keys.set = set;
                    keys.fetched = Some(Instant::now());
                }
                Err(e) if keys.set.keys.is_empty() => return Err(e),
                Err(_) => {}
            }
        }
        keys.set
            .find(kid)
            .cloned()
            .ok_or_else(|| "unknown signing key".to_string())
    }

    pub async fn verify(&self, token: &str) -> Result<Value, String> {
        let header = decode_header(token).map_err(|e| format!("invalid token header: {e}"))?;
        if !ALLOWED.contains(&header.alg) {
            return Err(format!("algorithm {:?} is not allowed", header.alg));
        }
        let kid = header.kid.ok_or("the token has no kid")?;
        let key = DecodingKey::from_jwk(&self.key_for(&kid).await?)
            .map_err(|e| format!("unusable signing key: {e}"))?;

        let mut validation = Validation::new(header.alg);
        validation.leeway = 0;
        validation.validate_nbf = true;
        match &self.config.issuer {
            Some(issuer) => validation.set_issuer(&[issuer]),
            None => validation.iss = None,
        }
        if self.config.audience.is_empty() {
            validation.validate_aud = false;
        } else {
            validation.set_audience(&self.config.audience);
        }
        decode::<Value>(token, &key, &validation)
            .map(|data| data.claims)
            .map_err(|e| e.to_string())
    }

    #[cfg(test)]
    fn with_keys(config: AuthConfig, set: JwkSet) -> Self {
        let auth = Self::new(config).unwrap();
        *auth.keys.try_write().unwrap() = Keys {
            set,
            fetched: Some(Instant::now()),
            attempted: Some(Instant::now()),
        };
        auth
    }
}

fn unauthorized(auth: &Auth) -> HttpResponse {
    let challenge = format!(
        "Bearer realm=\"mcp\", resource_metadata=\"{}\"",
        auth.config.metadata_url()
    );
    HttpResponse::Unauthorized()
        .insert_header((WWW_AUTHENTICATE, challenge))
        .finish()
}

/// Requires a valid bearer token, when auth is configured.
pub async fn require_token<B: MessageBody + 'static>(
    auth: Option<web::Data<Auth>>,
    req: ServiceRequest,
    next: Next<B>,
) -> Result<ServiceResponse<EitherBody<B>>, Error> {
    if let Some(auth) = auth {
        let token = req
            .headers()
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split_once(' '))
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
            .map(|(_, token)| token.trim());
        let valid = match token {
            Some(token) => auth.verify(token).await.is_ok(),
            None => false,
        };
        if !valid {
            return Ok(req.into_response(unauthorized(&auth)).map_into_right_body());
        }
    }
    next.call(req).await.map(|res| res.map_into_left_body())
}

/// The RFC 9728 document that tells MCP clients which authorization servers to use.
pub async fn metadata(auth: web::Data<Auth>) -> HttpResponse {
    HttpResponse::Ok().json(json!({
        "resource": format!("{}/mcp", auth.config.public_url),
        "authorization_servers": auth.config.authorization_servers,
        "bearer_methods_supported": ["header"],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{App, middleware::from_fn, test as actix_test};
    use jsonwebtoken::{EncodingKey, Header};

    const PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----\n\
        MC4CAQAwBQYDK2VwBCIEIEgP13/GVn2iMoZBXd81xiKby6WTU01trE0AO/6uQxKX\n\
        -----END PRIVATE KEY-----\n";
    const PUBLIC_X: &str = "Iw4TSSnc3y3XJuNraMAJsKt6mWEVK8AhSKNJ1o4Oo6w";
    const ISSUER: &str = "https://idp.example.com/";

    fn config(issuer: Option<&str>, audience: &[&str]) -> AuthConfig {
        AuthConfig {
            jwks_uri: "http://127.0.0.1:1/jwks.json".to_string(), // nothing listens here
            public_url: "https://mcp.example.com".to_string(),
            authorization_servers: vec!["https://idp.example.com".to_string()],
            issuer: issuer.map(String::from),
            audience: audience.iter().map(|a| a.to_string()).collect(),
        }
    }

    fn auth(issuer: Option<&str>, audience: &[&str]) -> Auth {
        let set: JwkSet = serde_json::from_value(json!({ "keys": [
            { "kty": "OKP", "crv": "Ed25519", "x": PUBLIC_X, "kid": "k1", "alg": "EdDSA", "use": "sig" }
        ]}))
        .unwrap();
        Auth::with_keys(config(issuer, audience), set)
    }

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn token(iss: &str, aud: &str, exp_offset: i64) -> String {
        let claims =
            json!({ "iss": iss, "aud": aud, "sub": "alice", "exp": now() as i64 + exp_offset });
        let mut header = Header::new(Algorithm::EdDSA);
        header.kid = Some("k1".to_string());
        jsonwebtoken::encode(
            &header,
            &claims,
            &EncodingKey::from_ed_pem(PRIVATE_KEY.as_bytes()).unwrap(),
        )
        .unwrap()
    }

    #[actix_web::test]
    async fn a_valid_token_is_accepted() {
        let claims = auth(Some(ISSUER), &["mcp"])
            .verify(&token(ISSUER, "mcp", 3600))
            .await
            .unwrap();
        assert_eq!(claims["sub"], "alice");
    }

    #[actix_web::test]
    async fn issuer_audience_and_expiry_are_enforced() {
        let auth = auth(Some(ISSUER), &["mcp", "other"]);
        assert!(
            auth.verify(&token("https://evil.example.com/", "mcp", 3600))
                .await
                .is_err()
        );
        assert!(
            auth.verify(&token(ISSUER, "somebody-else", 3600))
                .await
                .is_err()
        );
        assert!(auth.verify(&token(ISSUER, "mcp", -60)).await.is_err());
        // any one of several audiences is enough
        assert!(auth.verify(&token(ISSUER, "other", 3600)).await.is_ok());
    }

    #[actix_web::test]
    async fn without_issuer_and_audience_only_the_signature_and_expiry_count() {
        let auth = auth(None, &[]);
        assert!(
            auth.verify(&token("whoever", "whatever", 3600))
                .await
                .is_ok()
        );
        assert!(
            auth.verify(&token("whoever", "whatever", -60))
                .await
                .is_err()
        );
    }

    #[actix_web::test]
    async fn symmetric_none_and_unknown_keys_are_refused() {
        let auth = auth(None, &[]);
        let claims = json!({ "sub": "mallory", "exp": now() + 3600 });
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some("k1".to_string());
        let forged =
            jsonwebtoken::encode(&header, &claims, &EncodingKey::from_secret(b"x")).unwrap();
        assert!(
            auth.verify(&forged)
                .await
                .unwrap_err()
                .contains("not allowed")
        );
        assert!(auth.verify("e30.e30.").await.is_err());

        let mut header = Header::new(Algorithm::EdDSA);
        header.kid = Some("other".to_string());
        let unknown = jsonwebtoken::encode(
            &header,
            &claims,
            &EncodingKey::from_ed_pem(PRIVATE_KEY.as_bytes()).unwrap(),
        )
        .unwrap();
        assert!(
            auth.verify(&unknown)
                .await
                .unwrap_err()
                .contains("unknown signing key")
        );
        assert!(auth.verify("not-a-jwt").await.is_err());
    }

    #[actix_web::test]
    async fn the_middleware_challenges_and_the_metadata_is_public() {
        let auth = web::Data::new(auth(Some(ISSUER), &["mcp"]));
        let app = actix_test::init_service(
            App::new()
                .app_data(auth.clone())
                .route(
                    "/.well-known/oauth-protected-resource/mcp",
                    web::get().to(metadata),
                )
                .service(
                    web::scope("/mcp")
                        .wrap(from_fn(require_token))
                        .route("", web::get().to(|| async { "ok" })),
                ),
        )
        .await;

        let res = actix_test::call_service(
            &app,
            actix_test::TestRequest::get().uri("/mcp").to_request(),
        )
        .await;
        assert_eq!(res.status(), 401);
        assert_eq!(
            res.headers().get(WWW_AUTHENTICATE).unwrap(),
            "Bearer realm=\"mcp\", resource_metadata=\"https://mcp.example.com/.well-known/oauth-protected-resource/mcp\""
        );

        let bad = actix_test::TestRequest::get()
            .uri("/mcp")
            .insert_header((
                AUTHORIZATION,
                format!("Bearer {}", token(ISSUER, "nope", 3600)),
            ))
            .to_request();
        assert_eq!(actix_test::call_service(&app, bad).await.status(), 401);

        let good = actix_test::TestRequest::get()
            .uri("/mcp")
            .insert_header((
                AUTHORIZATION,
                format!("bearer {}", token(ISSUER, "mcp", 3600)),
            ))
            .to_request();
        assert_eq!(actix_test::call_service(&app, good).await.status(), 200);

        let res = actix_test::call_service(
            &app,
            actix_test::TestRequest::get()
                .uri("/.well-known/oauth-protected-resource/mcp")
                .to_request(),
        )
        .await;
        let body: Value = actix_test::read_body_json(res).await;
        assert_eq!(body["resource"], "https://mcp.example.com/mcp");
        assert_eq!(
            body["authorization_servers"],
            json!(["https://idp.example.com"])
        );
        assert_eq!(body["bearer_methods_supported"], json!(["header"]));
    }

    fn inputs() -> AuthInputs {
        AuthInputs {
            jwks: Some("https://idp.example.com/jwks.json".into()),
            public_url: Some("https://mcp.example.com/".into()),
            auth_servers: vec!["https://idp.example.com".into()],
            ..AuthInputs::default()
        }
    }

    #[test]
    fn no_options_means_no_auth_but_a_partial_set_is_an_error() {
        assert!(resolve(AuthInputs::default()).unwrap().is_none());
        let err = resolve(AuthInputs {
            jwks: inputs().jwks,
            ..AuthInputs::default()
        })
        .unwrap_err();
        assert!(err.contains("--public-url") && err.contains("--auth-server"));
        assert!(!err.contains("--jwks"));
    }

    #[test]
    fn urls_are_validated_and_cleaned() {
        let config = resolve(AuthInputs {
            audience: vec![" mcp ".into(), "".into()],
            ..inputs()
        })
        .unwrap()
        .unwrap();
        assert_eq!(config.public_url, "https://mcp.example.com");
        assert_eq!(config.audience, ["mcp"]);
        assert_eq!(
            config.metadata_url(),
            "https://mcp.example.com/.well-known/oauth-protected-resource/mcp"
        );
        let bad = AuthInputs {
            jwks: Some("not a url".into()),
            ..inputs()
        };
        assert!(
            resolve(bad)
                .unwrap_err()
                .contains("--jwks must be an http(s) URL")
        );
    }
}
