//! OAuth2 credential types for LLM provider authentication
//!
//! Supports multiple authentication schemes:
//! - API key (traditional, e.g. OpenAI, Anthropic)
//! - Bearer token (short-lived, e.g. some enterprise proxies)
//! - OAuth2 client credentials (Azure AD, Google, etc.)

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Authentication credential for LLM providers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Credential {
 /// Simple API key (most common)
    ApiKey {
 /// The secret key
        key: String,
    },
 /// Bearer token with optional expiration
    BearerToken {
 /// The token string
        token: String,
 /// When the token expires (if known)
        #[serde(skip_serializing_if = "Option::is_none")]
        expires_at: Option<DateTime<Utc>>,
    },
 /// OAuth2 client-credentials flow
    OAuth2 {
 /// Current access token
        access_token: String,
 /// Refresh token (if available)
        #[serde(skip_serializing_if = "Option::is_none")]
        refresh_token: Option<String>,
 /// Token expiration time
        expires_at: DateTime<Utc>,
 /// OAuth2 token endpoint URL
        token_url: String,
 /// OAuth2 client ID
        client_id: String,
 /// OAuth2 client secret
        #[serde(skip_serializing_if = "Option::is_none")]
        client_secret: Option<String>,
 /// Optional scope string
        #[serde(skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
    },
}

impl Credential {
 /// Create an API key credential (backward-compat helper).
    pub fn api_key(key: impl Into<String>) -> Self {
        Self::ApiKey { key: key.into() }
    }

 /// Create a bearer token credential.
    pub fn bearer_token(token: impl Into<String>) -> Self {
        Self::BearerToken {
            token: token.into(),
            expires_at: None,
        }
    }

 /// Build the Authorization header value for this credential.
    pub fn authorization_header(&self) -> String {
        match self {
            Credential::ApiKey { key } => format!("Bearer {key}"),
            Credential::BearerToken { token, .. } => format!("Bearer {token}"),
            Credential::OAuth2 { access_token, .. } => format!("Bearer {access_token}"),
        }
    }

 /// Returns true if the credential has a known expiration and is past it.
    pub fn is_expired(&self) -> bool {
        match self {
            Credential::ApiKey { .. } => false,
            Credential::BearerToken { expires_at, .. } => {
                expires_at.is_some_and(|t| Utc::now() >= t)
            }
            Credential::OAuth2 { expires_at, .. } => Utc::now() >= *expires_at,
        }
    }

 /// Returns true if the credential expires within the given margin.
    pub fn is_expiring_soon(&self, margin: Duration) -> bool {
        match self {
            Credential::ApiKey { .. } => false,
            Credential::BearerToken { expires_at, .. } => {
                expires_at.is_some_and(|t| Utc::now() + margin >= t)
            }
            Credential::OAuth2 { expires_at, .. } => Utc::now() + margin >= *expires_at,
        }
    }

 /// Refresh the credential if it supports refresh and is expired or expiring.
 ///
 /// For `OAuth2`, performs a client-credentials token refresh.
 /// For other variants this is a no-op.
    pub async fn refresh_if_needed(&mut self, client: &reqwest::Client) -> crate::Result<()> {
        let needs_refresh = self.is_expired() || self.is_expiring_soon(Duration::minutes(5));
        if !needs_refresh {
            return Ok(());
        }

        if let Credential::OAuth2 {
            refresh_token: Some(refresh),
            token_url,
            client_id,
            client_secret,
            scope,
            access_token,
            expires_at,
            ..
        } = self
        {
            let mut params = vec![
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh.as_str()),
                ("client_id", client_id.as_str()),
            ];
            if let Some(secret) = client_secret {
                params.push(("client_secret", secret.as_str()));
            }
            if let Some(scope) = scope {
                params.push(("scope", scope.as_str()));
            }

            let resp = client
                .post(token_url.clone())
                .form(&params)
                .send()
                .await
                .map_err(crate::error::SyscityError::Http)?;

            if !resp.status().is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(crate::error::SyscityError::ExternalService {
                    source: format!("OAuth2 refresh failed: {}", body),
                    cause: None,
                });
            }

            let data: TokenResponse =
                resp.json()
                    .await
                    .map_err(|e| crate::error::SyscityError::ExternalService {
                        source: format!("OAuth2 refresh response invalid: {}", e),
                        cause: None,
                    })?;

            *access_token = data.access_token;
            *expires_at = Utc::now() + Duration::seconds(data.expires_in as i64);
            if let Some(new_refresh) = data.refresh_token {
                *refresh = new_refresh;
            }
        }
        Ok(())
    }
}

/// Resolve a credential following the priority chain:
///
/// 1. Environment variable (e.g. `SYSCITY_PROVIDER_{NAME}_KEY`) — highest priority
/// 2. Bearer token / OAuth2 from auth_profile
/// 3. API key from config `api_keys` list
/// 4. Single `api_key` from config — lowest priority
///
/// This
pub fn resolve_from_env_and_config(
    provider_name: &str,
    config_api_key: &str,
    config_api_keys: &[String],
) -> Option<Credential> {
    let env_key =
        format!("SYSCITY_PROVIDER_{}_KEY", provider_name.to_uppercase().replace('-', "_"));

 // 1. Environment variable (highest priority)
    if let Ok(key) = std::env::var(&env_key) {
        if !key.is_empty() {
            return Some(Credential::ApiKey { key });
        }
    }

 // 2. Config api_keys list
    if let Some(key) = config_api_keys.first() {
        if !key.is_empty() {
            return Some(Credential::ApiKey { key: key.clone() });
        }
    }

 // 3. Single api_key from config
    if !config_api_key.is_empty() {
        return Some(Credential::ApiKey {
            key: config_api_key.to_string(),
        });
    }

    None
}

impl fmt::Display for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Credential::ApiKey { key } => {
                let masked = if key.len() > 8 {
                    format!("{}****", &key[..4])
                } else {
                    "****".to_string()
                };
                write!(f, "ApiKey({})", masked)
            }
            Credential::BearerToken { token, expires_at } => {
                let masked = if token.len() > 8 {
                    format!("{}****", &token[..4])
                } else {
                    "****".to_string()
                };
                write!(
                    f,
                    "BearerToken({}{})",
                    masked,
                    expires_at
                        .map(|t| format!(" exp={}", t))
                        .unwrap_or_default()
                )
            }
            Credential::OAuth2 {
                access_token,
                expires_at,
                client_id,
                ..
            } => {
                let masked = if access_token.len() > 8 {
                    format!("{}****", &access_token[..4])
                } else {
                    "****".to_string()
                };
                write!(f, "OAuth2(client={} token={} exp={})", client_id, masked, expires_at)
            }
        }
    }
}

/// OAuth2 token endpoint response.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_key_authorization_header() {
        let cred = Credential::api_key("sk-test");
        assert_eq!(cred.authorization_header(), "Bearer sk-test");
    }

    #[test]
    fn test_bearer_token_authorization_header() {
        let cred = Credential::BearerToken {
            token: "tok-123".to_string(),
            expires_at: None,
        };
        assert_eq!(cred.authorization_header(), "Bearer tok-123");
    }

    #[test]
    fn test_oauth2_authorization_header() {
        let cred = Credential::OAuth2 {
            access_token: "at-xyz".to_string(),
            refresh_token: Some("rt-abc".to_string()),
            expires_at: Utc::now() + Duration::hours(1),
            token_url: "https://example.com/token".to_string(),
            client_id: "client-1".to_string(),
            client_secret: Some("secret".to_string()),
            scope: None,
        };
        assert_eq!(cred.authorization_header(), "Bearer at-xyz");
    }

    #[test]
    fn test_api_key_never_expires() {
        let cred = Credential::api_key("sk-test");
        assert!(!cred.is_expired());
        assert!(!cred.is_expiring_soon(Duration::minutes(1)));
    }

    #[test]
    fn test_bearer_token_expiration() {
        let past = Utc::now() - Duration::minutes(1);
        let cred = Credential::BearerToken {
            token: "tok".to_string(),
            expires_at: Some(past),
        };
        assert!(cred.is_expired());
        assert!(cred.is_expiring_soon(Duration::minutes(5)));

        let future = Utc::now() + Duration::hours(1);
        let cred2 = Credential::BearerToken {
            token: "tok".to_string(),
            expires_at: Some(future),
        };
        assert!(!cred2.is_expired());
        assert!(!cred2.is_expiring_soon(Duration::minutes(5)));
    }

    #[test]
    fn test_oauth2_expiration() {
        let past = Utc::now() - Duration::minutes(1);
        let cred = Credential::OAuth2 {
            access_token: "at".to_string(),
            refresh_token: None,
            expires_at: past,
            token_url: "https://example.com/token".to_string(),
            client_id: "c".to_string(),
            client_secret: None,
            scope: None,
        };
        assert!(cred.is_expired());

        let near_future = Utc::now() + Duration::minutes(2);
        let cred2 = Credential::OAuth2 {
            access_token: "at".to_string(),
            refresh_token: None,
            expires_at: near_future,
            token_url: "https://example.com/token".to_string(),
            client_id: "c".to_string(),
            client_secret: None,
            scope: None,
        };
        assert!(!cred2.is_expired());
        assert!(cred2.is_expiring_soon(Duration::minutes(5)));
    }

    #[test]
    fn test_credential_display_masks_secrets() {
        let cred = Credential::api_key("sk-very-long-secret-key");
        let s = format!("{}", cred);
        assert!(s.contains("sk-v****"));
        assert!(!s.contains("secret-key"));
    }

    #[test]
    fn test_credential_refresh_no_op_for_api_key() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let client = reqwest::Client::new();
            let mut cred = Credential::api_key("sk-test");
 // Should not panic or error
            let result = cred.refresh_if_needed(&client).await;
            assert!(result.is_ok());
        });
    }
}
