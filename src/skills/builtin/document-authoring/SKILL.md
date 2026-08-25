---
name: document-authoring
description: "Create documents (slides, docx, xlsx, html, markdown) with data charts and diagrams embedded"
version: "1.0.0"
author: "syscity"
triggers:
  - type: keyword
    pattern: "chart"
    priority: 80
  - type: keyword
    pattern: "diagram"
    priority: 80
  - type: keyword
    pattern: "infographic"
    priority: 80
  - type: keyword
    pattern: "report"
    priority: 70
  - type: keyword
    pattern: "presentation"
    priority: 70
  - type: keyword
    pattern: "slides"
    priority: 70
  - type: intent
    pattern: "visualize_data"
    priority: 80
  - type: intent
    pattern: "create_document"
    priority: 60
syscity:
  emoji: "📊"
  category: "documents"
  tags:
    - "charts"
    - "diagrams"
    - "documents"
    - "reports"
    - "presentations"
---

# Document Authoring Skill

Create documents (`write_report`) and embed visualizations (`generate_chart`,
`svg_to_png`) with the correct format-specific mechanism.

## When to create a visual

Decide based on the CONTENT, then pick the mechanism by the TARGET FORMAT:

| Content contains | Use | Tool |
|---|---|---|
| Numeric data / series / time series / categories-with-values | A **data chart** (bar/line/pie) | `generate_chart` |
| Structure / process / concept / hierarchy / timeline | A **diagram or infographic** (hand-authored SVG) | author SVG, then `svg_to_png` |
| Prose / argument / narrative only | **No visual** — text and tables only | — |

Rules of thumb:

- Numbers that must be accurate (axes, scales, proportions) → **always**
  `generate_chart`; never hand-write chart SVG (the LLM's hand-computed
  coordinates are error-prone).
- Qualitative structure (3-column breakdown, pyramid, flow, timeline) → **author
  SVG yourself** (this is your strength), then `svg_to_png` to rasterize.
- Do not create a visual for its own sake; a visual is justified when it
  clarifies structure or makes data readable.

## Format-specific embedding

After generating a chart/diagram you get back `filename` (e.g.
`chart-123.png`), `png_url`, and `svg_url`. Reference them per format:

| Target format | How to embed |
|---|---|
| `slides` (pptx) | `<img src="<filename>.png">` inside the canvas HTML (same artifact dir) |
| `docx` | `<img src="<filename>.png">` as a block-level element in the flowing HTML |
| `html` | `<img src="<png_url>">` **or** inline the `<svg>` directly (self-contained) |
| `markdown` | `![alt](<png_url>)` |
| `xlsx` | N/A (use `rust_xlsxwriter` native charts via a future tool; do not embed images) |

## Steps

When the user asks for a document with a visualization:

1. Decide the target format (`slides` / `docx` / `xlsx` / `html` / `markdown`).
2. Decide, per the table above, which visuals (if any) are warranted.
3. For each data chart → `generate_chart(chart_type, series, categories, title)`.
4. For each diagram → author the SVG, then `svg_to_png(svg, filename)`.
5. Reference each visual in the document content using the format-specific rule.
6. Write the document with `write_report` using the appropriate `format`.

## Tools

This skill may use the following tools:
- `write_report` — write slides/docx/xlsx/html/markdown documents
- `generate_chart` — render bar/line/pie data charts to SVG + PNG
- `svg_to_png` — rasterize hand-authored SVG diagrams/infographics to PNG

## Example Interactions

### Example 1 — data chart in a report

**User:** "画个本月各产品销量柱状图，写进周报（docx）"

**Action:**
1. `generate_chart` with `chart_type="bar"`, `series=[{name:"销量", data:[1200,800,1500]}]`, `categories=["产品A","产品B","产品C"]`, `filename="sales"` → returns `sales.png`.
2. `write_report` with `format="docx"` and content containing `<img src="sales.png">`.

### Example 2 — diagram in slides

**User:** "做一个 3 页的架构介绍 PPT，带一张分层示意图"

**Action:**
1. Author the layered-pyramid SVG, then `svg_to_png` with `filename="layers"` → returns `layers.png`.
2. `write_report` with `format="slides"` and one canvas `<div class="slide">` containing `<img src="layers.png">`.

### Example 3 — prose only

**User:** "写一篇短文介绍量子计算"

**Action:** No visual — just `write_report(format="markdown")` with prose and, if useful, tables.
