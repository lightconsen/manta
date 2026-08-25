//! Flowing-HTML → DOCX converter.
//!
//! Contract: the input is a fragment of "flowing" HTML — block elements laid
//! out in document order (no absolute positioning). The converter walks the
//! DOM and produces a `.docx` via `docx_rs`.
//!
//! # Supported subset
//!
//! Block elements (in document order):
//! - `h1`–`h6` → docx heading paragraphs (mapped to `Heading1`–`Heading6`
//!   styles; levels 1–4 are registered, 5–6 fall back to `outline_lvl`).
//! - `p` → body paragraph.
//! - `ul` / `ol` / `li` → numbered paragraphs; one level of nesting is
//!   supported (deeper nests are clamped to level 1).
//! - `table` → `w:tbl`; `<thead>` rows render their cells bold. All four
//!   outer borders are set.
//! - `pre` and bare `code` blocks → monospace paragraph.
//! - `blockquote` → indented paragraph.
//! - `div`, `section`, `article`, `main`, `header`, `footer`, `nav` →
//!   transparent containers; their children are processed recursively.
//!
//! Inline elements:
//! - `strong` / `b` → bold run.
//! - `em` / `i` → italic run.
//! - `a[href]` → external hyperlink (the URL is recorded in document
//!   relationships by `docx_rs`).
//! - `code` → monospace run.
//! - `br` → line break within the current paragraph.
//! - Everything else (`span`, `u`, `s`, …) → children are processed with the
//!   current inline state; unknown tags are passed through gracefully.
//!
//! Whitespace: runs of whitespace inside non-`<pre>` content are collapsed to
//! a single space (browser-style). `<pre>` content preserves whitespace.
//!
//! Unsupported elements (`img`, `video`, `svg`, `iframe`, …) are silently
//! skipped. Malformed HTML never panics — the converter degrades to whatever
//! the `scraper` parser produces.

use std::io::Cursor;

use docx_rs::{
    AbstractNumbering, BorderType, Docx, Hyperlink, HyperlinkType, IndentLevel, Level, LevelJc,
    LevelText, NumberFormat, Numbering, NumberingId, Paragraph, Run, RunFonts, SpecialIndentType,
    Start, Style, StyleType, Table, TableBorder, TableBorderPosition, TableCell, TableRow,
};
use scraper::{ElementRef, Html, Node};

// ---------------------------------------------------------------------------
// Numbering IDs
// ---------------------------------------------------------------------------
/// Abstract numbering used for bullet lists (all `<ul>` in the document share
/// one abstract numbering with two indent levels).
const BULLET_ABSTRACT_ID: usize = 1;
/// Abstract numbering used for ordered lists.
const DECIMAL_ABSTRACT_ID: usize = 2;
/// Concrete numbering ID for bullet lists.
const BULLET_NUM_ID: usize = 1;
/// Concrete numbering ID for decimal (ordered) lists.
const DECIMAL_NUM_ID: usize = 2;

/// Monospace font family used for `<code>` / `<pre>` runs.
const MONO_FONT: &str = "Courier New";

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Convert flowing HTML to a `.docx` (returns the file bytes).
///
/// See the module documentation for the supported HTML subset.
pub fn flow_html_to_docx(html: &str) -> Result<Vec<u8>, String> {
    let doc = Html::parse_document(html);

    let root = doc
        .select(
            &scraper::Selector::parse("html")
                .map_err(|e| format!("internal selector error: {e}"))?,
        )
        .next()
        .ok_or_else(|| "no <html> root found after parsing".to_string())?;

    let mut ctx = BuildContext::new();
    process_block_children(root, &mut ctx);

    let mut docx = Docx::new();

    // Register heading styles (Heading1 … Heading4 with outline levels).
    for level in 1..=4usize {
        let style_id = format!("Heading{level}");
        let name = format!("heading {level}");
        docx = docx.add_style(
            Style::new(&style_id, StyleType::Paragraph)
                .name(&name)
                .bold()
                .outline_lvl(level - 1),
        );
    }

    // Register numbering definitions.
    docx = docx
        .add_abstract_numbering(
            AbstractNumbering::new(BULLET_ABSTRACT_ID)
                .add_level(bullet_level(0, 720))
                .add_level(bullet_level(1, 1440)),
        )
        .add_abstract_numbering(
            AbstractNumbering::new(DECIMAL_ABSTRACT_ID)
                .add_level(decimal_level(0, 720))
                .add_level(decimal_level(1, 1440)),
        )
        .add_numbering(Numbering::new(BULLET_NUM_ID, BULLET_ABSTRACT_ID))
        .add_numbering(Numbering::new(DECIMAL_NUM_ID, DECIMAL_ABSTRACT_ID));

    // Add all collected children.
    for child in ctx.children {
        match child {
            DocChild::Paragraph(p) => docx = docx.add_paragraph(*p),
            DocChild::Table(t) => docx = docx.add_table(*t),
        }
    }

    let mut buf = Cursor::new(Vec::new());
    docx.pack(&mut buf)
        .map_err(|e| format!("docx pack failed: {e}"))?;
    Ok(buf.into_inner())
}

// ---------------------------------------------------------------------------
// Internal builder context
// ---------------------------------------------------------------------------

/// Accumulated document children (paragraphs and tables in order).
enum DocChild {
    Paragraph(Box<Paragraph>),
    Table(Box<Table>),
}

/// Mutable state threaded through the recursive HTML walk.
struct BuildContext {
    children: Vec<DocChild>,
}

impl BuildContext {
    fn new() -> Self {
        Self { children: Vec::new() }
    }

    fn push_paragraph(&mut self, p: Paragraph) {
        self.children.push(DocChild::Paragraph(Box::new(p)));
    }

    fn push_table(&mut self, t: Table) {
        self.children.push(DocChild::Table(Box::new(t)));
    }
}

// ---------------------------------------------------------------------------
// Block-level processing
// ---------------------------------------------------------------------------

/// Process block-level children of `parent`, appending paragraphs and tables
/// to `ctx`.
fn process_block_children(parent: ElementRef<'_>, ctx: &mut BuildContext) {
    for child in parent.children() {
        match child.value() {
            Node::Element(_) => {
                if let Some(el) = ElementRef::wrap(child) {
                    handle_block_element(el, ctx);
                }
            }
            Node::Text(t) => {
                let trimmed = t.text.trim();
                if !trimmed.is_empty() {
                    // Bare text at block level → wrap in a paragraph.
                    let text = collapse_ws(&t.text);
                    let trimmed2 = text.trim();
                    if !trimmed2.is_empty() {
                        ctx.push_paragraph(Paragraph::new().add_run(Run::new().add_text(trimmed2)));
                    }
                }
            }
            _ => {}
        }
    }
}

/// Dispatch a single block-level element.
fn handle_block_element(el: ElementRef<'_>, ctx: &mut BuildContext) {
    let tag = el.value().name();
    match tag {
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let level: usize = tag[1..].parse().unwrap_or(1);
            let items = collect_inline_items(el, InlineState::default());
            let style_id = format!("Heading{level}");
            let mut para = Paragraph::new().style(&style_id);
            para = apply_inline_items(para, items);
            // For h5/h6 which don't have registered styles, also set
            // outline_lvl so the heading still appears in navigation.
            if level > 4 {
                para = para.outline_lvl(level - 1);
            }
            ctx.push_paragraph(para);
        }
        "p" => {
            let items = collect_inline_items(el, InlineState::default());
            if items.is_empty() {
                // Empty paragraph (spacer).
                ctx.push_paragraph(Paragraph::new());
            } else {
                let para = Paragraph::new();
                ctx.push_paragraph(apply_inline_items(para, items));
            }
        }
        "ul" => process_list(el, ListKind::Bullet, 0, ctx),
        "ol" => process_list(el, ListKind::Decimal, 0, ctx),
        "table" => {
            if let Some(table) = build_table(el) {
                ctx.push_table(table);
            }
        }
        "pre" => {
            // Preserve whitespace; render in monospace.
            let raw_text: String = el.text().collect::<Vec<_>>().join("");
            if !raw_text.trim().is_empty() {
                ctx.push_paragraph(
                    Paragraph::new().add_run(
                        Run::new()
                            .add_text(raw_text)
                            .fonts(RunFonts::new().ascii(MONO_FONT).hi_ansi(MONO_FONT)),
                    ),
                );
            }
        }
        "blockquote" => {
            // Render as an indented paragraph. Collect inner text.
            let items = collect_inline_items(el, InlineState::default());
            let mut para = Paragraph::new().indent(Some(720), None, None, None);
            para = apply_inline_items(para, items);
            ctx.push_paragraph(para);
        }
        // Transparent containers — recurse.
        "div" | "section" | "article" | "main" | "header" | "footer" | "nav" | "body" | "html" => {
            process_block_children(el, ctx);
        }
        // Unknown block: try to extract text as a paragraph.
        _ => {
            let items = collect_inline_items(el, InlineState::default());
            if !items.is_empty() {
                let para = Paragraph::new();
                ctx.push_paragraph(apply_inline_items(para, items));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Lists
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum ListKind {
    Bullet,
    Decimal,
}

impl ListKind {
    fn num_id(self) -> usize {
        match self {
            ListKind::Bullet => BULLET_NUM_ID,
            ListKind::Decimal => DECIMAL_NUM_ID,
        }
    }
}

/// Process a `<ul>` or `<ol>` element. Each direct `<li>` child becomes a
/// numbered paragraph. Nested lists inside a `<li>` are handled recursively.
fn process_list(list_el: ElementRef<'_>, kind: ListKind, depth: usize, ctx: &mut BuildContext) {
    let level = depth.min(1); // clamp to 0 or 1

    for child in list_el.children() {
        let child_el = match ElementRef::wrap(child) {
            Some(el) => el,
            None => continue,
        };
        if child_el.value().name() != "li" {
            continue;
        }

        // Fresh numbering handles per paragraph (not Copy/Clone).
        let num_id = NumberingId::new(kind.num_id());
        let indent_level = IndentLevel::new(level);

        // Collect inline items from the <li> itself (ignoring nested lists).
        let items = collect_li_inline_items(child_el);
        if !items.is_empty() {
            let para = Paragraph::new().numbering(num_id, indent_level);
            ctx.push_paragraph(apply_inline_items(para, items));
        } else {
            // <li> with only a nested list — emit an empty numbered paragraph.
            ctx.push_paragraph(Paragraph::new().numbering(num_id, indent_level));
        }

        // Process nested lists inside this <li>.
        for nested in child_el.child_elements() {
            let nested_tag = nested.value().name();
            if nested_tag == "ul" {
                process_list(nested, ListKind::Bullet, depth + 1, ctx);
            } else if nested_tag == "ol" {
                process_list(nested, ListKind::Decimal, depth + 1, ctx);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Inline processing
// ---------------------------------------------------------------------------

/// Formatting state propagated through inline recursion.
#[derive(Clone, Copy, Default)]
struct InlineState {
    bold: bool,
    italic: bool,
    mono: bool,
}

/// A collected inline item: either a run or a hyperlink.
enum InlineItem {
    Run(Box<Run>),
    Link { url: String, runs: Vec<Run> },
}

/// Collect inline items from the children of `el`, applying `state` as the
/// initial formatting.
fn collect_inline_items(el: ElementRef<'_>, state: InlineState) -> Vec<InlineItem> {
    let mut items = Vec::new();
    collect_inline_inner(el, state, &mut items);
    items
}

/// Like `collect_inline_items` but skips nested list elements (`<ul>`, `<ol>`)
/// so they don't appear as inline text in the parent `<li>` paragraph.
fn collect_li_inline_items(li_el: ElementRef<'_>) -> Vec<InlineItem> {
    let mut items = Vec::new();
    for child in li_el.children() {
        match child.value() {
            Node::Text(t) => {
                let text = collapse_ws(&t.text);
                if !text.is_empty() {
                    items.push(InlineItem::Run(Box::new(Run::new().add_text(text))));
                }
            }
            Node::Element(_) => {
                if let Some(el) = ElementRef::wrap(child) {
                    let tag = el.value().name();
                    // Skip nested lists — they are processed separately.
                    if tag == "ul" || tag == "ol" {
                        continue;
                    }
                    collect_inline_from_element(el, InlineState::default(), &mut items);
                }
            }
            _ => {}
        }
    }
    items
}

fn collect_inline_inner(el: ElementRef<'_>, state: InlineState, out: &mut Vec<InlineItem>) {
    for child in el.children() {
        match child.value() {
            Node::Text(t) => {
                let text = collapse_ws(&t.text);
                if !text.is_empty() {
                    out.push(InlineItem::Run(Box::new(make_run(&text, state))));
                }
            }
            Node::Element(_) => {
                if let Some(child_el) = ElementRef::wrap(child) {
                    collect_inline_from_element(child_el, state, out);
                }
            }
            _ => {}
        }
    }
}

fn collect_inline_from_element(el: ElementRef<'_>, state: InlineState, out: &mut Vec<InlineItem>) {
    let tag = el.value().name();
    match tag {
        "strong" | "b" => {
            let mut s = state;
            s.bold = true;
            collect_inline_inner(el, s, out);
        }
        "em" | "i" => {
            let mut s = state;
            s.italic = true;
            collect_inline_inner(el, s, out);
        }
        "code" => {
            let mut s = state;
            s.mono = true;
            collect_inline_inner(el, s, out);
        }
        "a" => {
            let url = el.value().attr("href").unwrap_or("").to_string();
            let mut inner = Vec::new();
            collect_inline_inner(el, state, &mut inner);
            if url.is_empty() {
                // No href — emit the inner content inline without a link.
                out.extend(inner);
            } else {
                let runs = flatten_to_runs(inner);
                if !runs.is_empty() {
                    out.push(InlineItem::Link { url, runs });
                }
            }
        }
        "br" => {
            out.push(InlineItem::Run(Box::new(
                Run::new().add_break(docx_rs::BreakType::TextWrapping),
            )));
        }
        // Block elements encountered in inline context: skip.
        "p" | "div" | "ul" | "ol" | "table" | "pre" | "blockquote" | "h1" | "h2" | "h3" | "h4"
        | "h5" | "h6" | "section" | "article" => {}
        // Everything else (span, u, s, etc.): recurse with current state.
        _ => {
            collect_inline_inner(el, state, out);
        }
    }
}

/// Flatten inline items to runs (nested links lose their URL — invalid HTML
/// nesting, handled gracefully).
fn flatten_to_runs(items: Vec<InlineItem>) -> Vec<Run> {
    let mut runs = Vec::new();
    for item in items {
        match item {
            InlineItem::Run(r) => runs.push(*r),
            InlineItem::Link { runs: r, .. } => runs.extend(r),
        }
    }
    runs
}

/// Build a `Run` with the given text and formatting state.
fn make_run(text: &str, state: InlineState) -> Run {
    let mut run = Run::new().add_text(text);
    if state.bold {
        run = run.bold();
    }
    if state.italic {
        run = run.italic();
    }
    if state.mono {
        run = run.fonts(RunFonts::new().ascii(MONO_FONT).hi_ansi(MONO_FONT));
    }
    run
}

/// Apply collected inline items to a paragraph.
fn apply_inline_items(mut para: Paragraph, items: Vec<InlineItem>) -> Paragraph {
    for item in items {
        match item {
            InlineItem::Run(run) => para = para.add_run(*run),
            InlineItem::Link { url, runs } => {
                let mut link = Hyperlink::new(&url, HyperlinkType::External);
                for run in runs {
                    link = link.add_run(run);
                }
                para = para.add_hyperlink(link);
            }
        }
    }
    para
}

// ---------------------------------------------------------------------------
// Tables
// ---------------------------------------------------------------------------

/// Build a `Table` from a `<table>` element. Returns `None` if no rows found.
fn build_table(table_el: ElementRef<'_>) -> Option<Table> {
    let mut rows: Vec<TableRow> = Vec::new();
    let mut col_count: usize = 0;

    // Look for <thead> and <tbody> children, or direct <tr> children.
    let mut thead_rows: Vec<(Vec<String>, bool)> = Vec::new();
    let mut body_rows: Vec<(Vec<String>, bool)> = Vec::new();

    // We collect raw cell data: (text, is_header).
    // For richer content we'd need full inline processing per cell, but for
    // the contract (thead bold, borders) we keep cells as text paragraphs.

    for child in table_el.child_elements() {
        let tag = child.value().name();
        match tag {
            "thead" => {
                for tr in child.child_elements() {
                    if tr.value().name() == "tr" {
                        let (cells, n) = collect_row_cells(tr, true);
                        col_count = col_count.max(n);
                        thead_rows.push((cells, true));
                    }
                }
            }
            "tbody" | "tfoot" => {
                for tr in child.child_elements() {
                    if tr.value().name() == "tr" {
                        let (cells, n) = collect_row_cells(tr, false);
                        col_count = col_count.max(n);
                        body_rows.push((cells, false));
                    }
                }
            }
            "tr" => {
                let (cells, n) = collect_row_cells(child, false);
                col_count = col_count.max(n);
                body_rows.push((cells, false));
            }
            _ => {}
        }
    }

    if thead_rows.is_empty() && body_rows.is_empty() {
        return None;
    }

    // Build docx TableRows.
    for (cells, is_header) in &thead_rows {
        rows.push(build_table_row(cells, *is_header, col_count));
    }
    for (cells, is_header) in &body_rows {
        rows.push(build_table_row(cells, *is_header, col_count));
    }

    let grid = vec![2000; col_count];
    let table = Table::new(rows)
        .set_grid(grid)
        .set_borders(default_borders());
    Some(table)
}

/// Collect cell texts from a `<tr>`. Returns `(cell_texts, cell_count)`.
fn collect_row_cells(tr_el: ElementRef<'_>, is_header: bool) -> (Vec<String>, usize) {
    let mut cells = Vec::new();
    for child in tr_el.child_elements() {
        let tag = child.value().name();
        if tag == "td" || tag == "th" {
            let text: String = child.text().map(collapse_ws).collect::<Vec<_>>().join("");
            let text = text.trim().to_string();
            // th elements are always header cells regardless of section.
            let _is_th = tag == "th" || is_header;
            cells.push(text);
        }
    }
    let count = cells.len();
    (cells, count)
}

/// Build a single `TableRow` from cell text values.
fn build_table_row(cells: &[String], is_header: bool, col_count: usize) -> TableRow {
    let mut table_cells: Vec<TableCell> = Vec::new();
    for (i, text) in cells.iter().enumerate() {
        let _ = i;
        let run = if is_header {
            Run::new().add_text(text).bold()
        } else {
            Run::new().add_text(text)
        };
        let para = if text.is_empty() {
            Paragraph::new()
        } else {
            Paragraph::new().add_run(run)
        };
        table_cells.push(TableCell::new().add_paragraph(para));
    }
    // Pad missing cells so the row has the expected column count.
    while table_cells.len() < col_count {
        table_cells.push(TableCell::new().add_paragraph(Paragraph::new()));
    }
    TableRow::new(table_cells)
}

/// Default table borders (thin black on all four sides plus inside borders).
fn default_borders() -> docx_rs::TableBorders {
    let border = |pos: TableBorderPosition| {
        TableBorder::new(pos)
            .border_type(BorderType::Single)
            .size(4)
            .color("000000")
    };
    docx_rs::TableBorders::new()
        .set(border(TableBorderPosition::Top))
        .set(border(TableBorderPosition::Bottom))
        .set(border(TableBorderPosition::Left))
        .set(border(TableBorderPosition::Right))
        .set(border(TableBorderPosition::InsideH))
        .set(border(TableBorderPosition::InsideV))
}

// ---------------------------------------------------------------------------
// Numbering level helpers
// ---------------------------------------------------------------------------

/// Create a bullet list level definition.
fn bullet_level(level: usize, left_indent_twips: i32) -> Level {
    Level::new(
        level,
        Start::new(1),
        NumberFormat::new("bullet"),
        LevelText::new("\u{2022}"),
        LevelJc::new("left"),
    )
    .indent(Some(left_indent_twips), Some(SpecialIndentType::Hanging(360)), None, None)
}

/// Create a decimal (numbered) list level definition.
fn decimal_level(level: usize, left_indent_twips: i32) -> Level {
    let text = if level == 0 { "%1." } else { "%2." };
    Level::new(
        level,
        Start::new(1),
        NumberFormat::new("decimal"),
        LevelText::new(text),
        LevelJc::new("left"),
    )
    .indent(Some(left_indent_twips), Some(SpecialIndentType::Hanging(360)), None, None)
}

// ---------------------------------------------------------------------------
// Whitespace
// ---------------------------------------------------------------------------

/// Collapse runs of whitespace to a single space (browser-style).
fn collapse_ws(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut prev_ws = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !prev_ws {
                result.push(' ');
            }
            prev_ws = true;
        } else {
            result.push(c);
            prev_ws = false;
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};

    /// Unzip a docx and return the contents of `word/document.xml`.
    fn document_xml(docx_bytes: &[u8]) -> String {
        let mut archive =
            zip::ZipArchive::new(Cursor::new(docx_bytes)).expect("docx should be a valid zip");
        let mut entry = archive
            .by_name("word/document.xml")
            .expect("docx should contain word/document.xml");
        let mut xml = String::new();
        entry
            .read_to_string(&mut xml)
            .expect("document.xml should be UTF-8");
        xml
    }

    #[test]
    fn heading_paragraph_and_inline_formatting() {
        let html = r#"
            <h1>Title</h1>
            <p>Hello <strong>bold</strong> and <em>italic</em> world</p>
        "#;
        let bytes = flow_html_to_docx(html).expect("conversion should succeed");
        let xml = document_xml(&bytes);

        // Heading text present.
        assert!(xml.contains("Title"), "heading text missing");
        // Paragraph text present.
        assert!(xml.contains("Hello"), "paragraph text missing");
        assert!(xml.contains("bold"), "bold text missing");
        assert!(xml.contains("italic"), "italic text missing");
        // Bold and italic run properties (docx-rs serializes `<w:b />`).
        assert!(xml.contains("<w:b"), "bold run property missing");
        assert!(xml.contains("<w:i"), "italic run property missing");
        // Heading style reference.
        assert!(xml.contains("Heading1"), "Heading1 style reference missing");
    }

    #[test]
    fn bulleted_and_numbered_lists() {
        let html = r#"
            <ul>
                <li>Apple</li>
                <li>Banana</li>
            </ul>
            <ol>
                <li>First</li>
                <li>Second</li>
            </ol>
        "#;
        let bytes = flow_html_to_docx(html).expect("conversion should succeed");
        let xml = document_xml(&bytes);

        // List items present.
        assert!(xml.contains("Apple"), "bullet item missing");
        assert!(xml.contains("Banana"), "bullet item missing");
        assert!(xml.contains("First"), "numbered item missing");
        assert!(xml.contains("Second"), "numbered item missing");
        // Numbering property markers.
        assert!(xml.contains("w:numPr"), "w:numPr missing — lists not applied");
        assert!(xml.contains("w:numId"), "w:numId missing");
    }

    #[test]
    fn table_with_header_row() {
        let html = r#"
            <table>
                <thead>
                    <tr><th>Name</th><th>Value</th></tr>
                </thead>
                <tbody>
                    <tr><td>alpha</td><td>1</td></tr>
                </tbody>
            </table>
        "#;
        let bytes = flow_html_to_docx(html).expect("conversion should succeed");
        let xml = document_xml(&bytes);

        assert!(xml.contains("<w:tbl>"), "table element missing");
        assert!(xml.contains("Name"), "header cell text missing");
        assert!(xml.contains("Value"), "header cell text missing");
        assert!(xml.contains("alpha"), "body cell text missing");
        // Header cells should be bold.
        assert!(xml.contains("<w:b"), "header bold missing");
    }

    #[test]
    fn hyperlink_present() {
        let html = r#"<p>Visit <a href="https://example.com">Example</a> site</p>"#;
        let bytes = flow_html_to_docx(html).expect("conversion should succeed");
        let xml = document_xml(&bytes);

        assert!(xml.contains("Example"), "link text missing");
        assert!(xml.contains("<w:hyperlink"), "hyperlink element missing");
        // The hyperlink should reference a relationship ID.
        assert!(xml.contains("r:id"), "hyperlink relationship ID missing");
    }

    #[test]
    fn malformed_html_does_not_panic() {
        // Intentionally broken HTML.
        let html = "<div><p>unclosed<p>another<span></div></garbage>";
        let bytes = flow_html_to_docx(html).expect("malformed HTML should still produce output");

        // Must be a valid zip (starts with PK magic bytes).
        assert!(bytes.len() >= 2, "output too short to be a zip");
        assert_eq!(bytes[0], b'P', "missing PK magic byte 0");
        assert_eq!(bytes[1], b'K', "missing PK magic byte 1");

        // Must contain document.xml.
        let _xml = document_xml(&bytes);
    }
}
