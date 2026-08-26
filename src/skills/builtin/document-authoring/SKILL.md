---
name: document-authoring
description: "Create documents (slides, docx, xlsx, html, markdown) with diagrams embedded"
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

Create documents (`write_report`) and embed diagrams (`svg_to_png`) with the
correct format-specific mechanism.

## When to create a visual

Decide based on the CONTENT, then pick the mechanism by the TARGET FORMAT:

| Content contains | Use | Tool |
|---|---|---|
| Structure / process / concept / hierarchy / timeline | A **diagram or infographic** (hand-authored SVG) | author SVG, then `svg_to_png` |
| Prose / argument / narrative only | **No visual** — text and tables only | — |

Rules of thumb:

- Qualitative structure (3-column breakdown, pyramid, flow, timeline) → **author
  SVG yourself** (this is your strength), then `svg_to_png` to rasterize.
- Do not create a visual for its own sake; a visual is justified when it
  clarifies structure or makes data readable.

## Format-specific embedding

After generating a diagram you get back `filename` (e.g.
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
3. For each diagram → author the SVG, then `svg_to_png(svg, filename)`.
4. Reference each visual in the document content using the format-specific rule.
5. Write the document with `write_report` using the appropriate `format`.

## Tools

This skill may use the following tools:
- `write_report` — write slides/docx/xlsx/html/markdown documents
- `svg_to_png` — rasterize hand-authored SVG diagrams/infographics to PNG

## Example Interactions

### Example 1 — diagram in slides

**User:** "做一个 3 页的架构介绍 PPT，带一张分层示意图"

**Action:**
1. Author the layered-pyramid SVG, then `svg_to_png` with `filename="layers"` → returns `layers.png`.
2. `write_report` with `format="slides"` and one canvas `<div class="slide">` containing `<img src="layers.png">`.

### Example 2 — prose only

**User:** "写一篇短文介绍量子计算"

**Action:** No visual — just `write_report(format="markdown")` with prose and, if useful, tables.
