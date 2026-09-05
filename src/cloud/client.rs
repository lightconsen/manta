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
            // Expired or revoked session — forget the token so the UI flips
            // back to "Sign in" instead of reporting a dead signed-in state.
            let _ = crate::cloud::session::clear_token().await;
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

    /// GET /api/v1/kb — the user's knowledge bases (with quota).
    pub async fn list_kbs(&self) -> Result<Value> {
        let url = format!("{}/api/v1/kb", self.api_base);
        let resp = self.auth(self.http.get(&url)).send().await?;
        self.parse_response(resp, "GET /api/v1/kb").await
    }

    /// GET /api/v1/subscription — plan, credit balance, overdraft state.
    pub async fn subscription(&self) -> Result<Value> {
        self.get_json("/api/v1/subscription").await
    }

    /// GET /api/v1/usage?days= — credit consumption for the period.
    pub async fn usage(&self, days: u32) -> Result<Value> {
        let path = format!("/api/v1/usage?days={}", days.clamp(1, 365));
        self.get_json(&path).await
    }

    /// POST /api/v1/devices — bind this device to the account, returning the
    /// `device_token` (P2-9). Re-binds are idempotent; already-bound devices
    /// return the existing device with a null token.
    pub async fn bind_device(
        &self,
        device_id: &str,
        display_name: &str,
        public_key: Option<&str>,
    ) -> Result<Value> {
        let mut body = json!({
            "device_id": device_id,
            "display_name": display_name,
        });
        if let Some(key) = public_key {
            body["public_key"] = json!(key);
        }
        self.post_json("/api/v1/devices", body).await
    }

    /// GET a JSON endpoint (with the session token).
    async fn get_json(&self, path: &str) -> Result<Value> {
        let url = format!("{}{}", self.api_base, path);
        let resp = self.auth(self.http.get(&url)).send().await?;
        self.parse_response(resp, path).await
    }

    /// POST /api/v1/kb — create a knowledge base.
    pub async fn kb_create(&self, name: &str) -> Result<Value> {
        self.post_json("/api/v1/kb", json!({ "name": name })).await
    }

    /// DELETE /api/v1/kb/:id — delete a knowledge base (and its documents).
    ///
    /// Handled directly (not via `parse_response`): the cloud answers `204`
    /// with an empty body, which would fail JSON parsing.
    pub async fn kb_delete(&self, kb_id: &str) -> Result<()> {
        let url = format!("{}/api/v1/kb/{kb_id}", self.api_base);
        let resp = self.auth(self.http.delete(&url)).send().await?;
        let status = resp.status();
        if !status.is_success() {
            if status == reqwest::StatusCode::UNAUTHORIZED {
                let _ = crate::cloud::session::clear_token().await;
            }
            let text = resp.text().await?;
            return Err(SyscityError::Internal(format!(
                "cloud DELETE /api/v1/kb/{kb_id} status {status}: {text}"
            )));
        }
        Ok(())
    }

    /// POST /api/v1/kb/:id/query — semantic retrieval (§3.7).
    pub async fn kb_query(&self, kb_id: &str, query: &str, top_k: usize) -> Result<Value> {
        let path = format!("/api/v1/kb/{kb_id}/query");
        self.post_json(&path, json!({ "query": query, "top_k": top_k.min(50) }))
            .await
    }

    /// POST /api/v1/kb/:id/documents — upload a document (multipart).
    pub async fn kb_upload(
        &self,
        kb_id: &str,
        filename: &str,
        content: &[u8],
        mime: &str,
    ) -> Result<Value> {
        let url = format!("{}/api/v1/kb/{kb_id}/documents", self.api_base);
        let part = reqwest::multipart::Part::bytes(content.to_vec())
            .file_name(filename.to_string())
            .mime_str(mime)
            .map_err(|e| SyscityError::Internal(format!("invalid upload mime: {e}")))?;
        let form = reqwest::multipart::Form::new().part("file", part);
        let resp = self
            .auth(self.http.post(&url))
            .multipart(form)
            .send()
            .await?;
        self.parse_response(resp, "POST /api/v1/kb/documents").await
    }

    async fn post_json(&self, path: &str, body: Value) -> Result<Value> {
        let url = format!("{}{}", self.api_base, path);
        let resp = self.auth(self.http.post(&url)).json(&body).send().await?;
        self.parse_response(resp, path).await
    }

    /// Read a response: error on non-success status, else parse JSON.
    ///
    /// A `401 Unauthorized` means the stored session token is invalid, expired,
    /// or revoked — forget it (fail-safe: the UI then reports signed-out and
    /// the user re-logs-in) before surfacing the error.
    async fn parse_response(&self, resp: reqwest::Response, what: &str) -> Result<Value> {
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            if status == reqwest::StatusCode::UNAUTHORIZED {
                let _ = crate::cloud::session::clear_token().await;
            }
            return Err(SyscityError::Internal(format!("cloud {what} status {status}: {text}")));
        }
        Ok(serde_json::from_str(&text)?)
    }
}
