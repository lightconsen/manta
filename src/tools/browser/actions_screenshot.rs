//! Screenshot, PDF and screencast actions.

use super::{BrowserAction, BrowserScreenshot};
use serde_json::json;
use tracing::warn;

use base64::Engine;
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
use chromiumoxide::page::ScreenshotParamsBuilder;

use super::screencast::{screencast_start, screencast_stop};

pub(super) async fn execute_screenshot_actions(
    action: BrowserAction,
    page: &chromiumoxide::Page,
    _browser: Option<&chromiumoxide::Browser>,
    screenshot_data: &mut Option<BrowserScreenshot>,
) -> Result<serde_json::Value, String> {
    match action {
        BrowserAction::Screenshot { full_page, selector } => {
            let result = match selector {
                Some(sel) => match page.find_element(&sel).await {
                    Ok(elem) => elem.screenshot(CaptureScreenshotFormat::Png).await,
                    Err(e) => Err(e),
                },
                None => {
                    let params = if full_page.unwrap_or(false) {
                        ScreenshotParamsBuilder::default()
                            .format(CaptureScreenshotFormat::Png)
                            .full_page(true)
                            .build()
                    } else {
                        ScreenshotParamsBuilder::default()
                            .format(CaptureScreenshotFormat::Png)
                            .build()
                    };
                    page.screenshot(params).await
                }
            };

            match result {
                Ok(data) => {
                    // CAS-first: store the PNG bytes once and hand back a
                    // compact reference; fall back to inline base64 when the
                    // store is unavailable (fail-open — see attachments docs).
                    // The clone keeps the raw bytes available for the fallback.
                    match crate::attachments::store_bytes_async(data.clone(), "image/png").await {
                        Ok(aref) => {
                            let note = format!(
                                "Screenshot captured ({} bytes, image/png), stored as {}",
                                aref.size,
                                crate::attachments::short_id(&aref.digest)
                            );
                            *screenshot_data = Some(BrowserScreenshot::Ref(aref.clone()));
                            Ok(json!({
                                "success": true,
                                "format": "png",
                                "size": aref.size,
                                "image_ref": aref.to_json(),
                                "note": note,
                            }))
                        }
                        Err(e) => {
                            warn!(
                                "attachment store write failed ({}); falling back to inline base64",
                                e
                            );
                            let base64 = base64::engine::general_purpose::STANDARD.encode(&data);
                            *screenshot_data = Some(BrowserScreenshot::Inline(base64.clone()));
                            Ok(json!({
                                "success": true,
                                "format": "png",
                                "base64_length": base64.len(),
                                "data": format!("data:image/png;base64,{}", base64)
                            }))
                        }
                    }
                }
                Err(e) => Err(format!("Failed to take screenshot: {}", e)),
            }
        }

        BrowserAction::PrintToPdf {
            landscape,
            display_header_footer,
            print_background,
            scale,
            paper_width,
            paper_height,
            margin_top,
            margin_bottom,
            margin_left,
            margin_right,
            page_ranges,
        } => {
            use chromiumoxide::cdp::browser_protocol::page::PrintToPdfParams;

            let mut params = PrintToPdfParams::default();
            if let Some(v) = landscape {
                params.landscape = Some(v);
            }
            if let Some(v) = display_header_footer {
                params.display_header_footer = Some(v);
            }
            if let Some(v) = print_background {
                params.print_background = Some(v);
            }
            if let Some(v) = scale {
                params.scale = Some(v);
            }
            if let Some(v) = paper_width {
                params.paper_width = Some(v);
            }
            if let Some(v) = paper_height {
                params.paper_height = Some(v);
            }
            if let Some(v) = margin_top {
                params.margin_top = Some(v);
            }
            if let Some(v) = margin_bottom {
                params.margin_bottom = Some(v);
            }
            if let Some(v) = margin_left {
                params.margin_left = Some(v);
            }
            if let Some(v) = margin_right {
                params.margin_right = Some(v);
            }
            if let Some(ref v) = page_ranges {
                params.page_ranges = Some(v.clone());
            }

            match page.pdf(params).await {
                Ok(data) => {
                    let base64 = base64::engine::general_purpose::STANDARD.encode(&data);
                    Ok(json!({
                        "success": true,
                        "format": "pdf",
                        "base64_length": base64.len(),
                        "data": format!("data:application/pdf;base64,{}", base64)
                    }))
                }
                Err(e) => Err(format!("Failed to print PDF: {}", e)),
            }
        }

        BrowserAction::ScreencastStart { quality, every_nth_frame } => {
            screencast_start(page, quality, every_nth_frame).await
        }

        BrowserAction::ScreencastStop => screencast_stop(page).await,
        _ => Err("browser: action not handled by this group".to_string()),
    }
}
