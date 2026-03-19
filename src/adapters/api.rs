//! API client adapter for Manta
//!
//! This module provides an HTTP client for communicating with
//! external APIs.

use crate::config::ServiceConfig;
use crate::error::{MantaError, Result};
use crate::secrets::SecretRef;
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode};
use serde::{de::DeserializeOwned, Serialize};
use std::time::Duration;
use tracing::{debug, error, instrument, trace};

/// API client for external services
#[derive(Debug, Clone)]
pub struct ApiClient {
    client: Client,
    base_url: String,
    api_key: Option<String>,
    #[allow(dead_code)]
    timeout: Duration,
    retry_config: crate::config::RetryConfig,
}

impl ApiClient {
    /// Create a new API client from service configuration (async)
    ///
    /// This async version properly resolves SecretRef API keys.
    pub async fn new_async(config: &ServiceConfig) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds))
            .build()
            .map_err(|e| MantaError::Internal(format!("Failed to build HTTP client: {}", e)))?;

        // Resolve API key if it's a SecretRef
        let api_key = if let Some(ref key_ref) = config.api_key {
            match key_ref {
                SecretRef::String(s) if !s.starts_with('$') => Some(s.clone()),
                SecretRef::String(s) => {
                    // Try to resolve env var reference
                    let var_name = s.trim_start_matches('$');
                    std::env::var(var_name).ok()
                }
                _ => {
                    // For other SecretRef variants, we need the secrets resolver
                    // For now, return None and let the caller resolve it properly
                    None
                }
            }
        } else {
            None
        };

        Ok(Self {
            client,
            base_url: config.endpoint.clone(),
            api_key,
            timeout: Duration::from_secs(config.timeout_seconds),
            retry_config: config.retry.clone(),
        })
    }

    /// Create a new API client from service configuration
    ///
    /// Note: This synchronous version cannot fully resolve SecretRef API keys.
    /// Use `new_async` for proper secret resolution, or ensure secrets are
    /// resolved in the config before calling this method.
    pub fn new(config: &ServiceConfig) -> Result<Self> {
        // Try to get the resolved API key
        let api_key = config.api_key.as_ref().and_then(|key_ref| {
            match key_ref {
                SecretRef::String(s) if !s.starts_with('$') => Some(s.clone()),
                SecretRef::String(s) => {
                    // Try to resolve env var reference synchronously
                    let var_name = s.trim_start_matches('$');
                    std::env::var(var_name).ok()
                }
                _ => None, // Cannot resolve other variants synchronously
            }
        });

        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds))
            .build()
            .map_err(|e| MantaError::Internal(format!("Failed to build HTTP client: {}", e)))?;

        Ok(Self {
            client,
            base_url: config.endpoint.clone(),
            api_key,
            timeout: Duration::from_secs(config.timeout_seconds),
            retry_config: config.retry.clone(),
        })
    }

    /// Create a new API client with custom settings
    pub fn with_client(
        client: Client,
        base_url: impl Into<String>,
        api_key: Option<String>,
    ) -> Self {
        Self {
            client,
            base_url: base_url.into(),
            api_key,
            timeout: Duration::from_secs(30),
            retry_config: crate::config::RetryConfig::default(),
        }
    }

    /// Build a request with common headers
    fn build_request(&self, method: Method, path: &str) -> RequestBuilder {
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), path);

        let mut builder = self.client.request(method, &url);

        if let Some(ref key) = self.api_key {
            builder = builder.header("Authorization", format!("Bearer {}", key));
        }

        builder = builder
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .header("User-Agent", format!("manta/{} (Rust)", env!("CARGO_PKG_VERSION")));

        builder
    }

    /// Execute a request with retry logic
    async fn execute_with_retry(&self, request: RequestBuilder) -> Result<Response> {
        let mut attempt = 0;
        #[allow(unused_assignments)]
        let mut last_error = None;

        loop {
            let req = request
                .try_clone()
                .ok_or_else(|| MantaError::Internal("Failed to clone request".to_string()))?;

            match req.send().await {
                Ok(response) => {
                    trace!(status = %response.status(), "Received response");

                    // Check if we should retry based on status code
                    let status = response.status();
                    if status.is_success() {
                        return Ok(response);
                    }

                    // Don't retry client errors (4xx) except for rate limiting (429)
                    if status.is_client_error() && status != StatusCode::TOO_MANY_REQUESTS {
                        let body = response.text().await.unwrap_or_default();
                        return Err(MantaError::ExternalService {
                            source: format!("HTTP {}: {}", status, body),
                            cause: None,
                        });
                    }

                    // Retry server errors and rate limiting
                    last_error = Some(MantaError::ExternalService {
                        source: format!("HTTP {}", status),
                        cause: None,
                    });
                }
                Err(e) => {
                    error!(error = %e, "Request failed");
                    last_error = Some(MantaError::Http(e));
                }
            }

            attempt += 1;
            if attempt > self.retry_config.max_retries {
                break;
            }

            let delay = calculate_backoff(attempt, &self.retry_config);
            debug!(attempt, delay_ms = delay.as_millis(), "Retrying request");
            tokio::time::sleep(delay).await;
        }

        Err(last_error
            .unwrap_or_else(|| MantaError::Internal("Request failed after retries".to_string())))
    }

    /// Make a GET request
    #[instrument(skip(self), fields(path))]
    pub async fn get<T>(&self, path: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        debug!(path, "Making GET request");

        let request = self.build_request(Method::GET, path);
        let response = self.execute_with_retry(request).await?;

        response
            .json()
            .await
            .map_err(|e| MantaError::ExternalService {
                source: "Failed to parse response".to_string(),
                cause: Some(Box::new(e)),
            })
    }

    /// Make a POST request
    #[instrument(skip(self, body), fields(path))]
    pub async fn post<T, B>(&self, path: &str, body: &B) -> Result<T>
    where
        T: DeserializeOwned,
        B: Serialize,
    {
        debug!(path, "Making POST request");
        trace!(body = %serde_json::to_string(body).unwrap_or_default(), "Request body");

        let request = self.build_request(Method::POST, path).json(body);
        let response = self.execute_with_retry(request).await?;

        response
            .json()
            .await
            .map_err(|e| MantaError::ExternalService {
                source: "Failed to parse response".to_string(),
                cause: Some(Box::new(e)),
            })
    }

    /// Make a PUT request
    #[instrument(skip(self, body), fields(path))]
    pub async fn put<T, B>(&self, path: &str, body: &B) -> Result<T>
    where
        T: DeserializeOwned,
        B: Serialize,
    {
        debug!(path, "Making PUT request");

        let request = self.build_request(Method::PUT, path).json(body);
        let response = self.execute_with_retry(request).await?;

        response
            .json()
            .await
            .map_err(|e| MantaError::ExternalService {
                source: "Failed to parse response".to_string(),
                cause: Some(Box::new(e)),
            })
    }

    /// Make a DELETE request
    #[instrument(skip(self), fields(path))]
    pub async fn delete(&self, path: &str) -> Result<()> {
        debug!(path, "Making DELETE request");

        let request = self.build_request(Method::DELETE, path);
        let _response = self.execute_with_retry(request).await?;

        Ok(())
    }

    /// Check if the API is healthy
    pub async fn health_check(&self) -> Result<bool> {
        match self
            .client
            .get(format!("{}/health", self.base_url.trim_end_matches('/')))
            .timeout(Duration::from_secs(5))
            .send()
            .await
        {
            Ok(response) => Ok(response.status().is_success()),
            Err(_) => Ok(false),
        }
    }
}

/// Calculate backoff delay with exponential backoff and jitter
fn calculate_backoff(attempt: u32, config: &crate::config::RetryConfig) -> Duration {
    use std::time::Duration;

    // Exponential backoff: base_delay * 2^attempt
    let exponential = config
        .base_delay_ms
        .saturating_mul(2_u64.saturating_pow(attempt));
    let delay = exponential.min(config.max_delay_ms);

    // Add jitter (±25%)
    let jitter = (delay as f64 * 0.25) as u64;
    let jittered = if jitter > 0 {
        let offset = rand::random::<u64>() % (jitter * 2);
        delay.saturating_sub(jitter).saturating_add(offset)
    } else {
        delay
    };

    Duration::from_millis(jittered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RetryConfig;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn test_api_client_get() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "123",
                "name": "Test"
            })))
            .mount(&mock_server)
            .await;

        let config = ServiceConfig {
            endpoint: mock_server.uri(),
            api_key: None,
            timeout_seconds: 30,
            retry: RetryConfig::default(),
        };

        let client = ApiClient::new(&config).unwrap();
        let result: serde_json::Value = client.get("/test").await.unwrap();

        assert_eq!(result["id"], "123");
        assert_eq!(result["name"], "Test");
    }

    #[tokio::test]
    async fn test_api_client_post() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/test"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "created": true
            })))
            .mount(&mock_server)
            .await;

        let config = ServiceConfig {
            endpoint: mock_server.uri(),
            api_key: Some(SecretRef::String("secret".to_string())),
            timeout_seconds: 30,
            retry: RetryConfig::default(),
        };

        let client = ApiClient::new(&config).unwrap();
        let body = serde_json::json!({ "name": "Test" });
        let result: serde_json::Value = client.post("/test", &body).await.unwrap();

        assert_eq!(result["created"], true);
    }

    #[tokio::test]
    async fn test_api_client_not_found() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/not-found"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock_server)
            .await;

        let config = ServiceConfig {
            endpoint: mock_server.uri(),
            api_key: None,
            timeout_seconds: 30,
            retry: RetryConfig::default(),
        };

        let client = ApiClient::new(&config).unwrap();
        let result: Result<serde_json::Value> = client.get("/not-found").await;

        assert!(result.is_err());
    }
}
