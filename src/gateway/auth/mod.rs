//! Gateway Authentication Module
//!
//! Provides session cookie management and OAuth2 authentication flows
//! to complement the existing Bearer token auth in `security::AuthManager`.

use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::Response,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, warn};

use crate::gateway::GatewayState;

pub mod oauth;

/// Session cookie configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCookieConfig {
    /// Cookie name
    pub name: String,
    /// Cookie domain
    pub domain: Option<String>,
    /// Cookie path
    pub path: String,
    /// Secure flag (HTTPS only)
    pub secure: bool,
    /// HttpOnly flag
    pub http_only: bool,
    /// SameSite policy
    pub same_site: String,
    /// Max age in seconds
    pub max_age_secs: i64,
}

impl Default for SessionCookieConfig {
    fn default() -> Self {
        Self {
            name: "manta_session".to_string(),
            domain: None,
            path: "/".to_string(),
            secure: true,
            http_only: true,
            same_site: "lax".to_string(),
            max_age_secs: 86400 * 7, // 7 days
        }
    }
}

/// Extract session token from cookie header
pub fn extract_session_cookie(req: &Request, cookie_name: &str) -> Option<String> {
    let cookie_header = req.headers().get(header::COOKIE)?;
    let cookie_str = cookie_header.to_str().ok()?;

    for cookie in cookie_str.split(';') {
        let mut parts = cookie.trim().splitn(2, '=');
        let name = parts.next()?;
        let value = parts.next()?;
        if name == cookie_name {
            return Some(value.to_string());
        }
    }
    None
}

/// Build a Set-Cookie header value
pub fn build_set_cookie(config: &SessionCookieConfig, token: &str) -> String {
    let mut parts = vec![
        format!("{}={}", config.name, token),
        format!("Path={}", config.path),
        format!("Max-Age={}", config.max_age_secs),
    ];

    if config.secure {
        parts.push("Secure".to_string());
    }
    if config.http_only {
        parts.push("HttpOnly".to_string());
    }
    if let Some(ref domain) = config.domain {
        parts.push(format!("Domain={}", domain));
    }
    parts.push(format!("SameSite={}", config.same_site));

    parts.join("; ")
}

/// Build a cookie clear header (expires immediately)
pub fn build_clear_cookie(config: &SessionCookieConfig) -> String {
    format!(
        "{}=; Path={}; Max-Age=0; HttpOnly; SameSite={}",
        config.name, config.path, config.same_site
    )
}

/// Middleware: Session cookie authentication
///
/// Checks for a session cookie and validates it against the auth manager.
/// If valid, the request proceeds. Falls through to allow other auth
/// mechanisms (e.g., Bearer token) to be checked downstream.
pub async fn session_cookie_middleware(
    State(state): State<Arc<GatewayState>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Check if auth is required
    let auth_required = {
        let config = state.config.read().await;
        config.security.auth_required
    };

    if !auth_required {
        return Ok(next.run(req).await);
    }

    // Try to extract session from cookie
    let cookie_config = SessionCookieConfig::default();
    if let Some(token) = extract_session_cookie(&req, &cookie_config.name) {
        let _session: Option<crate::security::Session> =
            state.auth_manager.validate_session(&token).await;
        if _session.is_some() {
            debug!("Valid session cookie, allowing request");
            return Ok(next.run(req).await);
        }
    }

    // No valid session cookie — allow through so Bearer token auth can check
    Ok(next.run(req).await)
}

/// OAuth provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthConfig {
    /// Enable OAuth authentication
    pub enabled: bool,
    /// GitHub OAuth configuration
    pub github: Option<OAuthProviderConfig>,
    /// Google OAuth configuration
    pub google: Option<OAuthProviderConfig>,
}

impl Default for OAuthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            github: None,
            google: None,
        }
    }
}

/// Single OAuth provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthProviderConfig {
    /// Client ID from OAuth provider
    pub client_id: String,
    /// Client secret from OAuth provider
    pub client_secret: String,
    /// Authorization endpoint URL (optional, uses default if not set)
    pub auth_url: Option<String>,
    /// Token endpoint URL (optional, uses default if not set)
    pub token_url: Option<String>,
    /// Redirect URI — must match OAuth app settings
    pub redirect_uri: String,
    /// Scopes to request
    pub scopes: Vec<String>,
}

/// OAuth user profile returned by providers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthUserProfile {
    /// Provider-assigned user ID
    pub provider_user_id: String,
    /// Provider name (github, google, etc.)
    pub provider: String,
    /// User email
    pub email: Option<String>,
    /// Display name
    pub name: Option<String>,
    /// Avatar URL
    pub avatar_url: Option<String>,
}

/// CORS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorsConfig {
    /// Enable CORS
    pub enabled: bool,
    /// Allowed origins (use ["*"] for any)
    pub allowed_origins: Vec<String>,
    /// Allowed methods
    pub allowed_methods: Vec<String>,
    /// Allowed headers
    pub allowed_headers: Vec<String>,
    /// Allow credentials (cookies)
    pub allow_credentials: bool,
    /// Max age for preflight cache
    pub max_age_secs: u32,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allowed_origins: vec!["*".to_string()],
            allowed_methods: vec![
                "GET".to_string(),
                "POST".to_string(),
                "PUT".to_string(),
                "DELETE".to_string(),
                "OPTIONS".to_string(),
            ],
            allowed_headers: vec![
                "Content-Type".to_string(),
                "Authorization".to_string(),
                "X-Requested-With".to_string(),
            ],
            allow_credentials: true,
            max_age_secs: 3600,
        }
    }
}

/// CSP (Content Security Policy) configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CspConfig {
    /// Enable CSP
    pub enabled: bool,
    /// Default CSP policy string
    pub policy: String,
    /// Nonce-enabled script-src (for inline scripts)
    pub use_nonce: bool,
}

impl Default for CspConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            policy: "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self' ws: wss:; frame-ancestors 'none'; base-uri 'self'; form-action 'self';".to_string(),
            use_nonce: true,
        }
    }
}

/// Generate a random CSP nonce
pub fn generate_csp_nonce() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..16).map(|_| rng.gen()).collect();
    base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_set_cookie() {
        let config = SessionCookieConfig::default();
        let cookie = build_set_cookie(&config, "test_token");
        assert!(cookie.contains("manta_session=test_token"));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("Secure"));
        assert!(cookie.contains("SameSite=lax"));
    }

    #[test]
    fn test_build_clear_cookie() {
        let config = SessionCookieConfig::default();
        let cookie = build_clear_cookie(&config);
        assert!(cookie.contains("Max-Age=0"));
    }

    #[test]
    fn test_generate_csp_nonce() {
        let nonce1 = generate_csp_nonce();
        let nonce2 = generate_csp_nonce();
        assert!(!nonce1.is_empty());
        assert_ne!(nonce1, nonce2);
    }
}
