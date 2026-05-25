//! OAuth 2.0 + PKCE initial authorization flow
//!
//! Orchestrates the full PKCE flow:
//! 1. Generate verifier + challenge
//! 2. Build authorization URL
//! 3. Exchange authorization code for tokens
//!
//! ```rust,ignore
//! let flow = OAuthFlow::new();
//! let url = flow.authorization_url(&config);
//! // ... redirect user to url, receive code ...
//! let credential = flow.exchange_code("code", &config).await?;
//! ```

use base64::Engine as _;
use chrono::{Duration, Utc};
use serde::Deserialize;

use crate::model_router::oauth_credential::Credential;
use crate::model_router::pkce;
use crate::model_router::OAuthConfig;

/// Orchestrates an OAuth 2.0 + PKCE authorization flow.
#[derive(Debug)]
pub struct OAuthFlow {
    client: reqwest::Client,
    verifier: String,
    state: String,
}

impl OAuthFlow {
    /// Create a new flow with a fresh PKCE verifier and random state.
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            verifier: pkce::generate_verifier(),
            state: generate_state(),
        }
    }

    /// The PKCE code verifier (needed when exchanging the code).
    pub fn verifier(&self) -> &str {
        &self.verifier
    }

    /// The random state parameter (validate this in the callback).
    pub fn state(&self) -> &str {
        &self.state
    }

    /// Build the authorization URL to send the user to.
    pub fn authorization_url(&self, config: &OAuthConfig) -> String {
        let challenge = pkce::challenge_from_verifier(&self.verifier);
        let redirect_uri = format!("http://127.0.0.1:{}/callback", config.redirect_port);

        let mut url = format!(
            "{}?response_type=code&client_id={}&redirect_uri={}&code_challenge={}&code_challenge_method=S256&state={}",
            urlencoding::encode(&config.auth_url),
            urlencoding::encode(&config.client_id),
            urlencoding::encode(&redirect_uri),
            urlencoding::encode(&challenge),
            urlencoding::encode(&self.state),
        );

        if let Some(ref scope) = config.scope {
            url.push_str(&format!("&scope={}", urlencoding::encode(scope)));
        }

        url
    }

    /// Exchange an authorization code for an access token (and optional refresh token).
    pub async fn exchange_code(
        &self,
        code: &str,
        config: &OAuthConfig,
    ) -> crate::Result<Credential> {
        let redirect_uri = format!("http://127.0.0.1:{}/callback", config.redirect_port);

        let params = [
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", config.client_id.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("code_verifier", self.verifier.as_str()),
        ];

        let resp = self
            .client
            .post(&config.token_url)
            .form(&params)
            .send()
            .await
            .map_err(|e| crate::error::MantaError::ExternalService {
                source: format!("OAuth token exchange request failed: {}", e),
                cause: Some(Box::new(e)),
            })?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(crate::error::MantaError::ExternalService {
                source: format!("OAuth token exchange failed: {}", body),
                cause: None,
            });
        }

        let data: TokenResponse =
            resp.json()
                .await
                .map_err(|e| crate::error::MantaError::ExternalService {
                    source: format!("OAuth token exchange response invalid: {}", e),
                    cause: Some(Box::new(e)),
                })?;

        let expires_at = Utc::now() + Duration::seconds(data.expires_in as i64);

        Ok(Credential::OAuth2 {
            access_token: data.access_token,
            refresh_token: data.refresh_token,
            expires_at,
            token_url: config.token_url.clone(),
            client_id: config.client_id.clone(),
            client_secret: None,
            scope: config.scope.clone(),
        })
    }
}

impl Default for OAuthFlow {
    fn default() -> Self {
        Self::new()
    }
}

/// OAuth2 token endpoint response (authorization_code flow).
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct TokenResponse {
    access_token: String,
    #[serde(default = "default_expires_in")]
    expires_in: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    token_type: Option<String>,
}

fn default_expires_in() -> u64 {
    3600
}

fn generate_state() -> String {
    let bytes: Vec<u8> = (0..32).map(|_| rand::random::<u8>()).collect();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes)
}
