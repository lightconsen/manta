//! OAuth2 Authentication Handlers
//!
//! Implements OAuth2 authorization code flow for GitHub and Google.
//! Mirrors OpenClaw's `src/gateway/auth/oauth.ts` functionality.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect},
};
use oauth2::{
    basic::BasicClient, AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, RedirectUrl,
    Scope, TokenResponse, TokenUrl,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{error, info, warn};

use crate::gateway::auth::{
    build_set_cookie, OAuthProviderConfig, OAuthUserProfile, SessionCookieConfig,
};
use crate::gateway::GatewayState;
use crate::security::{User, UserId};

/// Query params for OAuth callback
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct OAuthCallbackQuery {
    code: String,
    state: Option<String>,
}

/// Error response for OAuth failures
#[derive(Debug, Serialize)]
pub struct OAuthErrorResponse {
    error: String,
    message: String,
}

/// Build an OAuth2 client for a provider
fn build_oauth_client(
    config: &OAuthProviderConfig,
    auth_url: &str,
    token_url: &str,
) -> Result<BasicClient, String> {
    let client = BasicClient::new(
        ClientId::new(config.client_id.clone()),
        Some(ClientSecret::new(config.client_secret.clone())),
        AuthUrl::new(auth_url.to_string()).map_err(|e| format!("Invalid auth URL: {}", e))?,
        Some(
            TokenUrl::new(token_url.to_string())
                .map_err(|e| format!("Invalid token URL: {}", e))?,
        ),
    )
    .set_redirect_uri(
        RedirectUrl::new(config.redirect_uri.clone())
            .map_err(|e| format!("Invalid redirect URI: {}", e))?,
    );

    Ok(client)
}

/// Handler: Initiate GitHub OAuth login
///
/// Redirects the user to GitHub's authorization endpoint.
pub async fn github_login_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let oauth_config = {
        let config = state.config.read().await;
        config.security.oauth.clone()
    };

    if !oauth_config.enabled {
        return (StatusCode::NOT_IMPLEMENTED, "OAuth is not enabled".to_string()).into_response();
    }

    let Some(github_config) = oauth_config.github else {
        return (StatusCode::NOT_IMPLEMENTED, "GitHub OAuth is not configured".to_string())
            .into_response();
    };

    let auth_url = github_config
        .auth_url
        .clone()
        .unwrap_or_else(|| "https://github.com/login/oauth/authorize".to_string());
    let token_url = github_config
        .token_url
        .clone()
        .unwrap_or_else(|| "https://github.com/login/oauth/access_token".to_string());

    let client = match build_oauth_client(&github_config, &auth_url, &token_url) {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to build GitHub OAuth client: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "OAuth configuration error".to_string())
                .into_response();
        }
    };

    let mut auth_request = client.authorize_url(CsrfToken::new_random);

    for scope in &github_config.scopes {
        auth_request = auth_request.add_scope(Scope::new(scope.clone()));
    }

    // Default scope if none configured
    if github_config.scopes.is_empty() {
        auth_request = auth_request.add_scope(Scope::new("read:user".to_string()));
        auth_request = auth_request.add_scope(Scope::new("user:email".to_string()));
    }

    let (auth_url, _csrf_token) = auth_request.url();

    info!("Redirecting to GitHub OAuth: {}", auth_url);
    Redirect::temporary(auth_url.as_str()).into_response()
}

/// Handler: GitHub OAuth callback
///
/// Exchanges the authorization code for an access token,
/// fetches the user profile, creates/updates the user in AuthManager,
/// and sets a session cookie.
pub async fn github_callback_handler(
    State(state): State<Arc<GatewayState>>,
    Query(params): Query<OAuthCallbackQuery>,
) -> impl IntoResponse {
    let oauth_config = {
        let config = state.config.read().await;
        config.security.oauth.clone()
    };

    if !oauth_config.enabled {
        return (StatusCode::NOT_IMPLEMENTED, "OAuth is not enabled".to_string()).into_response();
    }

    let Some(github_config) = oauth_config.github else {
        return (StatusCode::NOT_IMPLEMENTED, "GitHub OAuth is not configured".to_string())
            .into_response();
    };

    let auth_url = github_config
        .auth_url
        .clone()
        .unwrap_or_else(|| "https://github.com/login/oauth/authorize".to_string());
    let token_url = github_config
        .token_url
        .clone()
        .unwrap_or_else(|| "https://github.com/login/oauth/access_token".to_string());

    let client = match build_oauth_client(&github_config, &auth_url, &token_url) {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to build GitHub OAuth client: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "OAuth configuration error".to_string())
                .into_response();
        }
    };

    // Exchange code for token
    let token_result = match client
        .exchange_code(AuthorizationCode::new(params.code.clone()))
        .request_async(oauth2::reqwest::async_http_client)
        .await
    {
        Ok(token) => token,
        Err(e) => {
            warn!("GitHub token exchange failed: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(OAuthErrorResponse {
                    error: "token_exchange_failed".to_string(),
                    message: format!("Failed to exchange authorization code: {}", e),
                }),
            )
                .into_response();
        }
    };

    let access_token = token_result.access_token().secret();

    // Fetch user profile from GitHub API
    let profile = match fetch_github_profile(access_token).await {
        Ok(p) => p,
        Err(e) => {
            warn!("Failed to fetch GitHub profile: {}", e);
            return (
                StatusCode::BAD_GATEWAY,
                Json(OAuthErrorResponse {
                    error: "profile_fetch_failed".to_string(),
                    message: format!("Failed to fetch user profile: {}", e),
                }),
            )
                .into_response();
        }
    };

    // Create or update user
    let user_id = UserId::new(format!("github:{}", profile.provider_user_id));
    let user =
        User::new(user_id.0.clone(), profile.name.unwrap_or_else(|| "GitHub User".to_string()))
            .admin(false);

    if !state.auth_manager.user_exists(&user_id).await {
        if let Err(e) = state.auth_manager.register_user(user.clone()).await {
            warn!("Failed to register OAuth user: {}", e);
        }
    }

    // Create session
    let session = match state
        .auth_manager
        .create_session(user_id, 24 * 7, None)
        .await
    {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to create session: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Session creation failed".to_string())
                .into_response();
        }
    };

    // Set session cookie and redirect to admin
    let cookie_config = SessionCookieConfig::default();
    let cookie = build_set_cookie(&cookie_config, &session.token);

    info!("GitHub OAuth login successful for user: {}", profile.provider_user_id);

    (
        StatusCode::TEMPORARY_REDIRECT,
        [(axum::http::header::SET_COOKIE, cookie)],
        [(axum::http::header::LOCATION, "/admin")],
        "Redirecting...",
    )
        .into_response()
}

/// Fetch GitHub user profile
async fn fetch_github_profile(access_token: &str) -> Result<OAuthUserProfile, String> {
    let client = reqwest::Client::new();

    let user_resp = client
        .get("https://api.github.com/user")
        .header("Authorization", format!("token {}", access_token))
        .header("User-Agent", "manta-gateway")
        .send()
        .await
        .map_err(|e| format!("GitHub API request failed: {}", e))?;

    if !user_resp.status().is_success() {
        return Err(format!("GitHub API returned: {}", user_resp.status()));
    }

    let user_json: serde_json::Value = user_resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse GitHub response: {}", e))?;

    let provider_user_id = user_json["id"]
        .as_i64()
        .map(|id| id.to_string())
        .unwrap_or_default();
    let name = user_json["login"].as_str().map(|s| s.to_string());
    let email = user_json["email"].as_str().map(|s| s.to_string());
    let avatar_url = user_json["avatar_url"].as_str().map(|s| s.to_string());

    Ok(OAuthUserProfile {
        provider_user_id,
        provider: "github".to_string(),
        email,
        name,
        avatar_url,
    })
}

/// Handler: Initiate Google OAuth login
pub async fn google_login_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let oauth_config = {
        let config = state.config.read().await;
        config.security.oauth.clone()
    };

    if !oauth_config.enabled {
        return (StatusCode::NOT_IMPLEMENTED, "OAuth is not enabled".to_string()).into_response();
    }

    let Some(google_config) = oauth_config.google else {
        return (StatusCode::NOT_IMPLEMENTED, "Google OAuth is not configured".to_string())
            .into_response();
    };

    let auth_url = google_config
        .auth_url
        .clone()
        .unwrap_or_else(|| "https://accounts.google.com/o/oauth2/v2/auth".to_string());
    let token_url = google_config
        .token_url
        .clone()
        .unwrap_or_else(|| "https://oauth2.googleapis.com/token".to_string());

    let client = match build_oauth_client(&google_config, &auth_url, &token_url) {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to build Google OAuth client: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "OAuth configuration error".to_string())
                .into_response();
        }
    };

    let mut auth_request = client.authorize_url(CsrfToken::new_random);

    for scope in &google_config.scopes {
        auth_request = auth_request.add_scope(Scope::new(scope.clone()));
    }

    if google_config.scopes.is_empty() {
        auth_request = auth_request.add_scope(Scope::new("openid".to_string()));
        auth_request = auth_request.add_scope(Scope::new("profile".to_string()));
        auth_request = auth_request.add_scope(Scope::new("email".to_string()));
    }

    let (auth_url, _csrf_token) = auth_request.url();

    info!("Redirecting to Google OAuth: {}", auth_url);
    Redirect::temporary(auth_url.as_str()).into_response()
}

/// Handler: Google OAuth callback
pub async fn google_callback_handler(
    State(state): State<Arc<GatewayState>>,
    Query(params): Query<OAuthCallbackQuery>,
) -> impl IntoResponse {
    let oauth_config = {
        let config = state.config.read().await;
        config.security.oauth.clone()
    };

    if !oauth_config.enabled {
        return (StatusCode::NOT_IMPLEMENTED, "OAuth is not enabled".to_string()).into_response();
    }

    let Some(google_config) = oauth_config.google else {
        return (StatusCode::NOT_IMPLEMENTED, "Google OAuth is not configured".to_string())
            .into_response();
    };

    let auth_url = google_config
        .auth_url
        .clone()
        .unwrap_or_else(|| "https://accounts.google.com/o/oauth2/v2/auth".to_string());
    let token_url = google_config
        .token_url
        .clone()
        .unwrap_or_else(|| "https://oauth2.googleapis.com/token".to_string());

    let client = match build_oauth_client(&google_config, &auth_url, &token_url) {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to build Google OAuth client: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "OAuth configuration error".to_string())
                .into_response();
        }
    };

    let token_result = match client
        .exchange_code(AuthorizationCode::new(params.code.clone()))
        .request_async(oauth2::reqwest::async_http_client)
        .await
    {
        Ok(token) => token,
        Err(e) => {
            warn!("Google token exchange failed: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(OAuthErrorResponse {
                    error: "token_exchange_failed".to_string(),
                    message: format!("Failed to exchange authorization code: {}", e),
                }),
            )
                .into_response();
        }
    };

    let access_token = token_result.access_token().secret();

    let profile = match fetch_google_profile(access_token).await {
        Ok(p) => p,
        Err(e) => {
            warn!("Failed to fetch Google profile: {}", e);
            return (
                StatusCode::BAD_GATEWAY,
                Json(OAuthErrorResponse {
                    error: "profile_fetch_failed".to_string(),
                    message: format!("Failed to fetch user profile: {}", e),
                }),
            )
                .into_response();
        }
    };

    let user_id = UserId::new(format!("google:{}", profile.provider_user_id));
    let user =
        User::new(user_id.0.clone(), profile.name.unwrap_or_else(|| "Google User".to_string()))
            .admin(false);

    if !state.auth_manager.user_exists(&user_id).await {
        if let Err(e) = state.auth_manager.register_user(user.clone()).await {
            warn!("Failed to register OAuth user: {}", e);
        }
    }

    let session = match state
        .auth_manager
        .create_session(user_id, 24 * 7, None)
        .await
    {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to create session: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Session creation failed".to_string())
                .into_response();
        }
    };

    let cookie_config = SessionCookieConfig::default();
    let cookie = build_set_cookie(&cookie_config, &session.token);

    info!("Google OAuth login successful for user: {}", profile.provider_user_id);

    (
        StatusCode::TEMPORARY_REDIRECT,
        [(axum::http::header::SET_COOKIE, cookie)],
        [(axum::http::header::LOCATION, "/admin")],
        "Redirecting...",
    )
        .into_response()
}

/// Fetch Google user profile
async fn fetch_google_profile(access_token: &str) -> Result<OAuthUserProfile, String> {
    let client = reqwest::Client::new();

    let resp = client
        .get("https://openidconnect.googleapis.com/v1/userinfo")
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| format!("Google API request failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Google API returned: {}", resp.status()));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse Google response: {}", e))?;

    let provider_user_id = json["sub"].as_str().unwrap_or_default().to_string();
    let name = json["name"].as_str().map(|s| s.to_string());
    let email = json["email"].as_str().map(|s| s.to_string());
    let avatar_url = json["picture"].as_str().map(|s| s.to_string());

    Ok(OAuthUserProfile {
        provider_user_id,
        provider: "google".to_string(),
        email,
        name,
        avatar_url,
    })
}

/// Handler: Logout — clear session cookie and revoke session
pub async fn logout_handler(
    State(state): State<Arc<GatewayState>>,
    req: axum::extract::Request,
) -> impl IntoResponse {
    use crate::gateway::auth::extract_session_cookie;

    let cookie_config = SessionCookieConfig::default();
    let clear_cookie = crate::gateway::auth::build_clear_cookie(&cookie_config);

    // Revoke session if present
    if let Some(token) = extract_session_cookie(&req, &cookie_config.name) {
        state.auth_manager.revoke_session(&token).await;
    }

    info!("User logged out");

    (
        StatusCode::OK,
        [(axum::http::header::SET_COOKIE, clear_cookie)],
        Json(serde_json::json!({"status": "logged_out"})),
    )
}

use axum::Json;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::auth::OAuthProviderConfig;

    #[test]
    fn test_oauth_callback_query_deserialize() {
        let json = r#"{"code": "abc123", "state": "csrf_token"}"#;
        let query: OAuthCallbackQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.code, "abc123");
        assert_eq!(query.state, Some("csrf_token".to_string()));
    }

    #[test]
    fn test_oauth_callback_query_without_state() {
        let json = r#"{"code": "abc123"}"#;
        let query: OAuthCallbackQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.code, "abc123");
        assert_eq!(query.state, None);
    }

    #[test]
    fn test_oauth_error_response_serialize() {
        let resp = OAuthErrorResponse {
            error: "token_exchange_failed".to_string(),
            message: "Something went wrong".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("token_exchange_failed"));
        assert!(json.contains("Something went wrong"));
    }

    #[test]
    fn test_build_oauth_client_valid() {
        let config = OAuthProviderConfig {
            client_id: "test_id".to_string(),
            client_secret: "test_secret".to_string(),
            auth_url: None,
            token_url: None,
            redirect_uri: "http://localhost:8080/callback".to_string(),
            scopes: vec!["read:user".to_string()],
        };
        let result = build_oauth_client(
            &config,
            "https://github.com/login/oauth/authorize",
            "https://github.com/login/oauth/access_token",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_build_oauth_client_invalid_auth_url() {
        let config = OAuthProviderConfig {
            client_id: "test_id".to_string(),
            client_secret: "test_secret".to_string(),
            auth_url: None,
            token_url: None,
            redirect_uri: "http://localhost:8080/callback".to_string(),
            scopes: vec![],
        };
        let result = build_oauth_client(&config, "not-a-url", "https://example.com/token");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid auth URL"));
    }

    #[test]
    fn test_build_oauth_client_invalid_token_url() {
        let config = OAuthProviderConfig {
            client_id: "test_id".to_string(),
            client_secret: "test_secret".to_string(),
            auth_url: None,
            token_url: None,
            redirect_uri: "http://localhost:8080/callback".to_string(),
            scopes: vec![],
        };
        let result = build_oauth_client(&config, "https://example.com/auth", "not-a-url");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid token URL"));
    }

    #[test]
    fn test_build_oauth_client_invalid_redirect_uri() {
        let config = OAuthProviderConfig {
            client_id: "test_id".to_string(),
            client_secret: "test_secret".to_string(),
            auth_url: None,
            token_url: None,
            redirect_uri: "not-a-valid-uri".to_string(),
            scopes: vec![],
        };
        let result =
            build_oauth_client(&config, "https://example.com/auth", "https://example.com/token");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid redirect URI"));
    }
}
