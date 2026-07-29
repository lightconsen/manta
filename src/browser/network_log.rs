//! CDP Network-domain capture for the browser tool.
//!
//! Unlike the injected JS shim (which only sees fetch/XHR), this subscribes to
//! the Chrome DevTools Protocol Network domain and records **all** request
//! types (document, XHR, fetch, script, img, css, websocket, ...). Response
//! bodies are fetched lazily via `Network.getResponseBody` only for the
//! entries actually returned by `GetNetworkLog`.
//!
//! Capture is started lazily per page and keyed by target id; records are
//! kept in a bounded ring buffer (newest 500).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, OnceLock};

use chromiumoxide::cdp::browser_protocol::network::{
    EnableParams, EventLoadingFailed, EventLoadingFinished, EventRequestWillBeSent,
    EventResponseReceived, GetResponseBodyParams,
};
use chromiumoxide::Page;
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tracing::debug;

/// Maximum retained records per page.
const MAX_RECORDS: usize = 500;

/// A single captured network request.
#[derive(Debug, Clone)]
pub struct NetworkRecord {
    /// CDP request id (used for lazy body fetch)
    pub request_id: String,
    /// Request URL
    pub url: String,
    /// HTTP method
    pub method: String,
    /// Resource type (document, xhr, fetch, script, img, stylesheet, ...)
    pub resource_type: Option<String>,
    /// HTTP status code (None until response received)
    pub status: Option<i64>,
    /// MIME type from the response
    pub mime_type: Option<String>,
    /// Monotonic start timestamp (seconds)
    pub start_time: f64,
    /// Wall duration in ms once loading finished/failed
    pub duration_ms: Option<f64>,
    /// Failure text when loading failed
    pub error: Option<String>,
}

type Records = Arc<Mutex<VecDeque<NetworkRecord>>>;

fn logs() -> &'static Mutex<HashMap<String, Records>> {
    static LOGS: OnceLock<Mutex<HashMap<String, Records>>> = OnceLock::new();
    LOGS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Start CDP network capture for a page. Idempotent per target id.
pub async fn start_capture(page: &Page) -> Result<(), String> {
    let key = page.target_id().as_ref().to_string();
    {
        let logs = logs().lock().await;
        if logs.contains_key(&key) {
            return Ok(());
        }
    }

    let records: Records = Arc::new(Mutex::new(VecDeque::new()));

    let mut req_events = page
        .event_listener::<EventRequestWillBeSent>()
        .await
        .map_err(|e| format!("Failed to subscribe network events: {}", e))?;
    let mut resp_events = page
        .event_listener::<EventResponseReceived>()
        .await
        .map_err(|e| format!("Failed to subscribe network events: {}", e))?;
    let mut fin_events = page
        .event_listener::<EventLoadingFinished>()
        .await
        .map_err(|e| format!("Failed to subscribe network events: {}", e))?;
    let mut fail_events = page
        .event_listener::<EventLoadingFailed>()
        .await
        .map_err(|e| format!("Failed to subscribe network events: {}", e))?;

    page.execute(EnableParams::default())
        .await
        .map_err(|e| format!("Failed to enable Network domain: {}", e))?;

    let starts: Arc<Mutex<HashMap<String, f64>>> = Arc::new(Mutex::new(HashMap::new()));

    {
        let records = records.clone();
        let starts = starts.clone();
        tokio::spawn(async move {
            while let Some(ev) = req_events.next().await {
                let ts = *ev.timestamp.inner();
                starts
                    .lock()
                    .await
                    .insert(ev.request_id.as_ref().to_string(), ts);
                let mut guard = records.lock().await;
                if guard.len() >= MAX_RECORDS {
                    guard.pop_front();
                }
                guard.push_back(NetworkRecord {
                    request_id: ev.request_id.as_ref().to_string(),
                    url: ev.request.url.clone(),
                    method: ev.request.method.clone(),
                    resource_type: None,
                    status: None,
                    mime_type: None,
                    start_time: ts,
                    duration_ms: None,
                    error: None,
                });
            }
            debug!("network requestWillBeSent stream ended");
        });
    }
    {
        let records = records.clone();
        tokio::spawn(async move {
            while let Some(ev) = resp_events.next().await {
                let mut guard = records.lock().await;
                if let Some(rec) = guard
                    .iter_mut()
                    .find(|r| r.request_id == ev.request_id.as_ref())
                {
                    rec.status = Some(ev.response.status);
                    rec.url = ev.response.url.clone();
                    rec.mime_type = Some(ev.response.mime_type.clone());
                    rec.resource_type = Some(format!("{:?}", ev.r#type).to_lowercase());
                }
            }
            debug!("network responseReceived stream ended");
        });
    }
    {
        let records = records.clone();
        let starts = starts.clone();
        tokio::spawn(async move {
            while let Some(ev) = fin_events.next().await {
                let start = starts.lock().await.get(ev.request_id.as_ref()).copied();
                let mut guard = records.lock().await;
                if let Some(rec) = guard
                    .iter_mut()
                    .find(|r| r.request_id == ev.request_id.as_ref())
                {
                    if let Some(start) = start {
                        rec.duration_ms = Some((*ev.timestamp.inner() - start) * 1000.0);
                    }
                }
            }
            debug!("network loadingFinished stream ended");
        });
    }
    {
        let records = records.clone();
        tokio::spawn(async move {
            while let Some(ev) = fail_events.next().await {
                let mut guard = records.lock().await;
                if let Some(rec) = guard
                    .iter_mut()
                    .find(|r| r.request_id == ev.request_id.as_ref())
                {
                    rec.error = Some(ev.error_text.clone());
                }
            }
            debug!("network loadingFailed stream ended");
        });
    }

    logs().lock().await.insert(key, records);
    Ok(())
}

/// Query captured network records with filters and pagination.
///
/// When `include_body` is set, bodies are fetched lazily via
/// `Network.getResponseBody` for the returned page of entries only (failures
/// are silently omitted — some bodies, e.g. redirects or streams, are not
/// retrievable).
#[allow(clippy::too_many_arguments)]
pub async fn query(
    page: &Page,
    url: Option<&str>,
    method: Option<&str>,
    min_status: Option<u16>,
    max_status: Option<u16>,
    resource_type: Option<&str>,
    include_body: bool,
    limit: usize,
    offset: usize,
) -> Result<Value, String> {
    let key = page.target_id().as_ref().to_string();
    let records = {
        let logs = logs().lock().await;
        logs.get(&key).cloned()
    };
    let Some(records) = records else {
        return Err("Network capture not active for this page".to_string());
    };

    let filtered: Vec<NetworkRecord> = {
        let guard = records.lock().await;
        guard
            .iter()
            .filter(|r| url.is_none_or(|f| r.url.contains(f)))
            .filter(|r| method.is_none_or(|m| r.method.eq_ignore_ascii_case(m)))
            .filter(|r| {
                resource_type.is_none_or(|t| {
                    r.resource_type
                        .as_deref()
                        .is_some_and(|rt| rt.eq_ignore_ascii_case(t))
                })
            })
            .filter(|r| {
                min_status.is_none_or(|min| r.status.is_some_and(|s| s >= i64::from(min)))
                    && max_status.is_none_or(|max| r.status.is_some_and(|s| s <= i64::from(max)))
            })
            .cloned()
            .collect()
    };

    let total = filtered.len();
    let page_records: Vec<NetworkRecord> = filtered.into_iter().skip(offset).take(limit).collect();

    let mut entries = Vec::with_capacity(page_records.len());
    for rec in page_records {
        let mut entry = json!({
            "url": rec.url,
            "method": rec.method,
            "resource_type": rec.resource_type,
            "status": rec.status,
            "mime_type": rec.mime_type,
            "duration_ms": rec.duration_ms.map(|d| d.round()),
            "error": rec.error,
        });
        if include_body {
            match page
                .execute(GetResponseBodyParams::new(rec.request_id.clone()))
                .await
            {
                Ok(resp) => {
                    let body = resp.result.body;
                    const MAX_BODY: usize = 8192;
                    if body.len() > MAX_BODY {
                        entry["body"] = json!(format!("{}…[truncated]", &body[..MAX_BODY]));
                        entry["body_truncated"] = json!(true);
                    } else {
                        entry["body"] = json!(body);
                    }
                    if resp.result.base64_encoded {
                        entry["body_base64"] = json!(true);
                    }
                }
                Err(e) => {
                    debug!("getResponseBody failed for {}: {}", rec.request_id, e);
                }
            }
        }
        entries.push(entry);
    }

    Ok(json!({
        "success": true,
        "entries": entries,
        "count": entries.len(),
        "total": total,
        "offset": offset,
        "source": "cdp",
    }))
}

/// Remove captured records for a page (used by ClearCaptures).
pub async fn clear(page: &Page) {
    let key = page.target_id().as_ref().to_string();
    let records = {
        let logs = logs().lock().await;
        logs.get(&key).cloned()
    };
    if let Some(records) = records {
        records.lock().await.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// E2E against real Chrome: CDP capture records all request types.
    /// Run with: cargo test --features browser -- --ignored
    #[tokio::test]
    #[ignore = "requires a local Chrome installation"]
    async fn test_cdp_network_capture_e2e() {
        use chromiumoxide::browser::{Browser, BrowserConfig};

        // Minimal local HTTP server: one HTML page + one JSON fetch.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let body = if req.starts_with("GET /data.json") {
                        "{\"ok\":true}"
                    } else {
                        "<html><body><script>fetch('/data.json')</script></body></html>"
                    };
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        if body.starts_with('{') { "application/json" } else { "text/html" },
                        body
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });

        let (browser, mut handler) = Browser::launch(
            BrowserConfig::builder()
                .arg("--headless=new")
                .user_data_dir(std::env::temp_dir().join("syscity-e2e-netlog"))
                .build()
                .unwrap(),
        )
        .await
        .expect("launch Chrome");
        tokio::spawn(async move { while handler.next().await.is_some() {} });

        let page = browser.new_page("about:blank").await.unwrap();
        start_capture(&page).await.unwrap();
        page.goto(format!("http://127.0.0.1:{port}/"))
            .await
            .unwrap();
        // Give the page a moment to fire the fetch.
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        let result = query(&page, None, None, None, None, None, false, 50, 0)
            .await
            .unwrap();
        let entries = result["entries"].as_array().unwrap();
        let types: Vec<&str> = entries
            .iter()
            .filter_map(|e| e["resource_type"].as_str())
            .collect();
        assert!(
            types.iter().any(|t| *t == "document"),
            "should capture the document request, got {types:?}"
        );
        assert!(
            types.iter().any(|t| *t == "fetch"),
            "should capture the fetch request, got {types:?}"
        );

        // Body fetch for the JSON entry.
        let result = query(&page, Some("data.json"), None, None, None, None, true, 50, 0)
            .await
            .unwrap();
        let entries = result["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["body"].as_str(), Some("{\"ok\":true}"));
    }
}
