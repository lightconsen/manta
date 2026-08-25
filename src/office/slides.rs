//! Canvas-HTML → PPTX converter.
//!
//! Contract: the document is a sequence of `<div class="slide">` elements,
//! each a 1280×720 px canvas (16:9). Children carry inline styles with
//! absolute positioning (`left/top/width/height` in px) plus a small
//! supported subset: `background`/`background-color`, `color`, `font-weight`,
//! `text-align`. 1 px = 9525 EMU, so the HTML preview is pixel-identical to
//! the generated deck.
//!
//! v1 limitation: ppt-rs renders shape text with auto-fit sizing, so
//! `font-size` is intentionally not honored yet — the box geometry is exact
//! and the text scales to fit. A later backend (ooxmlsdk or a hand-rolled
//! writer) can honor font-size precisely without changing the contract.

use scraper::{ElementRef, Html, Selector};

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
    pub kind: ElementKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ElementKind {
    /// Plain text content (line breaks preserved).
    Text(String),
    /// Bullet list items.
    Bullets(Vec<String>),
    /// Image source path/URL from the `src` attribute.
    Image(String),
}

/// A parsed slide canvas.
#[derive(Debug, Clone, PartialEq)]
pub struct SlideSpec {
    /// Slide background color as `RRGGBB`, if specified.
    pub background: Option<String>,
    pub elements: Vec<CanvasElement>,
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

fn element_text(el: &ElementRef) -> String {
    // Join text nodes; <br>/<p>/<li> already separated by the caller for
    // lists — for plain text collapse runs of whitespace.
    let raw: String = el.text().collect::<Vec<_>>().join("");
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse_element(el: &ElementRef) -> Option<CanvasElement> {
    let style = parse_inline_style(el.value().attr("style").unwrap_or(""));
    if style_get(&style, "position") != Some("absolute") {
        return None;
    }
    let x_px = style_get(&style, "left").and_then(px).unwrap_or(0.0);
    let y_px = style_get(&style, "top").and_then(px).unwrap_or(0.0);
    // Default box: span to the canvas right edge with a one-line height.
    let w_px = style_get(&style, "width")
        .and_then(px)
        .unwrap_or(CANVAS_W_PX - x_px);
    let h_px = style_get(&style, "height").and_then(px).unwrap_or(80.0);
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
        let text = element_text(el);
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
        let background = style_get(&style, "background-color")
            .or_else(|| style_get(&style, "background"))
            .and_then(normalize_color);

        // Direct children only — nested content belongs to its parent box.
        let elements = slide_el
            .child_elements()
            .filter_map(|child| parse_element(&child))
            .collect();

        slides.push(SlideSpec { background, elements });
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
    use ppt_rs::generator::shapes::{Shape, ShapeFill, ShapeType};
    use ppt_rs::generator::{create_pptx_with_content, SlideContent};
    use ppt_rs::SlideLayout;

    let specs = parse_canvas(html)?;
    let mut slides = Vec::with_capacity(specs.len());

    for spec in specs {
        let mut slide = SlideContent::new("").with_layout(SlideLayout::Blank);

        // Slide background → full-canvas rect drawn first (under everything).
        if let Some(bg) = &spec.background {
            slide = slide.add_shape(
                Shape::new(ShapeType::Rectangle, 0, 0, SLIDE_W_EMU, SLIDE_H_EMU)
                    .with_fill(ShapeFill::new(bg)),
            );
        }

        for el in &spec.elements {
            match &el.kind {
                ElementKind::Text(text) => {
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
                    if !text.is_empty() {
                        shape = shape.with_text(text);
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
    patch_slide_size_16x9(&pptx)
}

/// Rewrite `<p:sldSz>` in `ppt/presentation.xml` from the generator's 4:3
/// default to the 1280×720-equivalent 16:9 size.
fn patch_slide_size_16x9(pptx: &[u8]) -> Result<Vec<u8>, String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r##"
        <div class="slide" style="background:#ffffff">
          <div style="position:absolute; left:80px; top:60px; width:1120px; height:100px; color:#191a23; font-weight:700">季度回顾</div>
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
        assert_eq!(els[0].kind, ElementKind::Text("季度回顾".to_string()));
        assert!((els[0].x_px - 80.0).abs() < 1e-9 && (els[0].w_px - 1120.0).abs() < 1e-9);
        assert!(els[0].bold);
        assert_eq!(els[0].color.as_deref(), Some("191a23"));
        match &els[1].kind {
            ElementKind::Bullets(items) => {
                assert_eq!(items, &vec!["收入增长 25%".to_string(), "用户量翻倍".to_string()])
            }
            other => panic!("expected bullets, got {other:?}"),
        }
        // Background-only box (no text) is kept as a colored rect.
        assert_eq!(els[2].kind, ElementKind::Text(String::new()));
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
    }
}
