---
name: nano-pdf
description: "Read, extract text, and analyze PDF documents"
version: "1.0.0"
author: "manta"
triggers:
  - type: command
    pattern: "pdf"
    priority: 100
  - type: keyword
    pattern: "read pdf"
    priority: 90
  - type: keyword
    pattern: "extract pdf"
    priority: 80
  - type: keyword
    pattern: "pdf document"
    priority: 70
openclaw:
  emoji: "📄"
  category: "documents"
  tags:
    - "pdf"
    - "documents"
    - "text-extraction"
---

# Nano PDF Skill

Read and extract content from PDF documents for analysis.

## Capabilities

- Extract text content from PDF files
- Get document metadata (title, author, page count)
- Extract text from specific page ranges
- Handle multi-column and complex layouts
- Process password-protected PDFs (with password)

## Usage Examples

### Read a PDF
"Read the file report.pdf and summarize it"

### Extract specific pages
"Extract text from pages 5-10 of the document"

### Get metadata
"What are the metadata of this PDF file?"

### Analyze content
"Find all mentions of 'revenue' in quarterly-report.pdf"

## Supported Operations

- Full text extraction
- Page-by-page extraction
- Table detection and extraction
- Image description (when vision model available)
- Link extraction from PDFs

## Output Formats

- Plain text with page separators
- Markdown with structure preserved
- JSON with page-by-page content
- Summary with key sections

## Best Practices

1. For large PDFs, extract relevant pages rather than all text
2. Use page ranges to focus on specific sections
3. Combine with summarize skill for long documents
4. Handle scanned PDFs with OCR when needed
