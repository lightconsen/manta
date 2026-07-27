//! Best-effort sourcemap resolution for console error stack traces.
//!
//! V8 stack frames reference bundled/minified files (`app.js:1:23456`). This
//! module fetches the corresponding source maps (via the `sourceMappingURL`
//! comment or the conventional `<url>.map` location), caches them per script
//! URL, and rewrites frames to their original source positions. Frames that
//! cannot be resolved are left unchanged.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use sourcemap::SourceMap;
use tokio::sync::Mutex;
use tracing::debug;

/// Cache of fetch results per script URL: `None` means "no map available".
fn map_cache() -> &'static Mutex<HashMap<String, Option<Arc<SourceMap>>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Arc<SourceMap>>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A parsed V8 stack frame location.
#[derive(Debug, PartialEq)]
struct FrameLoc {
    url: String,
    line: u32,
    col: u32,
}

/// Extract `url:line:col` from a V8 stack line, e.g.
/// `at foo (https://h/app.js:1:234)` or `at https://h/app.js:1:234`.
fn parse_frame(line: &str) -> Option<FrameLoc> {
    let candidate = if let Some(start) = line.rfind('(') {
        let end = line.rfind(')')?;
        line.get(start + 1..end)?
    } else {
        line.trim().strip_prefix("at ")?.trim()
    };
    let candidate = candidate.strip_prefix("async ").unwrap_or(candidate);
    if !candidate.contains("://") {
        return None;
    }
    let (path, col) = candidate.rsplit_once(':')?;
    let (url, line) = path.rsplit_once(':')?;
    Some(FrameLoc {
        url: url.to_string(),
        line: line.parse().ok()?,
        col: col.parse().ok()?,
    })
}

/// Fetch and parse the source map for a script URL, with caching.
async fn load_map(client: &reqwest::Client, script_url: &str) -> Option<Arc<SourceMap>> {
    {
        let cache = map_cache().lock().await;
        if let Some(cached) = cache.get(script_url) {
            return cached.clone();
        }
    }

    let resolved = async {
        let script = client.get(script_url).send().await.ok()?.text().await.ok()?;
        let map_url = script
            .lines()
            .rev()
            .take(5)
            .find_map(|l| l.trim().strip_prefix("//# sourceMappingURL="))
            .map(|u| {
                if u.starts_with("http") {
                    u.to_string()
                } else {
                    // Resolve relative to the script URL.
                    match script_url.rfind('/') {
                        Some(idx) => format!("{}/{}", &script_url[..idx], u),
                        None => u.to_string(),
                    }
                }
            })
            .unwrap_or_else(|| format!("{script_url}.map"));
        let map_text = client.get(&map_url).send().await.ok()?.text().await.ok()?;
        SourceMap::from_reader(map_text.as_bytes()).ok()
    }
    .await;

    let result = resolved.map(Arc::new);
    map_cache()
        .lock()
        .await
        .insert(script_url.to_string(), result.clone());
    result
}

/// Rewrite a V8 stack trace, resolving frames through source maps where
/// possible. Unresolvable frames are kept as-is.
pub async fn sourcemap_stack(client: &reqwest::Client, stack: &str) -> String {
    let mut out = String::with_capacity(stack.len());
    for (i, line) in stack.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let Some(frame) = parse_frame(line) else {
            out.push_str(line);
            continue;
        };
        let Some(map) = load_map(client, &frame.url).await else {
            out.push_str(line);
            continue;
        };
        // V8 lines are 1-based, columns 0-based; sourcemap crate wants
        // 0-based lines.
        match map.lookup_token(frame.line.saturating_sub(1), frame.col) {
            Some(token) => {
                let src = token.get_source().unwrap_or("?");
                out.push_str(&format!(
                    "{} [{}:{}:{}]",
                    line,
                    src,
                    token.get_src_line() + 1,
                    token.get_src_col()
                ));
            }
            None => out.push_str(line),
        }
    }
    out
}

/// Rewrite the `stack` field of every console message entry in place.
pub async fn sourcemap_messages(messages: &mut [serde_json::Value]) {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            debug!("sourcemap: http client unavailable: {}", e);
            return;
        }
    };
    for msg in messages.iter_mut() {
        let Some(stack) = msg.get("stack").and_then(|s| s.as_str()) else {
            continue;
        };
        if stack.is_empty() {
            continue;
        }
        let mapped = sourcemap_stack(&client, stack).await;
        if mapped != stack {
            msg["stack"] = serde_json::json!(mapped);
            msg["sourcemapped"] = serde_json::json!(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_frame_parens() {
        let f = parse_frame("    at foo (https://example.com/app.js:1:234)").unwrap();
        assert_eq!(f.url, "https://example.com/app.js");
        assert_eq!(f.line, 1);
        assert_eq!(f.col, 234);
    }

    #[test]
    fn test_parse_frame_bare() {
        let f = parse_frame("    at https://cdn.example.com/lib.js:10:5").unwrap();
        assert_eq!(f.url, "https://cdn.example.com/lib.js");
        assert_eq!(f.line, 10);
        assert_eq!(f.col, 5);
    }

    #[test]
    fn test_parse_frame_async() {
        let f = parse_frame("    at async https://h.test/x.js:3:7").unwrap();
        assert_eq!(f.url, "https://h.test/x.js");
        assert_eq!(f.line, 3);
        assert_eq!(f.col, 7);
    }

    #[test]
    fn test_parse_frame_non_url() {
        assert!(parse_frame("    at <anonymous>:1:1").is_none());
        assert!(parse_frame("Error: boom").is_none());
    }
}
