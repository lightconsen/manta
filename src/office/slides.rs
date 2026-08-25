//! Canvas-HTML → PPTX converter.
//!
//! Contract: the document is a sequence of `<div class="slide">` elements,
//! each a 1280×720 px canvas (16:9). Children carry inline styles with
//! absolute positioning (`left`/`top` or `right`/`bottom` plus `width`/
//! `height` in px) plus a small supported subset: `background`/`background-
//! color` (solid or `linear-gradient(...)`), `color`, `font-weight`,
//! `font-size`. 1 px = 9525 EMU, so the HTML preview is pixel-identical to
//! the generated deck.
//!
//! ppt-rs renders shape text with auto-fit sizing and has no API to carry an
//! explicit font size into a positioned shape, so after generation the
//! package is patched: for every element that declares `font-size`, the
//! runs inside its shape get `sz="{px * 0.75 * 100}"` (px → pt → hundredths
//! of a point) and `<a:normAutofit/>` becomes `<a:noAutofit/>` so Office
//! does not shrink the text back. Elements without an explicit `font-size`
//! keep the auto-fit behavior.

use scraper::{ElementRef, Html, Node, Selector};

/// EMU per CSS pixel at the 1280×720 canvas (1280 px = 12192000 EMU).
pub const EMU_PER_PX: f64 = 9525.0;
/// Canvas dimensions in px (16:9).
pub const CANVAS_W_PX: f64 = 1280.0;
pub const CANVAS_H_PX: f64 = 720.0;
/// 16:9 slide size in EMU (written into the package by the size patch).
const SLIDE_W_EMU: u32 = 12192000;
const SLIDE_H_EMU: u32 = 6858000;

fn emu(px_value: f64) -> u32 {
    (px_value * EMU_PER_PX).round().clamp(0.0, u32::MAX as f64) as u32
}

/// One absolutely-positioned element on a slide canvas.
#[derive(Debug, Clone, PartialEq)]
pub struct CanvasElement {
    pub x_px: f64,
    pub y_px: f64,
    pub w_px: f64,
    pub h_px: f64,
    /// Text/foreground color as `RRGGBB` (no `#`), if specified.
    pub color: Option<String>,
    /// Background color as `RRGGBB`, if specified.
    pub background: Option<String>,
    pub bold: bool,
    /// `font-size` in px, if specified. Converted to `sz` hundredths of a
    /// point (px × 0.75 × 100) in the generated deck.
    pub font_size_px: Option<f64>,
    pub kind: ElementKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ElementKind {
    /// Rich text content — lines of formatted segments (nested blocks and
    /// `<br>` become line breaks; inline tags carry their formatting).
    Text(TextBlock),
    /// Bullet list items.
    Bullets(Vec<String>),
    /// Image source path/URL from the `src` attribute.
    Image(String),
}

/// A text box's content: lines, each a list of formatted runs.
pub type TextBlock = Vec<Vec<TextSegment>>;

/// A single formatted run of text within a text box.
#[derive(Debug, Clone, PartialEq)]
pub struct TextSegment {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub mono: bool,
    /// Foreground color as `RRGGBB`, if specified.
    pub color: Option<String>,
    /// Font size in px, if specified (px × 75 → `sz` hundredths of a point).
    pub font_size_px: Option<f64>,
}

/// A parsed slide canvas.
#[derive(Debug, Clone, PartialEq)]
pub struct SlideSpec {
    /// Slide background color as `RRGGBB`, if specified.
    pub background: Option<String>,
    /// Slide background gradient (`linear-gradient(...)`), if specified.
    pub background_gradient: Option<GradientSpec>,
    pub elements: Vec<CanvasElement>,
}

/// A parsed linear gradient: direction angle plus color stops.
#[derive(Debug, Clone, PartialEq)]
pub struct GradientSpec {
    /// Direction angle in degrees (0 = to top, 90 = to right, 135 = diagonal).
    pub angle_deg: f32,
    /// `(RRGGBB color, position 0.0–100.0)` stops in order.
    pub stops: Vec<(String, f32)>,
}

/// Parse an inline `style` attribute into lowercase-key property pairs.
pub fn parse_inline_style(style: &str) -> Vec<(String, String)> {
    style
        .split(';')
        .filter_map(|decl| {
            let (k, v) = decl.split_once(':')?;
            let k = k.trim().to_ascii_lowercase();
            let v = v.trim().to_string();
            if k.is_empty() || v.is_empty() {
                None
            } else {
                Some((k, v))
            }
        })
        .collect()
}

fn style_get<'a>(style: &'a [(String, String)], key: &str) -> Option<&'a str> {
    style
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// Extract a px length from a CSS value ("80px" → 80.0, "80" → 80.0).
fn px(value: &str) -> Option<f64> {
    value.trim().trim_end_matches("px").trim().parse().ok()
}

/// Normalize a CSS color to `RRGGBB`. Supports `#rgb`, `#rrggbb`, and the
/// small named-color set the canvas contract allows.
pub fn normalize_color(value: &str) -> Option<String> {
    let v = value.trim().to_ascii_lowercase();
    let named = match v.as_str() {
        "black" => Some("000000"),
        "white" => Some("ffffff"),
        "red" => Some("ff0000"),
        "green" => Some("008000"),
        "blue" => Some("0000ff"),
        "gray" | "grey" => Some("808080"),
        "transparent" => None,
        _ => None,
    };
    if let Some(n) = named {
        return Some(n.to_string());
    }
    let hex = v.strip_prefix('#')?;
    let expanded = if hex.len() == 3 {
        hex.chars().flat_map(|c| [c, c]).collect::<String>()
    } else {
        hex.to_string()
    };
    if expanded.len() == 6 && expanded.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(expanded)
    } else {
        None
    }
}

/// Parse a CSS `linear-gradient(...)` value into a [`GradientSpec`].
///
/// Supports `linear-gradient(<angle>deg, <color> [<pos>%], …)` and the
/// `to <side>` direction keywords. Missing stop positions are distributed
/// evenly across 0–100.
pub fn parse_gradient(value: &str) -> Option<GradientSpec> {
    let inner = value
        .trim()
        .strip_prefix("linear-gradient(")?
        .strip_suffix(")")?;
    let mut parts = inner.split(',').map(str::trim);
    let first = parts.next()?.to_ascii_lowercase();
    let angle_deg = if let Some(d) = first.strip_suffix("deg") {
        d.trim().parse::<f32>().ok()?
    } else {
        match first.as_str() {
            "to right" => 90.0,
            "to left" => 270.0,
            "to bottom" => 180.0,
            "to top" => 0.0,
            "to bottom right" => 135.0,
            "to bottom left" => 225.0,
            _ => return None,
        }
    };

    let mut stops: Vec<(String, Option<f32>)> = Vec::new();
    for part in parts {
        let mut toks = part.split_whitespace();
        let color = normalize_color(toks.next()?)?;
        let pos = toks
            .next()
            .and_then(|p| p.trim_end_matches('%').parse::<f32>().ok());
        stops.push((color, pos));
    }
    if stops.is_empty() {
        return None;
    }
    let n = stops.len();
    let stops = stops
        .into_iter()
        .enumerate()
        .map(|(i, (color, pos))| {
            let pos = pos.unwrap_or(if n == 1 {
                100.0
            } else {
                i as f32 * 100.0 / (n - 1) as f32
            });
            (color, pos)
        })
        .collect();
    Some(GradientSpec { angle_deg, stops })
}

fn element_text(el: &ElementRef) -> String {
    // Join text nodes; <br>/<p>/<li> already separated by the caller for
    // lists — for plain text collapse runs of whitespace.
    let raw: String = el.text().collect::<Vec<_>>().join("");
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Collapse runs of whitespace to a single space (browser-style).
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Inline formatting state threaded through the rich-text walk.
#[derive(Clone, Default)]
struct InlineCtx {
    bold: bool,
    italic: bool,
    mono: bool,
    color: Option<String>,
    font_size_px: Option<f64>,
}

impl InlineCtx {
    fn merge_style(self, style: &[(String, String)]) -> Self {
        let bold = style_get(style, "font-weight")
            .map(|v| v == "bold" || v.parse::<u32>().map(|n| n >= 600).unwrap_or(false))
            .unwrap_or(self.bold);
        let color = style_get(style, "color")
            .and_then(normalize_color)
            .or(self.color);
        let font_size_px = style_get(style, "font-size")
            .and_then(px)
            .or(self.font_size_px);
        InlineCtx {
            bold,
            color,
            font_size_px,
            ..self
        }
    }
}

/// Walk an element's children into lines of formatted segments. `<br>` and
/// block-level descendants start new lines; inline tags (`b`, `strong`, `em`,
/// `i`, `code`) and nested inline-styled `div`/`span` adjust formatting.
fn collect_text_lines(el: &ElementRef, ctx: &InlineCtx) -> TextBlock {
    let mut lines: TextBlock = vec![Vec::new()];
    collect_text_inner(el, ctx, &mut lines);
    // Drop trailing empty lines and empty leading segments.
    while lines.len() > 1 && lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    lines
        .into_iter()
        .filter(|line| line.iter().any(|s| !s.text.is_empty()))
        .collect()
}

fn collect_text_inner(el: &ElementRef, ctx: &InlineCtx, lines: &mut TextBlock) {
    for child in el.children() {
        match child.value() {
            Node::Text(t) => {
                let text = collapse_ws(&t.text);
                if !text.is_empty() {
                    let seg = TextSegment {
                        text,
                        bold: ctx.bold,
                        italic: ctx.italic,
                        mono: ctx.mono,
                        color: ctx.color.clone(),
                        font_size_px: ctx.font_size_px,
                    };
                    // `lines` always holds at least one line; the fallback
                    // keeps this clippy-clean without an unwrap.
                    if let Some(last) = lines.last_mut() {
                        last.push(seg);
                    } else {
                        lines.push(vec![seg]);
                    }
                }
            }
            Node::Element(_) => {
                if let Some(child_el) = ElementRef::wrap(child) {
                    let tag = child_el.value().name();
                    let style = parse_inline_style(child_el.value().attr("style").unwrap_or(""));
                    match tag {
                        "br" => lines.push(Vec::new()),
                        "b" | "strong" => {
                            let mut c = ctx.clone();
                            c.bold = true;
                            collect_text_inner(&child_el, &c, lines);
                        }
                        "i" | "em" => {
                            let mut c = ctx.clone();
                            c.italic = true;
                            collect_text_inner(&child_el, &c, lines);
                        }
                        "code" => {
                            let mut c = ctx.clone();
                            c.mono = true;
                            collect_text_inner(&child_el, &c, lines);
                        }
                        // Block-level descendants → line break around content.
                        "div" | "p" | "li" | "blockquote" | "h1" | "h2" | "h3" | "h4" | "h5"
                        | "h6" => {
                            let c = ctx.clone().merge_style(&style);
                            if !lines.last().map(|l| l.is_empty()).unwrap_or(true) {
                                lines.push(Vec::new());
                            }
                            collect_text_inner(&child_el, &c, lines);
                            lines.push(Vec::new());
                        }
                        // Inline container (span/a/u/s) — inherit + own style.
                        _ => {
                            let c = ctx.clone().merge_style(&style);
                            collect_text_inner(&child_el, &c, lines);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn parse_element(el: &ElementRef) -> Option<CanvasElement> {
    let style = parse_inline_style(el.value().attr("style").unwrap_or(""));
    if style_get(&style, "position") != Some("absolute") {
        return None;
    }
    let left = style_get(&style, "left").and_then(px);
    let right = style_get(&style, "right").and_then(px);
    let top = style_get(&style, "top").and_then(px);
    let bottom = style_get(&style, "bottom").and_then(px);
    let width = style_get(&style, "width").and_then(px);
    let height = style_get(&style, "height").and_then(px);

    let font_size_px = style_get(&style, "font-size").and_then(px);

    // Width/height resolve first (they participate in right/bottom anchoring);
    // unanchored text defaults to a font-size-derived box.
    let w_px = match width {
        Some(w) => w,
        None => match (left, right) {
            (Some(l), Some(r)) => CANVAS_W_PX - l - r,
            (Some(l), None) => CANVAS_W_PX - l,
            (None, Some(_)) => font_size_px.map(|s| s * 2.5).unwrap_or(400.0),
            (None, None) => CANVAS_W_PX,
        },
    };
    let h_px = match height {
        Some(h) => h,
        None => match (top, bottom) {
            (Some(t), Some(b)) => CANVAS_H_PX - t - b,
            _ => font_size_px.map(|s| s * 1.4).unwrap_or(80.0),
        },
    };
    let x_px = match left {
        Some(l) => l,
        None => match right {
            Some(r) => CANVAS_W_PX - r - w_px,
            None => 0.0,
        },
    };
    let y_px = match top {
        Some(t) => t,
        None => match bottom {
            Some(b) => CANVAS_H_PX - b - h_px,
            None => 0.0,
        },
    };
    if w_px <= 0.0 || h_px <= 0.0 {
        return None;
    }

    let background = style_get(&style, "background-color")
        .or_else(|| style_get(&style, "background"))
        .and_then(normalize_color);
    let color = style_get(&style, "color").and_then(normalize_color);
    let bold = style_get(&style, "font-weight")
        .map(|v| v == "bold" || v.parse::<u32>().map(|n| n >= 600).unwrap_or(false))
        .unwrap_or(false);

    let tag = el.value().name();
    let kind = if tag == "img" {
        ElementKind::Image(el.value().attr("src").unwrap_or("").to_string())
    } else if tag == "ul" || tag == "ol" {
        let li_sel = Selector::parse("li").ok()?;
        let items = el
            .select(&li_sel)
            .map(|li| element_text(&li))
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>();
        if items.is_empty() {
            return None;
        }
        ElementKind::Bullets(items)
    } else {
        // The element's own style seeds the inline context; nested blocks
        // override per-run.
        let ctx = InlineCtx {
            bold,
            color: color.clone(),
            font_size_px,
            ..Default::default()
        };
        let text = collect_text_lines(el, &ctx);
        if text.is_empty() && background.is_none() {
            return None;
        }
        ElementKind::Text(text)
    };

    Some(CanvasElement {
        x_px,
        y_px,
        w_px,
        h_px,
        color,
        background,
        bold,
        font_size_px,
        kind,
    })
}

/// Parse a canvas-HTML document into slide specs. Returns an error when the
/// document contains no `<div class="slide">` elements.
pub fn parse_canvas(html: &str) -> Result<Vec<SlideSpec>, String> {
    let doc = Html::parse_document(html);
    let slide_sel =
        Selector::parse("div.slide").map_err(|e| format!("internal selector error: {e}"))?;

    let mut slides = Vec::new();
    for slide_el in doc.select(&slide_sel) {
        let style = parse_inline_style(slide_el.value().attr("style").unwrap_or(""));
        let bg_value =
            style_get(&style, "background-color").or_else(|| style_get(&style, "background"));
        let background = bg_value.and_then(normalize_color);
        let background_gradient = bg_value.and_then(parse_gradient);

        // Direct children only — nested content belongs to its parent box.
        let elements = slide_el
            .child_elements()
            .filter_map(|child| parse_element(&child))
            .collect();

        slides.push(SlideSpec {
            background,
            background_gradient,
            elements,
        });
    }

    if slides.is_empty() {
        return Err(
            "no <div class=\"slide\"> found — canvas documents must wrap each slide in one"
                .to_string(),
        );
    }
    Ok(slides)
}

/// Resolve an image `src` to (bytes, format) for embedding. Implementations
/// typically read workspace-relative paths inside the write fence.
/// Returns `None` to skip the image.
pub trait ImageResolver {
    fn resolve(&self, src: &str) -> Option<(Vec<u8>, String)>;
}

/// Build the deck with ppt-rs, then patch the slide size to 16:9 (ppt-rs's
/// high-level generator hardcodes 4:3 — the canvas is 1280×720).
pub fn canvas_html_to_pptx(
    html: &str,
    title: &str,
    resolver: &dyn ImageResolver,
) -> Result<Vec<u8>, String> {
    use ppt_rs::generator::images::Image;
    use ppt_rs::generator::shapes::{
        GradientDirection, GradientFill, GradientStop, Shape, ShapeFill, ShapeType,
    };
    use ppt_rs::generator::{create_pptx_with_content, SlideContent};
    use ppt_rs::SlideLayout;

    let specs = parse_canvas(html)?;
    let mut slides = Vec::with_capacity(specs.len());

    for spec in &specs {
        let mut slide = SlideContent::new("").with_layout(SlideLayout::Blank);

        // Slide background → full-canvas rect drawn first (under everything).
        if let Some(bg) = &spec.background {
            slide = slide.add_shape(
                Shape::new(ShapeType::Rectangle, 0, 0, SLIDE_W_EMU, SLIDE_H_EMU)
                    .with_fill(ShapeFill::new(bg)),
            );
        } else if let Some(grad) = &spec.background_gradient {
            let stops = grad
                .stops
                .iter()
                .map(|(color, pos)| GradientStop {
                    color: color.clone(),
                    position: (pos * 1000.0).round() as u32,
                    transparency: None,
                })
                .collect();
            slide = slide.add_shape(
                Shape::new(ShapeType::Rectangle, 0, 0, SLIDE_W_EMU, SLIDE_H_EMU).with_gradient(
                    GradientFill {
                        stops,
                        direction: GradientDirection::Angle(grad.angle_deg.round() as u32),
                    },
                ),
            );
        }

        for el in &spec.elements {
            match &el.kind {
                ElementKind::Text(block) => {
                    let mut shape = Shape::new(
                        ShapeType::Rectangle,
                        emu(el.x_px),
                        emu(el.y_px),
                        emu(el.w_px),
                        emu(el.h_px),
                    );
                    if let Some(bg) = &el.background {
                        shape = shape.with_fill(ShapeFill::new(bg));
                    }
                    // Placeholder flat text (space-joined) so ppt-rs emits a
                    // text-bearing shape; the post-patch rewrites its txBody
                    // with the structured lines/segments.
                    let flat: String = block
                        .iter()
                        .flat_map(|line| line.iter().map(|s| s.text.as_str()))
                        .collect::<Vec<_>>()
                        .join(" ");
                    if !flat.is_empty() {
                        shape = shape.with_text(&flat);
                    }
                    slide = slide.add_shape(shape);
                }
                ElementKind::Bullets(items) => {
                    // v1: bullets ride in a positioned text box.
                    let text = items
                        .iter()
                        .map(|i| format!("• {i}"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    let mut shape = Shape::new(
                        ShapeType::Rectangle,
                        emu(el.x_px),
                        emu(el.y_px),
                        emu(el.w_px),
                        emu(el.h_px),
                    );
                    if let Some(bg) = &el.background {
                        shape = shape.with_fill(ShapeFill::new(bg));
                    }
                    slide = slide.add_shape(shape.with_text(&text));
                }
                ElementKind::Image(src) => {
                    if let Some((bytes, format)) = resolver.resolve(src) {
                        let img = Image::from_bytes(bytes, emu(el.w_px), emu(el.h_px), &format)
                            .position(emu(el.x_px), emu(el.y_px));
                        slide = slide.add_image(img);
                    }
                }
            }
        }
        slides.push(slide);
    }

    let pptx = create_pptx_with_content(title, slides)
        .map_err(|e| format!("pptx generation failed: {e}"))?;
    patch_pptx(&pptx, &specs)
}

/// Post-generation package patch, applied by rewriting the zip in one pass:
/// 1. `ppt/presentation.xml`: slide size 4:3 → the 1280×720 16:9 size.
/// 2. `ppt/slides/slideN.xml`: inject explicit font sizes for elements that
///    declare `font-size` (ppt-rs only emits auto-fit text).
fn patch_pptx(pptx: &[u8], specs: &[SlideSpec]) -> Result<Vec<u8>, String> {
    use std::io::{Cursor, Read, Write};

    let mut archive = zip::ZipArchive::new(Cursor::new(pptx))
        .map_err(|e| format!("pptx is not a readable zip: {e}"))?;
    let mut out = Vec::with_capacity(pptx.len());
    let mut writer = zip::ZipWriter::new(Cursor::new(&mut out));

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("zip entry {i}: {e}"))?;
        let name = entry.name().to_string();
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut buf)
            .map_err(|e| format!("read zip entry {name}: {e}"))?;

        if name == "ppt/presentation.xml" {
            let xml = String::from_utf8_lossy(&buf).into_owned();
            let patched = xml.replace(
                "<p:sldSz cx=\"9144000\" cy=\"6858000\" type=\"screen4x3\"/>",
                &format!("<p:sldSz cx=\"{SLIDE_W_EMU}\" cy=\"{SLIDE_H_EMU}\"/>"),
            );
            if patched == xml {
                return Err("slide size marker not found in presentation.xml".to_string());
            }
            buf = patched.into_bytes();
        } else if let Some(spec) = slide_spec_for_entry(&name, specs) {
            let xml = String::from_utf8_lossy(&buf).into_owned();
            buf = patch_slide_text(&xml, spec).into_bytes();
        }

        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .map_err(|e| format!("zip write entry: {e}"))?;
        writer
            .write_all(&buf)
            .map_err(|e| format!("zip write bytes: {e}"))?;
    }
    // finish() finalizes the archive and hands back the cursor, releasing
    // the mutable borrow of `out` when it drops.
    writer.finish().map_err(|e| format!("zip finalize: {e}"))?;
    Ok(out)
}

/// Map `ppt/slides/slideN.xml` to its spec (N is 1-based).
fn slide_spec_for_entry<'a>(name: &str, specs: &'a [SlideSpec]) -> Option<&'a SlideSpec> {
    let rest = name.strip_prefix("ppt/slides/slide")?;
    let n: usize = rest.strip_suffix(".xml")?.parse().ok()?;
    specs.get(n.checked_sub(1)?)
}

/// Rewrite a slide's text-bearing shape `<p:sp>` blocks with rich text:
/// each line becomes a `<a:p>` paragraph, each segment a `<a:r>` run carrying
/// its own bold/italic/color/font-size.
///
/// Shapes appear in insertion order, mirroring element order in the spec
/// (background rect first, then elements; images are `p:pic`, not `p:sp`).
/// Only text-bearing `<p:sp>` blocks consume a queue entry, so textless
/// background boxes and placeholder shapes cannot shift the mapping.
fn patch_slide_text(xml: &str, spec: &SlideSpec) -> String {
    let mut queue: Vec<Option<TextBlock>> = spec
        .elements
        .iter()
        .map(|el| match &el.kind {
            ElementKind::Text(block) if !block.is_empty() => Some(block.clone()),
            ElementKind::Bullets(items) => {
                // Each bullet becomes a line; the element's style carries over.
                let lines = items
                    .iter()
                    .map(|item| {
                        vec![TextSegment {
                            text: format!("• {item}"),
                            bold: el.bold,
                            italic: false,
                            mono: false,
                            color: el.color.clone(),
                            font_size_px: el.font_size_px,
                        }]
                    })
                    .collect();
                Some(lines)
            }
            _ => None,
        })
        .collect();

    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    loop {
        let Some(start_rel) = rest.find("<p:sp>") else {
            out.push_str(rest);
            break;
        };
        let start = start_rel;
        let Some(end_rel) = rest[start..].find("</p:sp>") else {
            out.push_str(rest);
            break;
        };
        let end = start + end_rel + "</p:sp>".len();
        let block = &rest[start..end];

        let patched = if block.contains("<a:t>") {
            if let Some(Some(text_block)) = queue.first().cloned() {
                let tx = text_block_to_txbody(&text_block);
                queue.remove(0);
                replace_txbody(block, &tx)
            } else {
                block.to_string()
            }
        } else {
            block.to_string()
        };

        out.push_str(&rest[..start]);
        out.push_str(&patched);
        rest = &rest[end..];
    }
    out
}

/// Replace the first `<p:txBody>…</p:txBody>` in a shape block.
fn replace_txbody(block: &str, txbody: &str) -> String {
    let Some(start) = block.find("<p:txBody>") else {
        return block.to_string();
    };
    let Some(end_rel) = block[start..].find("</p:txBody>") else {
        return block.to_string();
    };
    let end = start + end_rel + "</p:txBody>".len();
    format!("{}{}{}", &block[..start], txbody, &block[end..])
}

/// Generate a `<p:txBody>` with one `<a:p>` per line and one `<a:r>` per
/// segment. `noAutofit` when any segment declares a font size, otherwise
/// `normAutofit` (auto-fit) as the fallback.
fn text_block_to_txbody(block: &TextBlock) -> String {
    let any_sized = block.iter().flatten().any(|s| s.font_size_px.is_some());
    let autofit = if any_sized {
        "noAutofit"
    } else {
        "normAutofit"
    };

    let mut paras = String::new();
    for line in block {
        let mut runs = String::new();
        for seg in line {
            let sz = seg
                .font_size_px
                .map(|s| (s * 75.0).round().max(100.0) as u32)
                .unwrap_or(1800);
            let mut attrs = format!("lang=\"en-US\" sz=\"{sz}\"");
            if seg.bold {
                attrs.push_str(" b=\"1\"");
            }
            if seg.italic {
                attrs.push_str(" i=\"1\"");
            }
            let mut children = String::new();
            if seg.mono {
                children.push_str(
                    "<a:latin typeface=\"Courier New\"/><a:ea typeface=\"Courier New\"/>\
                     <a:cs typeface=\"Courier New\"/>",
                );
            }
            if let Some(c) = &seg.color {
                children
                    .push_str(&format!("<a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill>", c));
            }
            runs.push_str(&format!(
                "<a:r><a:rPr {attrs}>{children}</a:rPr><a:t>{}</a:t></a:r>",
                escape_xml_text(&seg.text)
            ));
        }
        paras.push_str(&format!("<a:p><a:pPr algn=\"l\"/>{runs}</a:p>"));
    }

    format!(
        "<p:txBody><a:bodyPr wrap=\"square\" rtlCol=\"0\" anchor=\"t\" lIns=\"91440\" \
         tIns=\"45720\" rIns=\"91440\" bIns=\"45720\"><a:{autofit}/></a:bodyPr>\
         <a:lstStyle/>{paras}</p:txBody>"
    )
}

/// Escape XML special characters in run text.
fn escape_xml_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r##"
        <div class="slide" style="background:#ffffff">
          <div style="position:absolute; left:80px; top:60px; width:1120px; height:100px; color:#191a23; font-weight:700; font-size:56px">季度回顾</div>
          <ul style="position:absolute; left:80px; top:220px; width:500px; height:300px">
            <li>收入增长 25%</li>
            <li>用户量翻倍</li>
          </ul>
          <div style="position:absolute; left:640px; top:200px; width:560px; height:400px; background:#f8f9fa"></div>
        </div>
        <div class="slide">
          <div style="position:absolute; left:80px; top:60px; width:1120px">第二页</div>
        </div>
    "##;

    #[test]
    fn parse_canvas_extracts_slides_and_elements() {
        let slides = parse_canvas(SAMPLE).unwrap();
        assert_eq!(slides.len(), 2);
        assert_eq!(slides[0].background.as_deref(), Some("ffffff"));
        assert_eq!(slides[1].background, None);

        let els = &slides[0].elements;
        assert_eq!(els.len(), 3);
        assert_eq!(
            els[0].kind,
            ElementKind::Text(vec![vec![TextSegment {
                text: "季度回顾".to_string(),
                bold: true,
                italic: false,
                mono: false,
                color: Some("191a23".to_string()),
                font_size_px: Some(56.0),
            }]])
        );
        assert!((els[0].x_px - 80.0).abs() < 1e-9 && (els[0].w_px - 1120.0).abs() < 1e-9);
        assert!(els[0].bold);
        assert_eq!(els[0].color.as_deref(), Some("191a23"));
        assert_eq!(els[0].font_size_px, Some(56.0));
        assert_eq!(els[1].font_size_px, None);
        match &els[1].kind {
            ElementKind::Bullets(items) => {
                assert_eq!(items, &vec!["收入增长 25%".to_string(), "用户量翻倍".to_string()])
            }
            other => panic!("expected bullets, got {other:?}"),
        }
        // Background-only box (no text) is kept as a colored rect.
        assert_eq!(els[2].kind, ElementKind::Text(vec![]));
        assert_eq!(els[2].background.as_deref(), Some("f8f9fa"));
    }

    #[test]
    fn parse_canvas_requires_slide_divs() {
        assert!(parse_canvas("<p>hello</p>").is_err());
    }

    #[test]
    fn parse_canvas_skips_non_absolute_children() {
        let html = r#"<div class="slide"><div style="position:relative">flow text</div></div>"#;
        let slides = parse_canvas(html).unwrap();
        assert!(slides[0].elements.is_empty());
    }

    #[test]
    fn normalize_color_variants() {
        assert_eq!(normalize_color("#fff").as_deref(), Some("ffffff"));
        assert_eq!(normalize_color("#B22AC2").as_deref(), Some("b22ac2"));
        assert_eq!(normalize_color("white").as_deref(), Some("ffffff"));
        assert_eq!(normalize_color("transparent"), None);
        assert_eq!(normalize_color("rgb(1,2,3)"), None);
    }

    struct NoImages;
    impl ImageResolver for NoImages {
        fn resolve(&self, _src: &str) -> Option<(Vec<u8>, String)> {
            None
        }
    }

    #[test]
    fn canvas_to_pptx_produces_valid_16x9_package() {
        let bytes = canvas_html_to_pptx(SAMPLE, "测试", &NoImages).unwrap();
        let mut archive =
            zip::ZipArchive::new(std::io::Cursor::new(&bytes)).expect("pptx should be a zip");
        let names: Vec<String> = (0..archive.len())
            .filter_map(|i| archive.by_index(i).ok().map(|e| e.name().to_string()))
            .collect();
        assert!(names.iter().any(|n| n == "ppt/slides/slide1.xml"));
        assert!(names.iter().any(|n| n == "ppt/slides/slide2.xml"));

        // The 16:9 patch must have landed.
        let mut pres = String::new();
        use std::io::Read;
        archive
            .by_name("ppt/presentation.xml")
            .unwrap()
            .read_to_string(&mut pres)
            .unwrap();
        assert!(pres.contains("cx=\"12192000\""), "slide size not patched");
        assert!(!pres.contains("screen4x3"));

        // Slide 1 carries the title text and the bullet box.
        let mut slide1 = String::new();
        archive
            .by_name("ppt/slides/slide1.xml")
            .unwrap()
            .read_to_string(&mut slide1)
            .unwrap();
        assert!(slide1.contains("季度回顾"));
        assert!(slide1.contains("收入增长 25%"));

        // The title declares font-size:56px → sz=4200 (56 × 0.75 × 100) and
        // auto-fit disabled. The bullets declare no size → auto-fit kept.
        assert!(slide1.contains("sz=\"4200\""), "explicit font size missing");
        assert!(slide1.contains("<a:noAutofit/>"), "noAutofit not injected for sized element");
        assert!(
            slide1.contains("<a:normAutofit/>"),
            "auto-fit should be kept for unsized elements"
        );
    }

    #[test]
    fn font_sizes_map_to_their_own_shapes() {
        // Three sized text boxes must each get their own sz value, in
        // element order — a mapping bug would swap them.
        let html = r#"
            <div class="slide">
              <div style="position:absolute;left:0px;top:0px;width:600px;height:80px;font-size:64px">甲</div>
              <div style="position:absolute;left:0px;top:100px;width:600px;height:60px;font-size:24px">乙</div>
              <div style="position:absolute;left:0px;top:180px;width:600px;height:40px;font-size:12.5px">丙</div>
            </div>
        "#;
        let bytes = canvas_html_to_pptx(html, "t", &NoImages).unwrap();
        let mut archive =
            zip::ZipArchive::new(std::io::Cursor::new(&bytes)).expect("pptx should be a zip");
        let mut slide1 = String::new();
        use std::io::Read;
        archive
            .by_name("ppt/slides/slide1.xml")
            .unwrap()
            .read_to_string(&mut slide1)
            .unwrap();

        // 64px → 4800, 24px → 1800, 12.5px → 937.5 → rounds to 938 (min 100).
        assert!(slide1.contains("甲"));
        let (i_jia, i_yi, i_bing) = (
            slide1.find("甲").unwrap(),
            slide1.find("乙").unwrap(),
            slide1.find("丙").unwrap(),
        );
        let sz_4800 = slide1.find("sz=\"4800\"").expect("64px size missing");
        let sz_1800 = slide1.find("sz=\"1800\"").expect("24px size missing");
        let sz_938 = slide1.find("sz=\"938\"").expect("12.5px size missing");
        assert!(
            sz_4800 < i_jia && sz_1800 < i_yi && sz_938 < i_bing,
            "sizes must land inside their own shapes: {sz_4800} {sz_1800} {sz_938} vs {i_jia} {i_yi} {i_bing}"
        );
        assert_eq!(slide1.matches("<a:noAutofit/>").count(), 3);
    }

    #[test]
    fn parse_gradient_linear_with_stops() {
        let g = parse_gradient("linear-gradient(135deg,#1a1a2e 0%,#16213e 50%,#0f3460 100%)")
            .expect("valid gradient");
        assert!((g.angle_deg - 135.0).abs() < 1e-6);
        assert_eq!(
            g.stops,
            vec![
                ("1a1a2e".to_string(), 0.0),
                ("16213e".to_string(), 50.0),
                ("0f3460".to_string(), 100.0),
            ]
        );
        // Direction keyword + missing positions are distributed evenly.
        let g2 = parse_gradient("linear-gradient(to right, #000, #fff)").unwrap();
        assert!((g2.angle_deg - 90.0).abs() < 1e-6);
        assert_eq!(g2.stops, vec![("000000".to_string(), 0.0), ("ffffff".to_string(), 100.0)]);
        // Non-gradient returns None.
        assert!(parse_gradient("#ffffff").is_none());
    }

    #[test]
    fn right_bottom_anchoring_positions_box() {
        // A watermark anchored with right/bottom (no left/top/width/height)
        // must land against the bottom-right corner.
        let html = r#"<div class="slide"><div style="position:absolute;right:80px;bottom:120px;font-size:160px">AI</div></div>"#;
        let spec = parse_canvas(html).unwrap();
        let el = &spec[0].elements[0];
        let w = 160.0 * 2.5;
        let h = 160.0 * 1.4;
        assert!((el.x_px - (CANVAS_W_PX - 80.0 - w)).abs() < 0.01, "x={}", el.x_px);
        assert!((el.y_px - (CANVAS_H_PX - 120.0 - h)).abs() < 0.01, "y={}", el.y_px);
        assert!((el.w_px - w).abs() < 0.01);
        assert!((el.h_px - h).abs() < 0.01);
    }

    #[test]
    fn gradient_background_renders_into_pptx() {
        let html = r#"<div class="slide" style="background:linear-gradient(135deg,#1a1a2e,#0f3460)"><div style="position:absolute;left:0px;top:0px;width:100px;height:50px;font-size:20px">x</div></div>"#;
        let bytes = canvas_html_to_pptx(html, "t", &NoImages).unwrap();
        let mut archive =
            zip::ZipArchive::new(std::io::Cursor::new(&bytes)).expect("pptx should be a zip");
        let mut slide1 = String::new();
        use std::io::Read;
        archive
            .by_name("ppt/slides/slide1.xml")
            .unwrap()
            .read_to_string(&mut slide1)
            .unwrap();
        // ppt-rs emits a gradient fill element for the background rect.
        assert!(slide1.contains("<a:gradFill"), "gradient fill missing from slide XML");
        assert!(slide1.contains("srgbClr val=\"1a1a2e\""));
        assert!(slide1.contains("srgbClr val=\"0f3460\""));
    }

    #[test]
    fn nested_content_preserves_lines_and_formatting() {
        // A card with <b>+<br/> bullets and a nested two-div block (title +
        // body at different sizes) must come out as separate lines/segments.
        let html = r#"
            <div class="slide">
              <div style="position:absolute;left:60px;top:260px;width:560px;height:200px;font-size:22px;color:#1f3a5f;">
                <b>核心特征</b><br/>
                • 参数量巨大<br/>
                • 自监督预训练
              </div>
              <div style="position:absolute;left:60px;top:150px;width:360px;height:200px;">
                <div style="font-size:30px;font-weight:800;color:#e65100;">① 预训练</div>
                <div style="font-size:20px;color:#5d4037;">在海量文本上学习</div>
              </div>
            </div>
        "#;
        let specs = parse_canvas(html).unwrap();
        let els = &specs[0].elements;

        // Card 1: bold first line, then two bullet lines.
        match &els[0].kind {
            ElementKind::Text(block) => {
                assert_eq!(block.len(), 3, "expected 3 lines, got {:?}", block);
                assert!(block[0][0].bold, "first line should be bold");
                assert_eq!(block[0][0].text, "核心特征");
                assert_eq!(block[1][0].text, "• 参数量巨大");
                assert_eq!(block[2][0].text, "• 自监督预训练");
            }
            other => panic!("expected text, got {other:?}"),
        }

        // Card 2: nested divs → two lines with distinct sizes/colors.
        match &els[1].kind {
            ElementKind::Text(block) => {
                assert_eq!(block.len(), 2, "expected 2 lines, got {:?}", block);
                assert_eq!(block[0][0].text, "① 预训练");
                assert_eq!(block[0][0].font_size_px, Some(30.0));
                assert_eq!(block[0][0].color.as_deref(), Some("e65100"));
                assert_eq!(block[1][0].text, "在海量文本上学习");
                assert_eq!(block[1][0].font_size_px, Some(20.0));
                assert_eq!(block[1][0].color.as_deref(), Some("5d4037"));
            }
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn nested_content_renders_multiple_paragraphs() {
        let html = r#"
            <div class="slide">
              <div style="position:absolute;left:60px;top:260px;width:560px;height:200px;font-size:22px;">
                <b>核心特征</b><br/>• 参数量巨大<br/>• 自监督预训练
              </div>
            </div>
        "#;
        let bytes = canvas_html_to_pptx(html, "t", &NoImages).unwrap();
        let mut archive =
            zip::ZipArchive::new(std::io::Cursor::new(&bytes)).expect("pptx should be a zip");
        let mut slide1 = String::new();
        use std::io::Read;
        archive
            .by_name("ppt/slides/slide1.xml")
            .unwrap()
            .read_to_string(&mut slide1)
            .unwrap();
        // Three lines → three <a:p> paragraphs, bold on the first run.
        assert_eq!(slide1.matches("<a:p>").count(), 3, "expected 3 paragraphs");
        assert!(slide1.contains("b=\"1\""), "bold run missing");
        assert!(slide1.contains("核心特征"));
        assert!(slide1.contains("• 参数量巨大"));
        assert!(slide1.contains("• 自监督预训练"));
    }
}
