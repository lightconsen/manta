//! HTTP client for plugins
//!
//! Blocking HTTP calls from within the WASM sandbox.

use std::string::String;
use crate::ffi_call_to_string;

const JSON_CONTENT_TYPE: &str = "application/json";

/// Perform a GET request. Returns the response body as a string.
///
/// # Example
/// ```ignore
/// let body = http::get("https://api.example.com/data")?;
/// let data: serde_json::Value = serde_json::from_str(&body)?;
/// ```
pub fn get(url: &str) -> Option<String> {
    ffi_call_to_string(|out_ptr, out_len| unsafe {
        super::http_get(url.as_ptr(), url.len(), out_ptr, out_len)
    })
}

/// Perform a POST request with a JSON body. Returns the response body.
///
/// # Example
/// ```ignore
/// let payload = serde_json::json!({ "query": "hello" });
/// let body = http::post_json("https://api.example.com/search", &payload)?;
/// ```
pub fn post_json(url: &str, body: &serde_json::Value) -> Option<String> {
    let body_str = serde_json::to_string(body).ok()?;
    post(url, &body_str, JSON_CONTENT_TYPE)
}

/// Perform a POST request with a custom body and content type.
///
/// # Example
/// ```ignore
/// let body = http::post("https://api.example.com/webhook", "raw data", "text/plain")?;
/// ```
pub fn post(url: &str, body: &str, content_type: &str) -> Option<String> {
    ffi_call_to_string(|out_ptr, out_len| unsafe {
        super::http_post(
            url.as_ptr(), url.len(),
            body.as_ptr(), body.len(),
            content_type.as_ptr(), content_type.len(),
            out_ptr, out_len,
        )
    })
}
