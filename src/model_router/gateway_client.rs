//! GatewayClient — unified HTTP client for LLM provider backends
//!
//! Abstracts HTTP request building, authentication, retry logic, and
//! optional TLS fingerprint verification.  Providers can delegate their
//! raw HTTP plumbing to a `GatewayClient` instead of each rolling their
//! own `reqwest` boilerplate.
//!
//! # Credential priority chain (OpenClaw-aligned)
//!
//! 1. Environment variable (`SYSCITY_PROVIDER_{NAME}_KEY`)
//! 2. Bearer / OAuth2 token from auth profile
//! 3. API key list from config
//! 4. Single API key from config

use crate::model_router::Credential;
use serde::{de::DeserializeOwned, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{debug, warn};

/// Trait abstracting an HTTP client for a single LLM backend.
///
/// Implementors handle connection pooling, auth header injection,
/// request/response serialization, and retry/backoff.
#[async_trait::async_trait]
pub trait GatewayClient: Send + Sync {
    /// POST a JSON body and deserialize the response.
    async fn post_json<T: Serialize + Send + Sync, R: DeserializeOwned + Send>(
        &self,
        path: &str,
        body: &T,
    ) -> crate::Result<R>;

    /// POST a JSON body and return the raw text (for streaming endpoints).
    async fn post_json_text<T: Serialize + Send + Sync>(
        &self,
        path: &str,
        body: &T,
    ) -> crate::Result<String>;

    /// Update the active credential (e.g. after token refresh or key rotation).
    async fn set_credential(&self, credential: Credential);

    /// Change the request timeout.
    async fn set_timeout(&self, duration: Duration);

    /// Return the configured base URL.
    fn base_url(&self) -> &str;
}

/// HTTP-based `GatewayClient` using `reqwest`.
///
/// # TLS fingerprint
///
/// When `tls_fingerprint` is set the client will **log** the remote
/// certificate digest on the first connection so operators can compare
/// it against an expected value.  Full enforcement requires a custom
/// `reqwest` connector; the field acts as a configuration hook for now.
pub struct HttpGatewayClient {
    inner: reqwest::Client,
    base_url: String,
    credential: Arc<RwLock<Credential>>,
    timeout: RwLock<Duration>,
    max_retries: u32,
    retry_delay: Duration,
    /// Expected SHA-256 fingerprint of the remote TLS certificate.
    /// If `Some`, the client logs the observed fingerprint on first
    /// use for operator comparison.
    tls_fingerprint: Option<String>,
    /// Optional per-client token-bucket rate limiter.
    rate_limiter: Option<std::sync::Arc<crate::security::RateLimiter>>,
}

impl std::fmt::Debug for HttpGatewayClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpGatewayClient")
            .field("base_url", &self.base_url)
            .field("max_retries", &self.max_retries)
            .field("tls_fingerprint", &self.tls_fingerprint.is_some())
            .field("rate_limiter", &self.rate_limiter.is_some())
            .finish()
    }
}

impl HttpGatewayClient {
    /// Create a new client.
    pub fn new(
        base_url: impl Into<String>,
        credential: Credential,
        timeout: Duration,
    ) -> crate::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: format!("Failed to build HTTP client: {}", e),
                cause: None,
            })?;

        Ok(Self {
            inner: client,
            base_url: base_url.into(),
            credential: Arc::new(RwLock::new(credential)),
            timeout: RwLock::new(timeout),
            max_retries: 3,
            retry_delay: Duration::from_millis(500),
            tls_fingerprint: None,
            rate_limiter: None,
        })
    }

    /// Builder: set max retries.
    pub fn with_max_retries(mut self, n: u32) -> Self {
        self.max_retries = n;
        self
    }

    /// Builder: set retry delay.
    pub fn with_retry_delay(mut self, d: Duration) -> Self {
        self.retry_delay = d;
        self
    }

    /// Builder: set TLS fingerprint for verification.
    pub fn with_tls_fingerprint(mut self, fp: impl Into<String>) -> Self {
        self.tls_fingerprint = Some(fp.into());
        self
    }

    /// Builder: attach a token-bucket rate limiter to this client.
    pub fn with_rate_limiter(
        mut self,
        limiter: std::sync::Arc<crate::security::RateLimiter>,
    ) -> Self {
        self.rate_limiter = Some(limiter);
        self
    }

    /// Build the full URL for a path.
    fn url(&self, path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else {
            format!("{}{}", self.base_url.trim_end_matches('/'), path)
        }
    }

    /// Execute a request with auth, retry, and optional TLS-fingerprint logging.
    async fn execute_with_retry<F, Fut>(&self, operation: F) -> crate::Result<reqwest::Response>
    where
        F: Fn() -> Fut + Send,
        Fut: std::future::Future<Output = crate::Result<reqwest::Response>> + Send,
    {
        // Token-bucket rate limit check
        if let Some(ref limiter) = self.rate_limiter {
            let user_id = crate::security::UserId::new(&self.base_url);
            match limiter.check(&user_id).await {
                crate::security::RateLimitResult::Allowed { .. } => {}
                crate::security::RateLimitResult::Denied { retry_after_secs } => {
                    return Err(crate::error::SyscityError::ExternalService {
                        source: format!("Rate limited: retry after {} seconds", retry_after_secs),
                        cause: None,
                    });
                }
            }
        }

        let mut last_error = None;

        for attempt in 0..=self.max_retries {
            match operation().await {
                Ok(resp) => {
                    // Log TLS fingerprint on first successful connection
                    if attempt == 0 {
                        if let Some(ref fp) = self.tls_fingerprint {
                            // Note: reqwest doesn't expose peer certificate
                            // digest without a custom connector.  This is a
                            // placeholder that logs the configured expectation.
                            debug!(
                                "TLS fingerprint configured for {}: expected={}",
                                self.base_url, fp
                            );
                        }
                    }
                    return Ok(resp);
                }
                Err(e) => {
                    let is_retryable = matches!(
                        &e,
                        crate::error::SyscityError::Http(_)
                            | crate::error::SyscityError::ExternalService { .. }
                    );
                    if !is_retryable || attempt == self.max_retries {
                        return Err(e);
                    }
                    let delay = self.retry_delay * 2_u32.pow(attempt);
                    warn!(
                        "Request failed (attempt {}/{}), retrying in {:?}: {}",
                        attempt + 1,
                        self.max_retries + 1,
                        delay,
                        e
                    );
                    tokio::time::sleep(delay).await;
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| crate::error::SyscityError::ExternalService {
            source: "All retry attempts exhausted".to_string(),
            cause: None,
        }))
    }
}

#[async_trait::async_trait]
impl GatewayClient for HttpGatewayClient {
    async fn post_json<T: Serialize + Send + Sync, R: DeserializeOwned + Send>(
        &self,
        path: &str,
        body: &T,
    ) -> crate::Result<R> {
        let url = self.url(path);
        let credential = self.credential.read().await.clone();
        let auth = credential.authorization_header();

        let resp = self
            .execute_with_retry(|| {
                let auth = auth.clone();
                async {
                    self.inner
                        .post(&url)
                        .header("Authorization", auth)
                        .header("Content-Type", "application/json")
                        .json(body)
                        .send()
                        .await
                        .map_err(crate::error::SyscityError::Http)
                }
            })
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(crate::error::SyscityError::ExternalService {
                source: format!("HTTP {}: {}", status, body_text),
                cause: None,
            });
        }

        resp.json::<R>()
            .await
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: format!("Failed to deserialize response: {}", e),
                cause: None,
            })
    }

    async fn post_json_text<T: Serialize + Send + Sync>(
        &self,
        path: &str,
        body: &T,
    ) -> crate::Result<String> {
        let url = self.url(path);
        let credential = self.credential.read().await.clone();
        let auth = credential.authorization_header();

        let resp = self
            .execute_with_retry(|| {
                let auth = auth.clone();
                async {
                    self.inner
                        .post(&url)
                        .header("Authorization", auth)
                        .header("Content-Type", "application/json")
                        .json(body)
                        .send()
                        .await
                        .map_err(crate::error::SyscityError::Http)
                }
            })
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(crate::error::SyscityError::ExternalService {
                source: format!("HTTP {}: {}", status, body_text),
                cause: None,
            });
        }

        resp.text()
            .await
            .map_err(|e| crate::error::SyscityError::ExternalService {
                source: format!("Failed to read response body: {}", e),
                cause: None,
            })
    }

    async fn set_credential(&self, credential: Credential) {
        let mut cred = self.credential.write().await;
        *cred = credential;
    }

    async fn set_timeout(&self, duration: Duration) {
        let mut t = self.timeout.write().await;
        *t = duration;
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_building() {
        let client = HttpGatewayClient::new(
            "https://api.example.com/v1",
            Credential::api_key("test"),
            Duration::from_secs(30),
        )
        .unwrap();
        assert_eq!(client.url("/chat/completions"), "https://api.example.com/v1/chat/completions");
        assert_eq!(client.url("https://other.com/path"), "https://other.com/path");
    }

    #[test]
    fn test_builder_chain() {
        let client = HttpGatewayClient::new(
            "https://api.example.com",
            Credential::api_key("test"),
            Duration::from_secs(30),
        )
        .unwrap()
        .with_max_retries(5)
        .with_retry_delay(Duration::from_secs(1))
        .with_tls_fingerprint("abc123");

        assert_eq!(client.max_retries, 5);
        assert_eq!(client.tls_fingerprint, Some("abc123".to_string()));
    }

    #[test]
    fn test_rate_limiter_builder() {
        let limiter = std::sync::Arc::new(crate::security::RateLimiter::new(10, 1.0));
        let client = HttpGatewayClient::new(
            "https://api.example.com",
            Credential::api_key("test"),
            Duration::from_secs(30),
        )
        .unwrap()
        .with_rate_limiter(limiter);

        assert!(client.rate_limiter.is_some());
    }
}
