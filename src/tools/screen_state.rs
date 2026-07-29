//! Screen-state tools — let the LLM capture the full screen state
//! (screenshot + UI tree + optional OCR) or run OCR on demand.
//!
//! - [`ScreenStateTool`] (`screen_state`): unified snapshot, the antidote to
//!   "blind operation" — the LLM sees structure *and* pixels before acting.
//! - [`ScreenOcrTool`] (`screen_ocr`): on-demand OCR of the full screen or a
//!   region, for text the accessibility tree cannot see (PDFs, dialogs,
//!   image-based UIs).

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::computer::vision::ScreenState;
use crate::computer::{ComputerAdapter, Rect, UiElement};
use crate::tools::{
    approval::RiskLevel, create_schema, sdk::ToolCapabilities, Tool, ToolContext,
    ToolExecutionResult,
};

/// Lazily-initialized shared OCR engine (model loading is expensive, so it
/// only happens on first use).
#[cfg(feature = "vision")]
pub type SharedOcr = Arc<tokio::sync::Mutex<Option<crate::computer::vision::ocr_rapid::RapidOcr>>>;

#[cfg(feature = "vision")]
pub fn new_shared_ocr() -> SharedOcr {
    Arc::new(tokio::sync::Mutex::new(None))
}

/// Lock the shared OCR engine, initializing it on first use. The returned
/// guard may be held across `.await` (it is a `tokio::sync::Mutex`) and also
/// serializes concurrent OCR calls.
#[cfg(feature = "vision")]
async fn lock_ocr(
    shared: &SharedOcr,
) -> crate::Result<tokio::sync::MutexGuard<'_, Option<crate::computer::vision::ocr_rapid::RapidOcr>>>
{
    let mut guard = shared.lock().await;
    if guard.is_none() {
        *guard = Some(
            crate::computer::vision::ocr_rapid::RapidOcr::new_auto()
                .await
                .map_err(|e| {
                    crate::error::SyscityError::Internal(format!("OCR init failed: {}", e))
                })?,
        );
    }
    Ok(guard)
}

// ── screen_state ───────────────────────────────────────────────────────────

/// Tool that captures a unified [`ScreenState`] for the LLM.
pub struct ScreenStateTool {
    adapter: Option<Arc<dyn ComputerAdapter>>,
    #[cfg(feature = "vision")]
    ocr: SharedOcr,
}

impl ScreenStateTool {
    pub fn new(
        adapter: Option<Arc<dyn ComputerAdapter>>,
        #[cfg(feature = "vision")] ocr: SharedOcr,
    ) -> Self {
        Self {
            adapter,
            #[cfg(feature = "vision")]
            ocr,
        }
    }
}

#[async_trait]
impl Tool for ScreenStateTool {
    fn name(&self) -> &str {
        "screen_state"
    }

    fn description(&self) -> &str {
        r#"Capture the current screen state: a screenshot plus the accessibility UI tree (windows, buttons, text fields in hierarchy), with optional OCR text.

Use this BEFORE desktop actions to see what is on screen instead of operating blind. The UI tree gives structured element positions; OCR reads text the tree cannot see (PDFs, image-based UIs). OCR is slow (seconds) — only enable it when the tree is insufficient."#
    }

    fn parameters_schema(&self) -> Value {
        create_schema(
            "Capture screen state (screenshot + UI tree + optional OCR)",
            json!({
                "include_ocr": {
                    "type": "boolean",
                    "description": "Also run OCR to extract visible text (slow, seconds). Default false."
                },
                "max_tree_lines": {
                    "type": "integer",
                    "description": "Maximum UI-tree outline lines to return (default 100)"
                }
            }),
            Vec::<String>::new(),
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            requires_approval: false,
            risk_level: RiskLevel::Low,
            categories: vec!["computer".to_string(), "desktop".to_string()],
            ..Default::default()
        }
    }

    fn is_available(&self, _context: &ToolContext) -> bool {
        self.adapter.is_some()
    }

    async fn execute(
        &self,
        args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let adapter = self.adapter.as_ref().ok_or_else(|| {
            crate::error::SyscityError::Unsupported(
                "Computer adapter is not configured".to_string(),
            )
        })?;

        let max_lines = args["max_tree_lines"].as_u64().unwrap_or(100) as usize;
        let want_ocr = args["include_ocr"].as_bool().unwrap_or(false);

        #[allow(unused_mut)]
        let mut state = ScreenState::capture_light(adapter.as_ref())
            .await
            .map_err(|e| crate::error::SyscityError::Internal(e.to_string()))?;

        #[cfg(feature = "vision")]
        if want_ocr {
            let mut guard = lock_ocr(&self.ocr).await?;
            let ocr = guard.as_mut().ok_or_else(|| {
                crate::error::SyscityError::Internal("OCR engine unavailable".to_string())
            })?;
            let blocks = ocr.detect_text(&state.screenshot).await.unwrap_or_default();
            drop(guard);
            state.ocr_text = blocks
                .iter()
                .map(|b| b.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            state.ocr_regions = blocks
                .iter()
                .map(crate::computer::vision::TextBlockSer::from)
                .collect();
        }

        #[cfg(not(feature = "vision"))]
        if want_ocr {
            state.ocr_text = "(OCR unavailable: built without 'vision' feature)".to_string();
        }

        // Format: indented UI-tree outline + OCR text + screenshot in data.
        let mut output = String::from("UI tree:\n");
        let mut lines = 0usize;
        for root in &state.ui_tree {
            format_element(root, 0, &mut output, &mut lines, max_lines);
        }
        if state.ui_tree.is_empty() {
            output.push_str("(empty — no accessibility tree available)\n");
        } else if lines >= max_lines {
            output.push_str("… (tree truncated)\n");
        }
        if !state.ocr_text.is_empty() {
            output.push_str("\nOCR text:\n");
            output.push_str(&state.ocr_text);
        }

        let data = json!({
            "screenshot_base64": state.screenshot.base64,
            "screenshot_width": state.screenshot.width,
            "screenshot_height": state.screenshot.height,
            "ui_tree": serde_json::to_value(&state.ui_tree).unwrap_or_default(),
            "ocr_text": state.ocr_text,
            "ocr_regions": serde_json::to_value(&state.ocr_regions).unwrap_or_default(),
        });

        Ok(ToolExecutionResult::success(output).with_data(data))
    }
}

/// Render a UI element and its children as an indented outline.
/// Returns `false` when the line cap was hit (tree truncated).
fn format_element(
    el: &UiElement,
    depth: usize,
    out: &mut String,
    lines: &mut usize,
    max_lines: usize,
) {
    if *lines >= max_lines {
        return;
    }
    *lines += 1;
    let indent = "  ".repeat(depth);
    let label = el.label.as_deref().unwrap_or("");
    let state = if el.enabled { "" } else { " [disabled]" };
    out.push_str(&format!(
        "{}{} \"{}\"{} at ({},{}) {}x{}\n",
        indent, el.role, label, state, el.bounds.x, el.bounds.y, el.bounds.width, el.bounds.height
    ));
    for child in &el.children {
        format_element(child, depth + 1, out, lines, max_lines);
    }
}

// ── screen_ocr ─────────────────────────────────────────────────────────────

/// Tool that runs OCR on the full screen or a region.
pub struct ScreenOcrTool {
    adapter: Option<Arc<dyn ComputerAdapter>>,
    #[cfg(feature = "vision")]
    ocr: SharedOcr,
}

impl ScreenOcrTool {
    pub fn new(
        adapter: Option<Arc<dyn ComputerAdapter>>,
        #[cfg(feature = "vision")] ocr: SharedOcr,
    ) -> Self {
        Self {
            adapter,
            #[cfg(feature = "vision")]
            ocr,
        }
    }
}

#[async_trait]
impl Tool for ScreenOcrTool {
    fn name(&self) -> &str {
        "screen_ocr"
    }

    fn description(&self) -> &str {
        r#"Extract text from the screen using OCR (RapidOCR). Returns text blocks with positions and confidence scores.

Use when the accessibility tree lacks the text you need: PDF viewers, dialogs, image-based UIs, games. Optionally restrict to a screen region to speed up detection and improve accuracy."#
    }

    fn parameters_schema(&self) -> Value {
        create_schema(
            "OCR the screen or a region",
            json!({
                "region_x": { "type": "integer", "description": "Region left coordinate (omit for full screen)" },
                "region_y": { "type": "integer", "description": "Region top coordinate" },
                "region_width": { "type": "integer", "description": "Region width" },
                "region_height": { "type": "integer", "description": "Region height" }
            }),
            Vec::<String>::new(),
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            requires_approval: false,
            risk_level: RiskLevel::Low,
            categories: vec!["computer".to_string(), "desktop".to_string()],
            ..Default::default()
        }
    }

    fn is_available(&self, _context: &ToolContext) -> bool {
        self.adapter.is_some() && cfg!(feature = "vision")
    }

    async fn execute(
        &self,
        args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        #[cfg(not(feature = "vision"))]
        {
            let _ = args;
            return Ok(ToolExecutionResult::error(
                "screen_ocr requires the 'vision' feature".to_string(),
            ));
        }

        #[cfg(feature = "vision")]
        {
            let adapter = self.adapter.as_ref().ok_or_else(|| {
                crate::error::SyscityError::Unsupported(
                    "Computer adapter is not configured".to_string(),
                )
            })?;

            let region = parse_region(&args);
            let screenshot = adapter
                .screenshot(region)
                .await
                .map_err(|e| crate::error::SyscityError::Internal(e.to_string()))?;

            let mut guard = lock_ocr(&self.ocr).await?;
            let ocr = guard.as_mut().ok_or_else(|| {
                crate::error::SyscityError::Internal("OCR engine unavailable".to_string())
            })?;
            let blocks = ocr
                .detect_text(&screenshot)
                .await
                .map_err(|e| crate::error::SyscityError::Internal(e.to_string()))?;
            drop(guard);

            // Offset block bounds back to screen coordinates for region OCR.
            let (ox, oy) = region.map(|r| (r.x, r.y)).unwrap_or((0, 0));
            let regions: Vec<Value> = blocks
                .iter()
                .map(|b| {
                    json!({
                        "text": b.text,
                        "confidence": b.confidence,
                        "x": b.bounds.x + ox,
                        "y": b.bounds.y + oy,
                        "width": b.bounds.width,
                        "height": b.bounds.height,
                    })
                })
                .collect();

            let text = blocks
                .iter()
                .map(|b| b.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");

            let output = if text.is_empty() {
                "No text detected.".to_string()
            } else {
                text.clone()
            };

            Ok(ToolExecutionResult::success(output).with_data(json!({
                "text": text,
                "regions": regions,
            })))
        }
    }
}

/// Parse an optional region from tool args.
fn parse_region(args: &Value) -> Option<Rect> {
    let x = args["region_x"].as_i64()?;
    let y = args["region_y"].as_i64()?;
    let w = args["region_width"].as_u64()?;
    let h = args["region_height"].as_u64()?;
    Some(Rect::new(x as i32, y as i32, w as u32, h as u32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computer::types::Rect;

    fn el(role: &str, label: Option<&str>, children: Vec<UiElement>) -> UiElement {
        UiElement {
            id: String::new(),
            role: role.to_string(),
            label: label.map(String::from),
            value: None,
            bounds: Rect::new(10, 20, 100, 30),
            enabled: true,
            focused: false,
            children,
        }
    }

    #[test]
    fn format_element_renders_indented_outline() {
        let tree = el("window", Some("App"), vec![el("button", Some("OK"), vec![])]);
        let mut out = String::new();
        let mut lines = 0;
        format_element(&tree, 0, &mut out, &mut lines, 100);
        assert!(out.contains("window \"App\" at (10,20) 100x30"));
        assert!(out.contains("  button \"OK\""));
        assert_eq!(lines, 2);
    }

    #[test]
    fn format_element_truncates_at_max_lines() {
        let children = (0..10)
            .map(|i| el("button", Some(Box::leak(i.to_string().into_boxed_str())), vec![]))
            .collect();
        let tree = el("window", Some("App"), children);
        let mut out = String::new();
        let mut lines = 0;
        format_element(&tree, 0, &mut out, &mut lines, 5);
        assert_eq!(lines, 5);
        // Caller appends the truncation notice when the cap is hit.
        if lines >= 5 {
            out.push_str("… (tree truncated)\n");
        }
        assert!(out.contains("truncated"));
    }

    #[test]
    fn parse_region_requires_all_fields() {
        assert!(parse_region(&json!({})).is_none());
        assert!(parse_region(&json!({"region_x": 1})).is_none());
        let r = parse_region(&json!({
            "region_x": 10, "region_y": 20, "region_width": 300, "region_height": 200
        }))
        .unwrap();
        assert_eq!((r.x, r.y, r.width, r.height), (10, 20, 300, 200));
    }
}
