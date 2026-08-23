//! Network log capture, emulation and capture-clearing actions.

use super::{BrowserAction, BrowserScreenshot};
use serde_json::{json, Value};

use tracing::warn;

pub(super) async fn execute_network_actions(
    action: BrowserAction,
    page: &chromiumoxide::Page,
    _browser: Option<&chromiumoxide::Browser>,
    _screenshot_data: &mut Option<BrowserScreenshot>,
) -> Result<serde_json::Value, String> {
    match action {
        BrowserAction::GetNetworkLog {
            url,
            method,
            resource_type,
            min_status,
            max_status,
            include_body,
            limit,
            offset,
        } => {
            // Prefer the CDP capture (all resource types); fall back to
            // the injected fetch/XHR shim when CDP capture is inactive.
            if crate::browser::network_log::start_capture(page)
                .await
                .is_ok()
            {
                return crate::browser::network_log::query(
                    page,
                    crate::browser::network_log::NetworkQuery {
                        url: url.as_deref(),
                        method: method.as_deref(),
                        min_status,
                        max_status,
                        resource_type: resource_type.as_deref(),
                    },
                    include_body.unwrap_or(true),
                    limit.unwrap_or(50),
                    offset.unwrap_or(0),
                )
                .await;
            }

            crate::browser::instrument::ensure_instrumented(page).await?;
            let script = r#"() => (window.__syscity_net || [])"#;
            match page.evaluate(script).await {
                Ok(result) => {
                    let entries = result.into_value::<Vec<Value>>().unwrap_or_default();
                    let include_body = include_body.unwrap_or(true);
                    let filtered: Vec<Value> = entries
                        .into_iter()
                        .filter(|e| {
                            url.as_ref().is_none_or(|f| {
                                e.get("url")
                                    .and_then(|u| u.as_str())
                                    .is_some_and(|u| u.contains(f.as_str()))
                            })
                        })
                        .filter(|e| {
                            method.as_ref().is_none_or(|m| {
                                e.get("method")
                                    .and_then(|v| v.as_str())
                                    .is_some_and(|v| v.eq_ignore_ascii_case(m))
                            })
                        })
                        .filter(|e| {
                            let status = e.get("status").and_then(|s| s.as_u64());
                            min_status.is_none_or(|min| status.is_some_and(|s| s >= min as u64))
                                && max_status
                                    .is_none_or(|max| status.is_some_and(|s| s <= max as u64))
                        })
                        .map(|mut e| {
                            if !include_body {
                                if let Value::Object(ref mut obj) = e {
                                    obj.remove("body");
                                    obj.remove("body_truncated");
                                }
                            }
                            e
                        })
                        .collect();
                    let total = filtered.len();
                    let offset = offset.unwrap_or(0);
                    let page_entries: Vec<Value> = filtered
                        .into_iter()
                        .skip(offset)
                        .take(limit.unwrap_or(50))
                        .collect();
                    Ok(json!({
                        "success": true,
                        "entries": page_entries,
                        "count": page_entries.len(),
                        "total": total,
                        "offset": offset,
                        "source": "shim",
                        "note": "Captures fetch/XHR only (not document/image/css loads). Bodies truncated to 8KB."
                    }))
                }
                Err(e) => Err(format!("Failed to get network log: {}", e)),
            }
        }

        BrowserAction::EmulateNetwork {
            latency_ms,
            download_bps,
            upload_bps,
            offline,
        } => {
            // Deprecated in favor of the Emulation-domain variant in newer
            // CDP revisions, but still fully supported by Chrome.
            #[allow(deprecated)]
            use chromiumoxide::cdp::browser_protocol::network::EmulateNetworkConditionsParams;
            #[allow(deprecated)]
            let params = EmulateNetworkConditionsParams::new(
                offline.unwrap_or(false),
                latency_ms.unwrap_or(0.0),
                download_bps.unwrap_or(-1.0),
                upload_bps.unwrap_or(-1.0),
            );
            match page.execute(params).await {
                Ok(_) => Ok(json!({
                    "success": true,
                    "latency_ms": latency_ms.unwrap_or(0.0),
                    "offline": offline.unwrap_or(false)
                })),
                Err(e) => Err(format!("Failed to emulate network conditions: {}", e)),
            }
        }

        BrowserAction::EmulateCpu { rate } => {
            use chromiumoxide::cdp::browser_protocol::emulation::SetCpuThrottlingRateParams;
            match page
                .execute(SetCpuThrottlingRateParams::new(rate.max(1.0)))
                .await
            {
                Ok(_) => Ok(json!({ "success": true, "rate": rate })),
                Err(e) => Err(format!("Failed to set CPU throttling: {}", e)),
            }
        }

        BrowserAction::ClearCaptures => {
            crate::browser::network_log::clear(page).await;
            let script = r#"() => {
                    if (window.__syscity_net) window.__syscity_net.length = 0;
                    if (window.__syscity_console) window.__syscity_console.length = 0;
                }"#;
            match page.evaluate(script).await {
                Ok(_) => Ok(json!({ "success": true })),
                Err(e) => Err(format!("Failed to clear captures: {}", e)),
            }
        }

        BrowserAction::EmulateMobile { device_name } => {
            let (width, height, dpr, mobile, ua) = match device_name.to_lowercase().as_str() {
                "iphone_x" | "iphonex" => (
                    375,
                    812,
                    3.0,
                    true,
                    "Mozilla/5.0 (iPhone; CPU iPhone OS 16_0 like Mac OS X) \
                         AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.0 Mobile/15E148 \
                         Safari/604.1",
                ),
                "iphone_12" | "iphone12" => (
                    390,
                    844,
                    3.0,
                    true,
                    "Mozilla/5.0 (iPhone; CPU iPhone OS 16_0 like Mac OS X) \
                         AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.0 Mobile/15E148 \
                         Safari/604.1",
                ),
                "pixel_5" | "pixel5" => (
                    393,
                    851,
                    2.75,
                    true,
                    "Mozilla/5.0 (Linux; Android 13; Pixel 5) AppleWebKit/537.36 (KHTML, like \
                         Gecko) Chrome/112.0.0.0 Mobile Safari/537.36",
                ),
                "ipad" => (
                    810,
                    1080,
                    2.0,
                    true,
                    "Mozilla/5.0 (iPad; CPU OS 16_0 like Mac OS X) AppleWebKit/605.1.15 \
                         (KHTML, like Gecko) Version/16.0 Mobile/15E148 Safari/604.1",
                ),
                _ => (
                    375,
                    667,
                    2.0,
                    true,
                    "Mozilla/5.0 (iPhone; CPU iPhone OS 16_0 like Mac OS X) \
                         AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.0 Mobile/15E148 \
                         Safari/604.1",
                ),
            };

            use chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams;
            let params = SetDeviceMetricsOverrideParams::builder()
                .width(width as i64)
                .height(height as i64)
                .device_scale_factor(dpr)
                .mobile(mobile)
                .build()
                .map_err(|e| format!("Failed to build viewport params: {}", e))?;

            match page.execute(params).await {
                Ok(_) => {
                    let ua_script = format!(
                        r#"() => {{ Object.defineProperty(navigator, 'userAgent', {{ value: '{}', configurable: true }}); return true; }}"#,
                        ua
                    );
                    if let Err(e) = page.evaluate(ua_script.as_str()).await {
                        warn!("Failed to set custom user agent: {}", e);
                    }
                    Ok(json!({
                        "success": true,
                        "device": device_name,
                        "viewport": { "width": width, "height": height, "device_scale_factor": dpr, "mobile": mobile }
                    }))
                }
                Err(e) => Err(format!("Failed to emulate mobile device: {}", e)),
            }
        }

        BrowserAction::SetViewport {
            width,
            height,
            device_scale_factor,
            mobile,
        } => {
            use chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams;
            let mut builder = SetDeviceMetricsOverrideParams::builder()
                .width(width as i64)
                .height(height as i64);
            if let Some(dpr) = device_scale_factor {
                builder = builder.device_scale_factor(dpr);
            }
            if let Some(mob) = mobile {
                builder = builder.mobile(mob);
            }
            let params = builder
                .build()
                .map_err(|e| format!("Failed to build viewport params: {}", e))?;

            match page.execute(params).await {
                Ok(_) => Ok(json!({
                    "success": true,
                    "viewport": { "width": width, "height": height }
                })),
                Err(e) => Err(format!("Failed to set viewport: {}", e)),
            }
        }
        _ => Err("browser: action not handled by this group".to_string()),
    }
}
