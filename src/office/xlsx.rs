//! Table-HTML → XLSX converter.
//!
//! Contract: the document contains one or more `<table>` elements, each
//! representing a worksheet. The converter parses the HTML, infers cell value
//! types, and writes an `.xlsx` file with one sheet per table.
//!
//! # Supported HTML subset
//!
//! - `<table>` — one per sheet.
//! - `<caption>` — used as sheet name fallback (after `data-sheet` attribute).
//! - `<thead>`, `<tbody>` — row grouping; `<thead>` rows are bolded.
//! - `<tr>` — rows.
//! - `<th>`, `<td>` — cells. `colspan` duplicates the value into spanned
//!   columns. `rowspan` is **not** supported in v1 (cells below a rowspan are
//!   not shifted — they simply fill left-to-right, top-to-bottom).
//! - `data-sheet` attribute on `<table>` — preferred sheet name.
//! - `data-type="percent"` on `<td>`/`<th>` — parse `"12.5%"` as `0.125`.
//!
//! # Sheet naming
//!
//! Priority: `data-sheet` attribute → `<caption>` text → `"Sheet N"` (1-based).
//! Names are sanitized (strip `[]:*?/\`, truncate to 31 chars) and deduplicated
//! with `" 2"`, `" 3"`, … suffixes.
//!
//! # Value type inference
//!
//! - Integer / float (commas stripped) → Excel number.
//! - Percentage string (e.g. `"12.5%"`) **only** when `data-type="percent"` →
//!   number (`0.125`). Without the attribute, kept as a plain string.
//! - ISO date (`YYYY-MM-DD`) → string in v1 (no date formatting).
//! - Everything else → string.
//! - Leading/trailing whitespace trimmed; empty cell → blank.
//!
//! # v1 limitations
//!
//! - `rowspan` is not honored — cells fill left-to-right per row.
//! - ISO dates are written as strings, not Excel date numbers.
//! - No cell-level formatting beyond bold headers.

use rust_xlsxwriter::{Format, Workbook, Worksheet};
use scraper::{ElementRef, Html, Selector};

/// Inferred cell value written to the worksheet.
#[derive(Debug, Clone, PartialEq)]
enum CellValue {
    /// Numeric value (integer or float after comma stripping).
    Number(f64),
    /// String value (everything that doesn't parse as a number).
    Text(String),
    /// Empty / whitespace-only cell — skipped on write.
    Blank,
}

/// Sanitize a sheet name: strip Excel-forbidden characters and truncate to 31
/// characters.
fn sanitize_sheet_name(raw: &str) -> String {
    let sanitized: String = raw
        .chars()
        .filter(|c| !matches!(c, '[' | ']' | ':' | '*' | '?' | '/' | '\\'))
        .collect();
    let trimmed = sanitized.trim();
    trimmed.chars().take(31).collect()
}

/// Deduplicate a sheet name against already-used names by appending `" 2"`,
/// `" 3"`, … suffixes (truncating the base to keep the total ≤ 31 chars).
fn deduplicate_sheet_name(name: &str, existing: &[String]) -> String {
    if !existing.iter().any(|n| n == name) {
        return name.to_string();
    }
    for suffix in 2u32..1000 {
        let suffix_str = format!(" {suffix}");
        let max_base = 31_usize.saturating_sub(suffix_str.len());
        let base: String = name.chars().take(max_base).collect();
        let candidate = format!("{base}{suffix_str}");
        if !existing.iter().any(|n| n == &candidate) {
            return candidate;
        }
    }
    // Practically unreachable (< 1000 sheets with colliding names).
    name.to_string()
}

/// Infer the cell value from trimmed text and optional `data-type` attribute.
fn infer_value(text: &str, data_type: Option<&str>) -> CellValue {
    if text.is_empty() {
        return CellValue::Blank;
    }

    // Percentage with explicit data-type="percent" → decimal fraction.
    if data_type == Some("percent") {
        if let Some(stripped) = text.strip_suffix('%') {
            if let Ok(n) = stripped.trim().parse::<f64>() {
                return CellValue::Number(n / 100.0);
            }
        }
    }

    // Try integer / float with thousands separators (commas) stripped.
    let stripped = text.replace(',', "");
    if !stripped.is_empty() {
        if let Ok(n) = stripped.parse::<i64>() {
            #[allow(clippy::cast_precision_loss)]
            return CellValue::Number(n as f64);
        }
        if let Ok(n) = stripped.parse::<f64>() {
            return CellValue::Number(n);
        }
    }

    // Everything else is a string (including ISO dates in v1).
    CellValue::Text(text.to_string())
}

/// Write one `<tr>` row to the worksheet, expanding `colspan` and applying
/// bold formatting for header rows.
fn write_row(
    ws: &mut Worksheet,
    tr: &ElementRef<'_>,
    cell_sel: &Selector,
    row: u32,
    is_header: bool,
    bold_fmt: &Format,
) -> Result<(), String> {
    let mut col: u16 = 0;
    for cell in tr.select(cell_sel) {
        let colspan: u16 = cell
            .value()
            .attr("colspan")
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(1)
            .max(1);

        let data_type = cell.value().attr("data-type");
        let raw_text: String = cell.text().collect::<Vec<_>>().join("");
        let text = raw_text.trim();

        let value = infer_value(text, data_type);

        for c in col..col.saturating_add(colspan) {
            match &value {
                CellValue::Number(n) => {
                    if is_header {
                        ws.write_number_with_format(row, c, *n, bold_fmt)
                            .map_err(|e| format!("write cell ({row},{c}): {e}"))?;
                    } else {
                        ws.write_number(row, c, *n)
                            .map_err(|e| format!("write cell ({row},{c}): {e}"))?;
                    }
                }
                CellValue::Text(s) => {
                    if is_header {
                        ws.write_string_with_format(row, c, s.as_str(), bold_fmt)
                            .map_err(|e| format!("write cell ({row},{c}): {e}"))?;
                    } else {
                        ws.write_string(row, c, s.as_str())
                            .map_err(|e| format!("write cell ({row},{c}): {e}"))?;
                    }
                }
                CellValue::Blank => {
                    // Skip blank cells — visually identical to unwritten cells.
                }
            }
        }
        col = col.saturating_add(colspan);
    }
    Ok(())
}

/// Convert table HTML to an `.xlsx` (one sheet per `<table>`), returns file
/// bytes.
///
/// # Errors
///
/// Returns `Err` when the HTML contains no `<table>` elements or when xlsx
/// generation fails.
pub fn tables_html_to_xlsx(html: &str) -> Result<Vec<u8>, String> {
    let doc = Html::parse_document(html);

    let table_sel =
        Selector::parse("table").map_err(|e| format!("internal selector error: {e}"))?;
    let caption_sel =
        Selector::parse("caption").map_err(|e| format!("internal selector error: {e}"))?;
    let thead_sel =
        Selector::parse("thead tr").map_err(|e| format!("internal selector error: {e}"))?;
    let tbody_sel =
        Selector::parse("tbody tr").map_err(|e| format!("internal selector error: {e}"))?;
    let tr_sel = Selector::parse("tr").map_err(|e| format!("internal selector error: {e}"))?;
    let cell_sel =
        Selector::parse("th, td").map_err(|e| format!("internal selector error: {e}"))?;

    let tables: Vec<ElementRef<'_>> = doc.select(&table_sel).collect();
    if tables.is_empty() {
        return Err(
            "no <table> elements found — table HTML must contain at least one table".to_string()
        );
    }

    let mut workbook = Workbook::new();
    let mut sheet_names: Vec<String> = Vec::new();

    for (idx, table_el) in tables.iter().enumerate() {
        // Determine raw sheet name: data-sheet → caption → "Sheet N".
        let raw_name = table_el
            .value()
            .attr("data-sheet")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                table_el
                    .select(&caption_sel)
                    .next()
                    .map(|cap| {
                        let text: String = cap.text().collect();
                        text.trim().to_string()
                    })
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_else(|| format!("Sheet {}", idx + 1));

        let name = sanitize_sheet_name(&raw_name);
        let name = deduplicate_sheet_name(&name, &sheet_names);
        sheet_names.push(name.clone());

        let mut ws = Worksheet::new();
        ws.set_name(&name)
            .map_err(|e| format!("invalid sheet name '{name}': {e}"))?;

        let bold_fmt = Format::new().set_bold();

        let mut row_idx: u32 = 0;

        // Collect header rows (from thead) and body rows (from tbody).
        let thead_rows: Vec<ElementRef<'_>> = table_el.select(&thead_sel).collect();
        let tbody_rows: Vec<ElementRef<'_>> = table_el.select(&tbody_sel).collect();

        let has_structured_sections = !thead_rows.is_empty() || !tbody_rows.is_empty();

        if has_structured_sections {
            for tr in &thead_rows {
                write_row(&mut ws, tr, &cell_sel, row_idx, true, &bold_fmt)?;
                row_idx += 1;
            }
            for tr in &tbody_rows {
                write_row(&mut ws, tr, &cell_sel, row_idx, false, &bold_fmt)?;
                row_idx += 1;
            }
        } else {
            // Fallback: all <tr> descendants (tables without thead/tbody).
            let all_rows: Vec<ElementRef<'_>> = table_el.select(&tr_sel).collect();
            for tr in &all_rows {
                write_row(&mut ws, tr, &cell_sel, row_idx, false, &bold_fmt)?;
                row_idx += 1;
            }
        }

        workbook.push_worksheet(ws);
    }

    workbook
        .save_to_buffer()
        .map_err(|e| format!("xlsx generation failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use calamine::{Data, Reader, Xlsx};
    use std::io::{Cursor, Read};

    /// Helper: extract a numeric f64 from calamine Data (Int or Float).
    fn as_f64(data: &Data) -> Option<f64> {
        match data {
            Data::Int(v) => Some(*v as f64),
            Data::Float(v) => Some(*v),
            _ => None,
        }
    }

    /// Helper: open xlsx bytes with calamine.
    fn open_xlsx(bytes: Vec<u8>) -> Xlsx<Cursor<Vec<u8>>> {
        let cursor = Cursor::new(bytes);
        calamine::open_workbook_from_rs(cursor).unwrap()
    }

    // -----------------------------------------------------------------------
    // 1. Single table with numbers/strings/header → read-back verification.
    // -----------------------------------------------------------------------

    #[test]
    fn single_table_with_numbers_strings_header() {
        let html = r#"
            <table data-sheet="Sales">
                <thead>
                    <tr><th>Item</th><th>Qty</th><th>Price</th></tr>
                </thead>
                <tbody>
                    <tr><td>Widget</td><td>42</td><td>9.99</td></tr>
                    <tr><td>Gadget</td><td>7</td><td>1,234.56</td></tr>
                </tbody>
            </table>
        "#;

        let bytes = tables_html_to_xlsx(html).unwrap();
        let mut wb = open_xlsx(bytes);

        // Sheet name from data-sheet attribute.
        let names = wb.sheet_names();
        assert_eq!(names, vec!["Sales"]);

        let range = wb.worksheet_range("Sales").unwrap();

        // Header cells are strings.
        assert_eq!(range.get_value((0, 0)), Some(&Data::String("Item".to_string())));
        assert_eq!(range.get_value((0, 1)), Some(&Data::String("Qty".to_string())));
        assert_eq!(range.get_value((0, 2)), Some(&Data::String("Price".to_string())));

        // Body row 1: string, integer, float.
        assert_eq!(range.get_value((1, 0)), Some(&Data::String("Widget".to_string())));
        assert_eq!(as_f64(range.get_value((1, 1)).unwrap()), Some(42.0));
        assert!((as_f64(range.get_value((1, 2)).unwrap()).unwrap() - 9.99).abs() < 1e-9);

        // Body row 2: string, integer, float with thousands separator.
        assert_eq!(range.get_value((2, 0)), Some(&Data::String("Gadget".to_string())));
        assert_eq!(as_f64(range.get_value((2, 1)).unwrap()), Some(7.0));
        assert!((as_f64(range.get_value((2, 2)).unwrap()).unwrap() - 1234.56).abs() < 1e-9);
    }

    // -----------------------------------------------------------------------
    // 2. Two tables → two sheets with correct names.
    // -----------------------------------------------------------------------

    #[test]
    fn two_tables_produce_two_sheets() {
        let html = r#"
            <table>
                <caption>Revenue</caption>
                <tr><td>Q1</td><td>100</td></tr>
            </table>
            <table data-sheet="Costs">
                <tr><td>OpEx</td><td>50</td></tr>
            </table>
        "#;

        let bytes = tables_html_to_xlsx(html).unwrap();
        let wb = open_xlsx(bytes);

        let names = wb.sheet_names();
        assert_eq!(names.len(), 2);
        assert_eq!(names[0], "Revenue");
        assert_eq!(names[1], "Costs");
    }

    // -----------------------------------------------------------------------
    // 3. Colspan expansion: value appears in both columns on read-back.
    // -----------------------------------------------------------------------

    #[test]
    fn colspan_expansion() {
        let html = r#"
            <table>
                <tr><td colspan="2">Merged</td><td>Single</td></tr>
                <tr><td>A</td><td>B</td><td>C</td></tr>
            </table>
        "#;

        let bytes = tables_html_to_xlsx(html).unwrap();
        let mut wb = open_xlsx(bytes);

        let range = wb.worksheet_range_at(0).unwrap().unwrap();

        // "Merged" should appear in columns 0 and 1.
        assert_eq!(range.get_value((0, 0)), Some(&Data::String("Merged".to_string())));
        assert_eq!(range.get_value((0, 1)), Some(&Data::String("Merged".to_string())));
        assert_eq!(range.get_value((0, 2)), Some(&Data::String("Single".to_string())));

        // Second row is unaffected.
        assert_eq!(range.get_value((1, 0)), Some(&Data::String("A".to_string())));
        assert_eq!(range.get_value((1, 1)), Some(&Data::String("B".to_string())));
        assert_eq!(range.get_value((1, 2)), Some(&Data::String("C".to_string())));
    }

    // -----------------------------------------------------------------------
    // 4. Bold header: inspect styles.xml inside the xlsx zip.
    // -----------------------------------------------------------------------

    #[test]
    fn bold_header_in_styles_xml() {
        let html = r#"
            <table>
                <thead>
                    <tr><th>Header</th></tr>
                </thead>
                <tbody>
                    <tr><td>Body</td></tr>
                </tbody>
            </table>
        "#;

        let bytes = tables_html_to_xlsx(html).unwrap();
        let mut archive = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();

        // Read styles.xml — rust_xlsxwriter emits bold as <b/> inside a <font>.
        let mut styles_xml = String::new();
        archive
            .by_name("xl/styles.xml")
            .unwrap()
            .read_to_string(&mut styles_xml)
            .unwrap();

        // The bold font must be present in the styles.
        assert!(
            styles_xml.contains("<b/>") || styles_xml.contains("<b />"),
            "bold font marker <b/> not found in styles.xml:\n{styles_xml}"
        );

        // Verify that sheet1.xml row 0 references a non-default style index
        // (i.e. the bold format), while row 1 uses a different (or default)
        // style.
        let mut sheet_xml = String::new();
        archive
            .by_name("xl/worksheets/sheet1.xml")
            .unwrap()
            .read_to_string(&mut sheet_xml)
            .unwrap();

        // The header cell should have a style attribute (s="N" with N > 0).
        // rust_xlsxwriter writes cell elements like <c r="A1" s="1" t="s">.
        // We just verify that a style reference exists on the first cell.
        assert!(
            sheet_xml.contains("<c r=\"A1\" s=\""),
            "header cell A1 should reference a style in sheet1.xml:\n{sheet_xml}"
        );
    }

    // -----------------------------------------------------------------------
    // 5. No tables → Err.
    // -----------------------------------------------------------------------

    #[test]
    fn no_tables_returns_err() {
        let html = "<div><p>No tables here</p></div>";
        let result = tables_html_to_xlsx(html);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no <table> elements found"));
    }

    // -----------------------------------------------------------------------
    // Additional edge cases.
    // -----------------------------------------------------------------------

    #[test]
    fn percent_with_data_type() {
        let html = r#"
            <table>
                <tr><td data-type="percent">12.5%</td></tr>
                <tr><td>12.5%</td></tr>
            </table>
        "#;

        let bytes = tables_html_to_xlsx(html).unwrap();
        let mut wb = open_xlsx(bytes);

        let range = wb.worksheet_range_at(0).unwrap().unwrap();

        // With data-type="percent" → numeric 0.125.
        let val = as_f64(range.get_value((0, 0)).unwrap()).unwrap();
        assert!((val - 0.125).abs() < 1e-9);

        // Without data-type → kept as string.
        assert_eq!(range.get_value((1, 0)), Some(&Data::String("12.5%".to_string())));
    }

    #[test]
    fn sheet_name_sanitization() {
        let html = r#"
            <table data-sheet="Bad[]:*?/\Name">
                <tr><td>x</td></tr>
            </table>
        "#;

        let bytes = tables_html_to_xlsx(html).unwrap();
        let wb = open_xlsx(bytes);
        let names = wb.sheet_names();
        assert_eq!(names[0], "BadName");
    }

    #[test]
    fn sheet_name_deduplication() {
        let html = r#"
            <table data-sheet="Data"><tr><td>a</td></tr></table>
            <table data-sheet="Data"><tr><td>b</td></tr></table>
            <table data-sheet="Data"><tr><td>c</td></tr></table>
        "#;

        let bytes = tables_html_to_xlsx(html).unwrap();
        let wb = open_xlsx(bytes);
        let names = wb.sheet_names();
        assert_eq!(names, vec!["Data", "Data 2", "Data 3"]);
    }

    #[test]
    fn empty_cells_are_blank() {
        let html = r#"
            <table>
                <tr><td>A</td><td></td><td>C</td></tr>
            </table>
        "#;

        let bytes = tables_html_to_xlsx(html).unwrap();
        let mut wb = open_xlsx(bytes);
        let range = wb.worksheet_range_at(0).unwrap().unwrap();

        assert_eq!(range.get_value((0, 0)), Some(&Data::String("A".to_string())));
        // Empty cell should be Empty or absent.
        let mid = range.get_value((0, 1));
        assert!(
            mid.is_none() || matches!(mid, Some(&Data::Empty)),
            "empty cell should be blank, got {mid:?}"
        );
        assert_eq!(range.get_value((0, 2)), Some(&Data::String("C".to_string())));
    }

    #[test]
    fn fallback_sheet_naming() {
        let html = r#"
            <table><tr><td>a</td></tr></table>
            <table><tr><td>b</td></tr></table>
        "#;

        let bytes = tables_html_to_xlsx(html).unwrap();
        let wb = open_xlsx(bytes);
        let names = wb.sheet_names();
        assert_eq!(names, vec!["Sheet 1", "Sheet 2"]);
    }
}
