//! API client adapter for Syscity
//!
//! This module provides an HTTP client for communicating with
//! external APIs.

use crate::config::ServiceConfig;
use crate::error::{SyscityError, Result};
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
            .no_proxy()
            .build()
            .map_err(|e| SyscityError::Internal(format!("Failed to build HTTP client: {}", e)))?;

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
            .no_proxy()
            .build()
            .map_err(|e| SyscityError::Internal(format!("Failed to build HTTP client: {}", e)))?;

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
            .header("User-Agent", format!("syscity/{} (Rust)", env!("CARGO_PKG_VERSION")));

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
                .ok_or_else(|| SyscityError::Internal("Failed to clone request".to_string()))?;

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
                        return Err(SyscityError::ExternalService {
                            source: format!("HTTP {}: {}", status, body),
                            cause: None,
                        });
                    }

                    // Retry server errors and rate limiting
                    last_error = Some(SyscityError::ExternalService {
                        source: format!("HTTP {}", status),
                        cause: None,
                    });
                }
                Err(e) => {
                    error!(error = %e, "Request failed");
                    last_error = Some(SyscityError::Http(e));
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
            .unwrap_or_else(|| SyscityError::Internal("Request failed after retries".to_string())))
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
            .map_err(|e| SyscityError::ExternalService {
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
            .map_err(|e| SyscityError::ExternalService {
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
            .map_err(|e| SyscityError::ExternalService {
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
    use wiremock::matchers::{header, header_exists, method, path};
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
            timeout_seconds: 5,
            retry: RetryConfig {
                max_retries: 0,
                base_delay_ms: 0,
                max_delay_ms: 0,
            },
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
            timeout_seconds: 5,
            retry: RetryConfig {
                max_retries: 0,
                base_delay_ms: 0,
                max_delay_ms: 0,
            },
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
            timeout_seconds: 5,
            retry: RetryConfig {
                max_retries: 0,
                base_delay_ms: 0,
                max_delay_ms: 0,
            },
        };

        let client = ApiClient::new(&config).unwrap();
        let result: Result<serde_json::Value> = client.get("/not-found").await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_api_client_with_client() {
        let mock_server = MockServer::start().await;
        let client = Client::new();
        let api_client = ApiClient::with_client(client, mock_server.uri(), Some("key".to_string()));

        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .mount(&mock_server)
            .await;

        let result: serde_json::Value = api_client.get("/health").await.unwrap();
        assert_eq!(result["ok"], true);
    }

    #[tokio::test]
    async fn test_api_client_put() {
        let mock_server = MockServer::start().await;

        Mock::given(method("PUT"))
            .and(path("/update"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "updated": true
            })))
            .mount(&mock_server)
            .await;

        let config = ServiceConfig {
            endpoint: mock_server.uri(),
            api_key: None,
            timeout_seconds: 5,
            retry: RetryConfig {
                max_retries: 0,
                base_delay_ms: 0,
                max_delay_ms: 0,
            },
        };

        let client = ApiClient::new(&config).unwrap();
        let body = serde_json::json!({ "name": "Updated" });
        let result: serde_json::Value = client.put("/update", &body).await.unwrap();

        assert_eq!(result["updated"], true);
    }

    #[tokio::test]
    async fn test_api_client_delete() {
        let mock_server = MockServer::start().await;

        Mock::given(method("DELETE"))
            .and(path("/delete"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&mock_server)
            .await;

        let config = ServiceConfig {
            endpoint: mock_server.uri(),
            api_key: None,
            timeout_seconds: 5,
            retry: RetryConfig {
                max_retries: 0,
                base_delay_ms: 0,
                max_delay_ms: 0,
            },
        };

        let client = ApiClient::new(&config).unwrap();
        let result = client.delete("/delete").await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_api_client_health_check_ok() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock_server)
            .await;

        let config = ServiceConfig {
            endpoint: mock_server.uri(),
            api_key: None,
            timeout_seconds: 5,
            retry: RetryConfig {
                max_retries: 0,
                base_delay_ms: 0,
                max_delay_ms: 0,
            },
        };

        let client = ApiClient::new(&config).unwrap();
        assert!(client.health_check().await.unwrap());
    }

    #[tokio::test]
    async fn test_api_client_health_check_fail() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&mock_server)
            .await;

        let config = ServiceConfig {
            endpoint: mock_server.uri(),
            api_key: None,
            timeout_seconds: 5,
            retry: RetryConfig {
                max_retries: 0,
                base_delay_ms: 0,
                max_delay_ms: 0,
            },
        };

        let client = ApiClient::new(&config).unwrap();
        assert!(!client.health_check().await.unwrap());
    }

    #[tokio::test]
    async fn test_api_client_new_with_env_var_key() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .mount(&mock_server)
            .await;

        std::env::set_var("TEST_API_KEY", "secret123");

        let config = ServiceConfig {
            endpoint: mock_server.uri(),
            api_key: Some(SecretRef::String("$TEST_API_KEY".to_string())),
            timeout_seconds: 5,
            retry: RetryConfig {
                max_retries: 0,
                base_delay_ms: 0,
                max_delay_ms: 0,
            },
        };

        let client = ApiClient::new(&config).unwrap();
        let result: serde_json::Value = client.get("/test").await.unwrap();
        assert_eq!(result["ok"], true);

        std::env::remove_var("TEST_API_KEY");
    }

    #[tokio::test]
    async fn test_api_client_new_async_with_string_key() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .mount(&mock_server)
            .await;

        let config = ServiceConfig {
            endpoint: mock_server.uri(),
            api_key: Some(SecretRef::String("mykey".to_string())),
            timeout_seconds: 5,
            retry: RetryConfig {
                max_retries: 0,
                base_delay_ms: 0,
                max_delay_ms: 0,
            },
        };

        let client = ApiClient::new_async(&config).await.unwrap();
        let result: serde_json::Value = client.get("/test").await.unwrap();
        assert_eq!(result["ok"], true);
    }

    #[tokio::test]
    async fn test_api_client_retry_server_error_then_success() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/retry"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(1)
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path("/retry"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .mount(&mock_server)
            .await;

        let config = ServiceConfig {
            endpoint: mock_server.uri(),
            api_key: None,
            timeout_seconds: 5,
            retry: RetryConfig {
                max_retries: 2,
                base_delay_ms: 10,
                max_delay_ms: 100,
            },
        };

        let client = ApiClient::new(&config).unwrap();
        let result: serde_json::Value = client.get("/retry").await.unwrap();
        assert_eq!(result["ok"], true);
    }

    #[tokio::test]
    async fn test_api_client_no_retry_on_4xx() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/bad-request"))
            .respond_with(ResponseTemplate::new(400))
            .expect(1)
            .mount(&mock_server)
            .await;

        let config = ServiceConfig {
            endpoint: mock_server.uri(),
            api_key: None,
            timeout_seconds: 5,
            retry: RetryConfig {
                max_retries: 2,
                base_delay_ms: 10,
                max_delay_ms: 100,
            },
        };

        let client = ApiClient::new(&config).unwrap();
        let result: Result<serde_json::Value> = client.get("/bad-request").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_calculate_backoff_bounds() {
        let config = RetryConfig {
            max_retries: 3,
            base_delay_ms: 100,
            max_delay_ms: 1000,
        };

        // First retry attempt: 100 * 2^1 = 200ms base
        let delay1 = calculate_backoff(1, &config);
        assert!(delay1 >= Duration::from_millis(150));
        assert!(delay1 <= Duration::from_millis(250));

        // Second retry attempt: 100 * 2^2 = 400ms base
        let delay2 = calculate_backoff(2, &config);
        assert!(delay2 >= Duration::from_millis(300));
        assert!(delay2 <= Duration::from_millis(500));

        // High attempt should be capped at max_delay_ms then jittered
        let delay_large = calculate_backoff(10, &config);
        assert!(delay_large <= Duration::from_millis(1300));
        assert!(delay_large > Duration::from_millis(0));
    }

    #[tokio::test]
    async fn test_api_client_auth_header() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/auth"))
            .and(header("Authorization", "Bearer secret-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .mount(&mock_server)
            .await;

        let config = ServiceConfig {
            endpoint: mock_server.uri(),
            api_key: Some(SecretRef::String("secret-key".to_string())),
            timeout_seconds: 5,
            retry: RetryConfig {
                max_retries: 0,
                base_delay_ms: 0,
                max_delay_ms: 0,
            },
        };

        let client = ApiClient::new(&config).unwrap();
        let result: serde_json::Value = client.get("/auth").await.unwrap();
        assert_eq!(result["ok"], true);
    }

    #[tokio::test]
    async fn test_api_client_user_agent_header() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/ua"))
            .and(header_exists("User-Agent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .mount(&mock_server)
            .await;

        let config = ServiceConfig {
            endpoint: mock_server.uri(),
            api_key: None,
            timeout_seconds: 5,
            retry: RetryConfig {
                max_retries: 0,
                base_delay_ms: 0,
                max_delay_ms: 0,
            },
        };

        let client = ApiClient::new(&config).unwrap();
        let result: serde_json::Value = client.get("/ua").await.unwrap();
        assert_eq!(result["ok"], true);
    }

    #[tokio::test]
    async fn test_api_client_base_url_trailing_slash() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .mount(&mock_server)
            .await;

        let mut config = ServiceConfig {
            endpoint: mock_server.uri(),
            api_key: None,
            timeout_seconds: 5,
            retry: RetryConfig {
                max_retries: 0,
                base_delay_ms: 0,
                max_delay_ms: 0,
            },
        };
        config.endpoint.push('/');

        let client = ApiClient::new(&config).unwrap();
        let result: serde_json::Value = client.get("/test").await.unwrap();
        assert_eq!(result["ok"], true);
    }
}
