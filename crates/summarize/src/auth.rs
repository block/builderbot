use anyhow::Result;
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use url::Url;

/// Authentication method for the Databricks API.
#[derive(Debug, Clone)]
pub enum Auth {
    /// Bearer token (API key / PAT)
    Token(String),
    /// OAuth 2.0 with PKCE (browser-based flow with token caching + refresh)
    OAuth {
        host: String,
        client_id: String,
        redirect_url: String,
        scopes: Vec<String>,
    },
}

const DEFAULT_CLIENT_ID: &str = "databricks-cli";
const DEFAULT_REDIRECT_URL: &str = "http://localhost";
const DEFAULT_SCOPES: &[&str] = &["all-apis", "offline_access"];

impl Auth {
    /// Create OAuth auth from just a host URL, using Databricks CLI defaults.
    pub fn oauth(host: String) -> Self {
        Self::OAuth {
            host,
            client_id: DEFAULT_CLIENT_ID.to_string(),
            redirect_url: DEFAULT_REDIRECT_URL.to_string(),
            scopes: DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Create token-based auth.
    pub fn token(token: String) -> Self {
        Self::Token(token)
    }

    /// Resolve auth from environment variables.
    /// Checks DATABRICKS_TOKEN first, then falls back to OAuth via DATABRICKS_HOST.
    pub fn from_env() -> Result<Self> {
        if let Ok(token) = std::env::var("DATABRICKS_TOKEN") {
            if !token.is_empty() {
                return Ok(Self::token(token));
            }
        }

        let host = std::env::var("DATABRICKS_HOST").map_err(|_| {
            anyhow::anyhow!(
                "Must set either DATABRICKS_TOKEN or DATABRICKS_HOST environment variable"
            )
        })?;

        if host.is_empty() {
            anyhow::bail!("DATABRICKS_HOST is empty");
        }

        Ok(Self::oauth(host))
    }

    /// Get a bearer token (resolving OAuth if needed).
    pub async fn get_token(&self) -> Result<String> {
        match self {
            Auth::Token(token) => Ok(token.clone()),
            Auth::OAuth {
                host,
                client_id,
                redirect_url,
                scopes,
            } => get_oauth_token(host, client_id, redirect_url, scopes).await,
        }
    }
}

// ── OAuth Implementation ────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct TokenData {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: Option<DateTime<Utc>>,
}

fn cache_dir() -> PathBuf {
    let base = dirs_or_fallback();
    base.join("summarize").join("oauth")
}

fn dirs_or_fallback() -> PathBuf {
    if let Some(config) = dirs::config_dir() {
        config
    } else if let Ok(home) = std::env::var("HOME") {
        Path::new(&home).join(".config")
    } else {
        PathBuf::from("/tmp")
    }
}

struct TokenCache {
    cache_path: PathBuf,
}

impl TokenCache {
    fn new(host: &str, client_id: &str, scopes: &[String]) -> Self {
        let mut hasher = sha2::Sha256::new();
        hasher.update(host.as_bytes());
        hasher.update(client_id.as_bytes());
        hasher.update(scopes.join(",").as_bytes());
        let hash = format!("{:x}", hasher.finalize());

        let dir = cache_dir();
        let _ = std::fs::create_dir_all(&dir);
        let cache_path = dir.join(format!("{}.json", hash));

        Self { cache_path }
    }

    fn load(&self) -> Option<TokenData> {
        let contents = std::fs::read_to_string(&self.cache_path).ok()?;
        let token: TokenData = serde_json::from_str(&contents).ok()?;
        // Only use cached tokens that have a refresh token
        if token.refresh_token.is_some() {
            Some(token)
        } else {
            None
        }
    }

    fn save(&self, token: &TokenData) -> Result<()> {
        if let Some(parent) = self.cache_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let contents = serde_json::to_string(token)?;
        std::fs::write(&self.cache_path, contents)?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct OidcEndpoints {
    authorization_endpoint: String,
    token_endpoint: String,
}

async fn get_oidc_endpoints(host: &str) -> Result<OidcEndpoints> {
    let base_url = Url::parse(host)?;
    let oidc_url = base_url.join("oidc/.well-known/oauth-authorization-server")?;

    let client = reqwest::Client::new();
    let resp = client.get(oidc_url.as_str()).send().await?;

    if !resp.status().is_success() {
        anyhow::bail!("Failed to get OIDC config from {}", oidc_url);
    }

    let config: serde_json::Value = resp.json().await?;

    let authorization_endpoint = config
        .get("authorization_endpoint")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("authorization_endpoint not found"))?
        .to_string();

    let token_endpoint = config
        .get("token_endpoint")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("token_endpoint not found"))?
        .to_string();

    Ok(OidcEndpoints {
        authorization_endpoint,
        token_endpoint,
    })
}

struct OAuthFlow {
    endpoints: OidcEndpoints,
    client_id: String,
    #[allow(dead_code)]
    redirect_url: String,
    scopes: Vec<String>,
    state: String,
    verifier: String,
}

impl OAuthFlow {
    fn new(
        endpoints: OidcEndpoints,
        client_id: String,
        redirect_url: String,
        scopes: Vec<String>,
    ) -> Self {
        Self {
            endpoints,
            client_id,
            redirect_url,
            scopes,
            state: nanoid::nanoid!(16),
            verifier: nanoid::nanoid!(64),
        }
    }

    fn extract_token_data(
        &self,
        resp: &serde_json::Value,
        old_refresh_token: Option<&str>,
    ) -> Result<TokenData> {
        let access_token = resp
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("access_token not found in response"))?
            .to_string();

        let refresh_token = resp
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| old_refresh_token.map(|s| s.to_string()));

        let expires_at = resp
            .get("expires_in")
            .and_then(|v| v.as_u64())
            .map(|expires_in| Utc::now() + chrono::Duration::seconds(expires_in as i64));

        Ok(TokenData {
            access_token,
            refresh_token,
            expires_at,
        })
    }

    fn authorization_url(&self, redirect_url: &str) -> String {
        let challenge = {
            let digest = sha2::Sha256::digest(self.verifier.as_bytes());
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
        };

        let params = [
            ("response_type", "code"),
            ("client_id", &self.client_id),
            ("redirect_uri", redirect_url),
            ("scope", &self.scopes.join(" ")),
            ("state", &self.state),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
        ];

        format!(
            "{}?{}",
            self.endpoints.authorization_endpoint,
            serde_urlencoded::to_string(params).unwrap()
        )
    }

    async fn exchange_code(&self, code: &str, redirect_url: &str) -> Result<TokenData> {
        let params = [
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_url),
            ("code_verifier", &self.verifier),
            ("client_id", &self.client_id),
        ];

        let client = reqwest::Client::new();
        let resp = client
            .post(&self.endpoints.token_endpoint)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&params)
            .send()
            .await?;

        if !resp.status().is_success() {
            let err = resp.text().await?;
            anyhow::bail!("Failed to exchange code for token: {}", err);
        }

        let json: serde_json::Value = resp.json().await?;
        self.extract_token_data(&json, None)
    }

    async fn refresh_token(&self, refresh_token: &str) -> Result<TokenData> {
        let params = [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", &self.client_id),
        ];

        let client = reqwest::Client::new();
        let resp = client
            .post(&self.endpoints.token_endpoint)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&params)
            .send()
            .await?;

        if !resp.status().is_success() {
            let err = resp.text().await?;
            anyhow::bail!("Failed to refresh token: {}", err);
        }

        let json: serde_json::Value = resp.json().await?;
        self.extract_token_data(&json, Some(refresh_token))
    }

    async fn browser_flow(&self) -> Result<TokenData> {
        use axum::{extract::Query, response::Html, routing::get, Router};
        use std::net::SocketAddr;
        use tokio::sync::oneshot;

        let (tx, rx) = oneshot::channel();
        let state = self.state.clone();
        let tx = Arc::new(Mutex::new(Some(tx)));

        let app = Router::new().route(
            "/",
            get(move |Query(params): Query<HashMap<String, String>>| {
                let tx = Arc::clone(&tx);
                let state = state.clone();
                async move {
                    let code = params.get("code").cloned();
                    let received_state = params.get("state").cloned();

                    if let (Some(code), Some(received_state)) = (code, received_state) {
                        if received_state == state {
                            if let Some(sender) = tx.lock().await.take() {
                                if sender.send(code).is_ok() {
                                    return Html(
                                        "<h2>Login Success</h2><p>You can close this window.</p>",
                                    );
                                }
                            }
                            Html("<h2>Error</h2><p>Authentication already completed.</p>")
                        } else {
                            Html("<h2>Error</h2><p>State mismatch.</p>")
                        }
                    } else {
                        Html("<h2>Error</h2><p>Authentication failed.</p>")
                    }
                }
            }),
        );

        let addr = SocketAddr::from(([127, 0, 0, 1], 0));
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let actual_port = listener.local_addr()?.port();

        let server_handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let actual_redirect_url = format!("http://localhost:{}", actual_port);
        let authorization_url = self.authorization_url(&actual_redirect_url);

        eprintln!("Opening browser for Databricks OAuth login...");
        if opener::open(&authorization_url).is_err() {
            eprintln!(
                "Could not open browser. Please visit:\n{}",
                authorization_url
            );
        }

        let code = tokio::time::timeout(std::time::Duration::from_secs(120), rx)
            .await
            .map_err(|_| anyhow::anyhow!("OAuth login timed out after 120 seconds"))??;

        server_handle.abort();

        self.exchange_code(&code, &actual_redirect_url).await
    }
}

async fn get_oauth_token(
    host: &str,
    client_id: &str,
    redirect_url: &str,
    scopes: &[String],
) -> Result<String> {
    let cache = TokenCache::new(host, client_id, scopes);

    // Try cache first
    if let Some(token) = cache.load() {
        // Check if still valid
        if let Some(expires_at) = token.expires_at {
            if expires_at > Utc::now() {
                return Ok(token.access_token);
            }
            // Expired — try refresh
        } else {
            // No expiration info — assume valid
            return Ok(token.access_token);
        }

        // Try to refresh
        if let Some(refresh_token) = &token.refresh_token {
            if let Ok(endpoints) = get_oidc_endpoints(host).await {
                let flow = OAuthFlow::new(
                    endpoints,
                    client_id.to_string(),
                    redirect_url.to_string(),
                    scopes.to_vec(),
                );
                if let Ok(new_token) = flow.refresh_token(refresh_token).await {
                    let _ = cache.save(&new_token);
                    return Ok(new_token.access_token);
                }
            }
            // Refresh failed, fall through to new flow
        }
    }

    // Full browser-based OAuth flow
    let endpoints = get_oidc_endpoints(host).await?;
    let flow = OAuthFlow::new(
        endpoints,
        client_id.to_string(),
        redirect_url.to_string(),
        scopes.to_vec(),
    );

    let token = flow.browser_flow().await?;
    cache.save(&token)?;
    Ok(token.access_token)
}
