//! Cloud API client: OpenAI-compatible `/v1/*` + `/api/v1/*`, Bearer-authed.

use serde_json::{json, Value};

use crate::cloud::config::CloudConfig;
use crate::error::{Result, SyscityError};

/// Thin HTTP client for the cloud API. Built per-operation with a session
/// token; callers are gated by `cloud.enabled` + logged-in before use.
pub struct CloudClient {
    http: reqwest::Client,
    api_base: String,
    token: String,
}

impl CloudClient {
    pub fn new(cfg: &CloudConfig, token: String) -> Self {
        let http = reqwest::Client::builder()
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            http,
            api_base: cfg.api_base.trim_end_matches('/').to_string(),
            token,
        }
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.bearer_auth(&self.token)
    }

    /// GET /auth/me → the logged-in user, `None` when the token is invalid.
    pub async fn me(&self) -> Result<Option<Value>> {
        let url = format!("{}/auth/me", self.api_base);
        let resp = self.auth(self.http.get(&url)).send().await?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(SyscityError::Internal(format!("cloud /auth/me status {}", resp.status())));
        }
        Ok(Some(resp.json().await?))
    }

    /// POST /v1/chat/completions (OpenAI-compatible).
    pub async fn chat(&self, body: Value) -> Result<Value> {
        self.post_json("/v1/chat/completions", body).await
    }

    /// POST /v1/embeddings.
    pub async fn embeddings(&self, body: Value) -> Result<Value> {
        self.post_json("/v1/embeddings", body).await
    }

    /// POST /v1/search (web search aggregation).
    pub async fn search(&self, query: &str, max: u32) -> Result<Value> {
        self.post_json("/v1/search", json!({ "query": query, "max_results": max }))
            .await
    }

    async fn post_json(&self, path: &str, body: Value) -> Result<Value> {
        let url = format!("{}{}", self.api_base, path);
        let resp = self.auth(self.http.post(&url)).json(&body).send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(SyscityError::Internal(format!("cloud {path} status {status}: {text}")));
        }
        Ok(serde_json::from_str(&text)?)
    }
}
