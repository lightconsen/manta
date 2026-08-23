//! Content extraction and script/input actions.

use super::{BrowserAction, BrowserScreenshot};
use serde_json::{json, Value};

pub(super) async fn execute_content_actions(
    action: BrowserAction,
    page: &chromiumoxide::Page,
    _browser: Option<&chromiumoxide::Browser>,
    _screenshot_data: &mut Option<BrowserScreenshot>,
) -> Result<serde_json::Value, String> {
    match action {
        BrowserAction::GetHtml => match page.content().await {
            Ok(html) => Ok(json!({
                "success": true,
                "html": html,
                "length": html.len()
            })),
            Err(e) => Err(format!("Failed to get HTML: {}", e)),
        },

        BrowserAction::GetText { selector } => match selector {
            Some(sel) => match page.find_element(&sel).await {
                Ok(elem) => match elem.inner_text().await {
                    Ok(Some(text)) => Ok(json!({
                        "success": true,
                        "text": text,
                        "selector": sel
                    })),
                    Ok(None) => Ok(json!({
                        "success": true,
                        "text": "",
                        "selector": sel
                    })),
                    Err(e) => Err(format!("Failed to get text: {}", e)),
                },
                Err(e) => Err(format!("Element not found: {}", e)),
            },
            None => {
                let script = r#"() => document.body.innerText"#;
                match page.evaluate(script).await {
                    Ok(result) => {
                        let text = result.into_value::<String>().unwrap_or_default();
                        Ok(json!({
                            "success": true,
                            "text": text
                        }))
                    }
                    Err(e) => Err(format!("Failed to get page text: {}", e)),
                }
            }
        },

        BrowserAction::Snapshot { max_chars } => {
            let max = max_chars.unwrap_or(8000);
            match crate::browser::aria_snapshot(page, max).await {
                Ok(snapshot) => {
                    let text = snapshot.to_text();
                    Ok(json!({
                        "success": true,
                        "snapshot": text,
                        "url": snapshot.url,
                        "title": snapshot.title,
                        "interactive_count": snapshot.interactive_count(),
                        "truncated": snapshot.truncated
                    }))
                }
                Err(e) => Err(format!("Failed to take ARIA snapshot: {}", e)),
            }
        }

        BrowserAction::GetPerformanceMetrics => {
            let script = r#"() => {
                    const nav = performance.getEntriesByType('navigation')[0] || {};
                    return {
                        navigation: {
                            dns_lookup: nav.domainLookupEnd - nav.domainLookupStart,
                            connection_time: nav.connectEnd - nav.connectStart,
                            response_time: nav.responseEnd - nav.responseStart,
                            dom_interactive: nav.domInteractive,
                            dom_complete: nav.domComplete,
                            load_event: nav.loadEventEnd - nav.loadEventStart,
                            transfer_size: nav.transferSize,
                            decoded_body_size: nav.decodedBodySize
                        },
                        memory: performance.memory ? {
                            used_js_heap_size: performance.memory.usedJSHeapSize,
                            total_js_heap_size: performance.memory.totalJSHeapSize,
                            js_heap_size_limit: performance.memory.jsHeapSizeLimit
                        } : null
                    };
                }"#;
            match page.evaluate(script).await {
                Ok(result) => {
                    let metrics = result.value().cloned().unwrap_or(json!(null));
                    Ok(json!({ "success": true, "metrics": metrics }))
                }
                Err(e) => Err(format!("Failed to get performance metrics: {}", e)),
            }
        }

        BrowserAction::GetConsoleMessages { level, limit } => {
            crate::browser::instrument::ensure_instrumented(page).await?;
            let script = r#"() => (window.__syscity_console || [])"#;
            match page.evaluate(script).await {
                Ok(result) => {
                    let entries = result.into_value::<Vec<Value>>().unwrap_or_default();
                    let filtered: Vec<Value> = entries
                        .into_iter()
                        .filter(|e| {
                            level.as_ref().is_none_or(|l| {
                                e.get("level")
                                    .and_then(|v| v.as_str())
                                    .is_some_and(|v| v.eq_ignore_ascii_case(l))
                            })
                        })
                        .collect();
                    let total = filtered.len();
                    let mut messages: Vec<Value> =
                        filtered.into_iter().take(limit.unwrap_or(100)).collect();
                    // Best-effort: resolve error/warn stacks via source maps.
                    crate::browser::sourcemap::sourcemap_messages(&mut messages).await;
                    Ok(json!({
                        "success": true,
                        "messages": messages,
                        "count": messages.len(),
                        "total": total
                    }))
                }
                Err(e) => Err(format!("Failed to get console messages: {}", e)),
            }
        }

        BrowserAction::ExecuteScript { script } => {
            match page
                .evaluate(format!("() => {{ {} }}", script).as_str())
                .await
            {
                Ok(result) => {
                    let value = result.value().cloned().unwrap_or(json!(null));
                    Ok(json!({
                        "success": true,
                        "result": value
                    }))
                }
                Err(e) => Err(format!("Script execution failed: {}", e)),
            }
        }

        BrowserAction::Press { key } => {
            let script = format!(
                r#"() => {{
                        const el = document.activeElement || document.body;
                        const evt = new KeyboardEvent('keydown', {{ key: '{}', bubbles: true }});
                        el.dispatchEvent(evt);
                        const evtUp = new KeyboardEvent('keyup', {{ key: '{}', bubbles: true }});
                        el.dispatchEvent(evtUp);
                        return true;
                    }}"#,
                key, key
            );
            match page.evaluate(script.as_str()).await {
                Ok(_) => {
                    crate::browser::instrument::auto_wait(page).await;
                    Ok(json!({ "success": true, "key": key }))
                }
                Err(e) => Err(format!("Failed to press key: {}", e)),
            }
        }

        BrowserAction::Act { ref_id, action } => {
            match crate::browser::act_by_ref(page, ref_id, action).await {
                Ok(msg) => {
                    crate::browser::instrument::auto_wait(page).await;
                    Ok(json!({ "success": true, "message": msg }))
                }
                Err(e) => Err(format!("Failed to act on ref {}: {}", ref_id, e)),
            }
        }
        _ => Err("browser: action not handled by this group".to_string()),
    }
}
