//! Form filling, text input, drag/select and download behavior.

use super::{BrowserAction, BrowserScreenshot};
use serde_json::json;

use tracing::warn;

pub(super) async fn execute_form_actions(
    action: BrowserAction,
    page: &chromiumoxide::Page,
    browser: Option<&chromiumoxide::Browser>,
    _screenshot_data: &mut Option<BrowserScreenshot>,
) -> Result<serde_json::Value, String> {
    match action {
        BrowserAction::Type { selector, text, clear } => match page.find_element(&selector).await {
            Ok(elem) => {
                if clear.unwrap_or(true) {
                    if elem.click().await.is_err() {
                        warn!("Failed to click browser element before typing");
                    }
                    if elem.click().await.is_err() {
                        warn!("Failed to click browser element before typing");
                    }
                }
                match elem.type_str(&text).await {
                    Ok(_) => {
                        crate::browser::instrument::auto_wait(page).await;
                        Ok(json!({
                            "success": true,
                            "selector": selector,
                            "text_length": text.len()
                        }))
                    }
                    Err(e) => Err(format!("Failed to type: {}", e)),
                }
            }
            Err(e) => Err(format!("Element not found: {}", e)),
        },

        BrowserAction::FillForm { fields } => {
            let mut filled = 0usize;
            let mut errors = Vec::new();
            for field in &fields {
                match page.find_element(&field.selector).await {
                    Ok(elem) => {
                        if field.clear.unwrap_or(true) {
                            let _ = elem.click().await;
                        }
                        match elem.type_str(&field.value).await {
                            Ok(_) => filled += 1,
                            Err(e) => errors.push(format!("{}: {}", field.selector, e)),
                        }
                    }
                    Err(e) => errors.push(format!("{}: {}", field.selector, e)),
                }
            }
            crate::browser::instrument::auto_wait(page).await;
            if errors.is_empty() {
                Ok(json!({ "success": true, "filled": filled }))
            } else {
                Err(format!(
                    "Filled {}/{} fields; errors: {}",
                    filled,
                    fields.len(),
                    errors.join("; ")
                ))
            }
        }

        BrowserAction::Select { selector, text, start, end } => {
            let script = if let Some(text) = text {
                format!(
                    r#"() => {{
                            const el = document.querySelector('{}');
                            if (!el) return {{ error: 'Element not found' }};
                            if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') {{
                                el.focus();
                                const idx = el.value.indexOf('{}');
                                if (idx >= 0) {{
                                    el.setSelectionRange(idx, idx + {});
                                    return {{ success: true, selected: '{}' }};
                                }}
                                return {{ error: 'Text not found in element' }};
                            }}
                            const range = document.createRange();
                            const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
                            let node;
                            while ((node = walker.nextNode())) {{
                                const idx = node.textContent.indexOf('{}');
                                if (idx >= 0) {{
                                    range.setStart(node, idx);
                                    range.setEnd(node, idx + {});
                                    const sel = window.getSelection();
                                    sel.removeAllRanges();
                                    sel.addRange(range);
                                    return {{ success: true, selected: '{}' }};
                                }}
                            }}
                            return {{ error: 'Text not found' }};
                        }}"#,
                    selector,
                    text,
                    text.len(),
                    text,
                    text,
                    text.len(),
                    text
                )
            } else {
                let s = start.unwrap_or(0);
                let e = end.unwrap_or(usize::MAX);
                format!(
                    r#"() => {{
                            const el = document.querySelector('{}');
                            if (!el) return {{ error: 'Element not found' }};
                            if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') {{
                                el.focus();
                                const len = el.value.length;
                                const start = Math.min({}, len);
                                const end = Math.min({}, len);
                                el.setSelectionRange(start, end);
                                return {{ success: true, start, end, selected: el.value.substring(start, end) }};
                            }}
                            return {{ error: 'Selection by index only supported for input/textarea' }};
                        }}"#,
                    selector, s, e
                )
            };
            match page.evaluate(script.as_str()).await {
                Ok(result) => {
                    let value = result.value().cloned().unwrap_or(json!(null));
                    Ok(json!({ "success": true, "result": value }))
                }
                Err(e) => Err(format!("Failed to select: {}", e)),
            }
        }

        BrowserAction::Drag {
            selector,
            target_selector,
            delta_x,
            delta_y,
        } => {
            let script = if let Some(target) = target_selector {
                format!(
                    r#"() => {{
                            const src = document.querySelector('{}');
                            const dst = document.querySelector('{}');
                            if (!src || !dst) return {{ error: 'Element not found' }};
                            const srcRect = src.getBoundingClientRect();
                            const dstRect = dst.getBoundingClientRect();
                            const sx = srcRect.left + srcRect.width / 2;
                            const sy = srcRect.top + srcRect.height / 2;
                            const dx = dstRect.left + dstRect.width / 2;
                            const dy = dstRect.top + dstRect.height / 2;
                            ['mousedown','mousemove','mouseup'].forEach((type,i) => {{
                                const e = new MouseEvent(type, {{
                                    bubbles: true, clientX: i === 0 ? sx : dx, clientY: i === 0 ? sy : dy,
                                    buttons: 1
                                }});
                                (i === 0 ? src : document.body).dispatchEvent(e);
                            }});
                            return {{ success: true, from: {{ x: sx, y: sy }}, to: {{ x: dx, y: dy }} }};
                        }}"#,
                    selector, target
                )
            } else {
                let dx = delta_x.unwrap_or(100);
                let dy = delta_y.unwrap_or(0);
                format!(
                    r#"() => {{
                            const el = document.querySelector('{}');
                            if (!el) return {{ error: 'Element not found' }};
                            const rect = el.getBoundingClientRect();
                            const sx = rect.left + rect.width / 2;
                            const sy = rect.top + rect.height / 2;
                            const dx = sx + {};
                            const dy = sy + {};
                            ['mousedown','mousemove','mouseup'].forEach((type,i) => {{
                                const e = new MouseEvent(type, {{
                                    bubbles: true, clientX: i === 0 ? sx : dx, clientY: i === 0 ? sy : dy,
                                    buttons: 1
                                }});
                                (i === 0 ? el : document.body).dispatchEvent(e);
                            }});
                            return {{ success: true, from: {{ x: sx, y: sy }}, to: {{ x: dx, y: dy }} }};
                        }}"#,
                    selector, dx, dy
                )
            };
            match page.evaluate(script.as_str()).await {
                Ok(result) => {
                    let value = result.value().cloned().unwrap_or(json!(null));
                    Ok(json!({ "success": true, "result": value }))
                }
                Err(e) => Err(format!("Failed to drag: {}", e)),
            }
        }

        BrowserAction::UploadFiles { selector, files } => {
            use chromiumoxide::cdp::browser_protocol::dom::{
                GetDocumentParams, QuerySelectorParams, SetFileInputFilesParams,
            };
            let doc = page
                .execute(GetDocumentParams::default())
                .await
                .map_err(|e| format!("Failed to get document: {}", e))?;
            let root_id = doc.result.root.node_id;
            let query = page
                .execute(QuerySelectorParams::new(root_id, &selector))
                .await
                .map_err(|e| format!("Failed to query selector: {}", e))?;
            let node_id = query.result.node_id;
            let mut params = SetFileInputFilesParams::new(files);
            params.node_id = Some(node_id);
            match page.execute(params).await {
                Ok(_) => Ok(json!({
                    "success": true,
                    "selector": selector,
                })),
                Err(e) => Err(format!("Failed to set file input files: {}", e)),
            }
        }

        BrowserAction::HandleDialog { action, text } => {
            use chromiumoxide::cdp::browser_protocol::page::HandleJavaScriptDialogParams;
            let accept = action == "accept";
            let mut params = HandleJavaScriptDialogParams::new(accept);
            params.prompt_text = text;
            match page.execute(params).await {
                Ok(_) => Ok(json!({ "success": true, "action": action })),
                Err(e) => Err(format!("Failed to handle dialog (no dialog may be open): {}", e)),
            }
        }

        BrowserAction::SetDownloadBehavior { behavior, download_path } => {
            let browser_ref =
                browser.ok_or("Download behavior requires a browser session".to_string())?;
            use chromiumoxide::cdp::browser_protocol::browser::{
                SetDownloadBehaviorBehavior, SetDownloadBehaviorParams,
            };
            let behavior_enum = match behavior.as_str() {
                "allow" => SetDownloadBehaviorBehavior::Allow,
                "deny" => SetDownloadBehaviorBehavior::Deny,
                "allowAndName" => SetDownloadBehaviorBehavior::AllowAndName,
                _ => SetDownloadBehaviorBehavior::Default,
            };
            let mut params = SetDownloadBehaviorParams::new(behavior_enum);
            params.download_path = download_path;
            match browser_ref.execute(params).await {
                Ok(_) => Ok(json!({ "success": true, "behavior": behavior })),
                Err(e) => Err(format!("Failed to set download behavior: {}", e)),
            }
        }
        _ => Err("browser: action not handled by this group".to_string()),
    }
}
