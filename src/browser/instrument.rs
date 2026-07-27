//! Page instrumentation for the browser tool.
//!
//! Injects a small JavaScript shim into pages (before any page script runs,
//! via `Page.addScriptToEvaluateOnNewDocument`) that records:
//!
//! - **Network**: fetch/XHR calls with method, url, status, timing and a
//!   truncated text body, exposed on `window.__syscity_net`.
//! - **Console**: `console.*` calls and uncaught errors with stack traces,
//!   exposed on `window.__syscity_console`.
//! - **Pending requests**: `window.__syscity_pending` counts in-flight
//!   fetch/XHR requests, used by [`auto_wait`] to detect network idle.
//!
//! The shim approach works identically for pool-based persistent pages and
//! legacy per-call browsers, and avoids managing CDP event subscriptions.

use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams;
use chromiumoxide::Page;
use tracing::{debug, warn};

/// JavaScript shim injected into every page (idempotent).
pub const INSTRUMENT_SCRIPT: &str = r#"(() => {
  if (window.__syscity_instrumented) return;
  window.__syscity_instrumented = true;
  window.__syscity_net = [];
  window.__syscity_console = [];
  window.__syscity_pending = 0;

  const MAX_ENTRIES = 200;
  const MAX_BODY = 8192;

  const pushCapped = (arr, item) => {
    arr.push(item);
    if (arr.length > MAX_ENTRIES) arr.splice(0, arr.length - MAX_ENTRIES);
  };

  const recordBody = (entry, promise) => {
    promise
      .then((text) => {
        if (typeof text === "string") {
          entry.body = text.length > MAX_BODY ? text.slice(0, MAX_BODY) + "…[truncated]" : text;
          entry.body_truncated = text.length > MAX_BODY;
        }
      })
      .catch(() => {});
  };

  // ── fetch ──
  const origFetch = window.fetch;
  window.fetch = function (...args) {
    const start = performance.now();
    const method = (args[1] && args[1].method) || "GET";
    const url = typeof args[0] === "string" ? args[0] : (args[0] && args[0].url) || "";
    const entry = { type: "fetch", method, url, start_time: start };
    window.__syscity_pending++;
    return origFetch
      .apply(this, args)
      .then((resp) => {
        entry.status = resp.status;
        entry.duration = performance.now() - start;
        try {
          recordBody(entry, resp.clone().text());
        } catch (_) {}
        pushCapped(window.__syscity_net, entry);
        window.__syscity_pending--;
        return resp;
      })
      .catch((err) => {
        entry.error = String(err);
        entry.duration = performance.now() - start;
        pushCapped(window.__syscity_net, entry);
        window.__syscity_pending--;
        throw err;
      });
  };

  // ── XMLHttpRequest ──
  const origOpen = XMLHttpRequest.prototype.open;
  const origSend = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.open = function (method, url, ...rest) {
    this.__syscity_meta = { method, url: String(url), start_time: 0 };
    return origOpen.call(this, method, url, ...rest);
  };
  XMLHttpRequest.prototype.send = function (...args) {
    const meta = this.__syscity_meta || { method: "GET", url: "" };
    meta.start_time = performance.now();
    window.__syscity_pending++;
    this.addEventListener("loadend", () => {
      const entry = {
        type: "xhr",
        method: meta.method,
        url: meta.url,
        status: this.status,
        duration: performance.now() - meta.start_time,
        start_time: meta.start_time,
      };
      try {
        const text = this.responseText;
        if (typeof text === "string") {
          entry.body = text.length > MAX_BODY ? text.slice(0, MAX_BODY) + "…[truncated]" : text;
          entry.body_truncated = text.length > MAX_BODY;
        }
      } catch (_) {}
      pushCapped(window.__syscity_net, entry);
      window.__syscity_pending--;
    });
    return origSend.apply(this, args);
  };

  // ── console ──
  const capture = (level, args) => {
    try {
      pushCapped(window.__syscity_console, {
        level,
        text: args
          .map((a) => {
            try {
              return typeof a === "object" ? JSON.stringify(a) : String(a);
            } catch (_) {
              return String(a);
            }
          })
          .join(" "),
        stack: level === "error" || level === "warn" ? new Error().stack || "" : "",
        timestamp: Date.now(),
      });
    } catch (_) {}
  };
  for (const level of ["log", "info", "warn", "error", "debug"]) {
    const orig = console[level];
    console[level] = function (...args) {
      capture(level, args);
      return orig.apply(this, args);
    };
  }
  window.addEventListener("error", (e) => {
    pushCapped(window.__syscity_console, {
      level: "exception",
      text: e.message,
      stack: e.error && e.error.stack ? e.error.stack : "",
      timestamp: Date.now(),
    });
  });
  window.addEventListener("unhandledrejection", (e) => {
    pushCapped(window.__syscity_console, {
      level: "exception",
      text: "Unhandled rejection: " + String(e.reason),
      stack: e.reason && e.reason.stack ? e.reason.stack : "",
      timestamp: Date.now(),
    });
  });
})()"#;

/// Inject the instrumentation shim into the page and register it for all
/// future navigations. Idempotent: safe to call multiple times.
pub async fn ensure_instrumented(page: &Page) -> Result<(), String> {
    // Register for future documents (survives navigation). Ignore errors from
    // older Chrome versions lacking the command.
    if let Err(e) = page
        .execute(AddScriptToEvaluateOnNewDocumentParams::new(INSTRUMENT_SCRIPT))
        .await
    {
        debug!("addScriptToEvaluateOnNewDocument failed (non-fatal): {}", e);
    }
    // Inject into the current document if not already present.
    page.evaluate(INSTRUMENT_SCRIPT)
        .await
        .map_err(|e| format!("Failed to inject instrumentation: {}", e))?;
    Ok(())
}

/// Wait for the page to settle after an action: pending navigation plus
/// network-idle (no in-flight fetch/XHR) and `document.readyState ===
/// "complete"`. All waits are bounded; this never blocks longer than ~5s.
pub async fn auto_wait(page: &Page) {
    // A click/type may trigger a navigation — give it a short window.
    let _ = tokio::time::timeout(Duration::from_millis(1500), page.wait_for_navigation()).await;

    // Network-idle + readyState settle loop.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut stable_polls = 0u8;
    loop {
        if tokio::time::Instant::now() >= deadline || stable_polls >= 2 {
            break;
        }
        let settled = page
            .evaluate(
                r#"() => ({
                    pending: window.__syscity_pending || 0,
                    ready: document.readyState === "complete"
                })"#,
            )
            .await
            .ok()
            .and_then(|r| r.into_value::<serde_json::Value>().ok())
            .map(|v| {
                v.get("pending").and_then(|p| p.as_u64()).unwrap_or(0) == 0
                    && v.get("ready").and_then(|r| r.as_bool()).unwrap_or(true)
            })
            .unwrap_or(true);
        if settled {
            stable_polls += 1;
        } else {
            stable_polls = 0;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    if stable_polls < 2 {
        warn!("auto_wait: page did not fully settle within the time budget");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// E2E against real Chrome: instrumentation captures fetch + console, and
    /// auto_wait returns. Run with: cargo test --features browser -- --ignored
    #[tokio::test]
    #[ignore = "requires a local Chrome installation"]
    async fn test_instrumentation_e2e() {
        use chromiumoxide::browser::{Browser, BrowserConfig};
        use futures::StreamExt;

        let (browser, mut handler) = Browser::launch(
            BrowserConfig::builder()
                .arg("--headless=new")
                .user_data_dir(std::env::temp_dir().join("syscity-e2e-instrument"))
                .build()
                .unwrap(),
        )
        .await
        .expect("launch Chrome");
        tokio::spawn(async move { while handler.next().await.is_some() {} });

        let page = browser.new_page("about:blank").await.unwrap();
        ensure_instrumented(&page).await.unwrap();

        page.goto("data:text/html,<html><body><script>console.warn('hi-shim');fetch('data:application/json,{\"a\":1}')</script></body></html>")
            .await
            .unwrap();
        auto_wait(&page).await;

        let console: serde_json::Value = page
            .evaluate(r#"() => window.__syscity_console"#)
            .await
            .unwrap()
            .into_value()
            .unwrap();
        let entries = console.as_array().unwrap();
        assert!(
            entries.iter().any(|e| e.get("text").and_then(|t| t.as_str()) == Some("hi-shim")),
            "console capture should contain the warn message, got {entries:?}"
        );
    }
}
