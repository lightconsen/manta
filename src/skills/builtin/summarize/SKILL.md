---
name: summarize
description: "Summarize documents, conversations, URLs, and text content"
version: "1.0.0"
author: "manta"
triggers:
  - type: command
    pattern: "summarize"
    priority: 100
  - type: keyword
    pattern: "summarize"
    priority: 90
  - type: keyword
    pattern: "tldr"
    priority: 80
  - type: keyword
    pattern: "summary"
    priority: 70
openclaw:
  emoji: "📋"
  category: "productivity"
  tags:
    - "summarize"
    - "tldr"
    - "documents"
---

# Summarize Skill

Condense long content into concise, structured summaries.

## Capabilities

- Summarize plain text, markdown, or HTML content
- Fetch and summarize URLs or web pages
- Summarize conversation history or session context
- Extract key points, action items, and decisions
- Generate executive summaries, bullet-point lists, or paragraph summaries

## Usage Examples

### Summarize text
"Summarize this article: [paste text]"

### Summarize a URL
"Give me a TLDR of https://example.com/article"

### Summarize conversation
"Summarize what we've discussed so far"

### Bullet-point summary
"Give me the key points from this document in bullet form"

## Output Formats

- **Executive summary** — 2-3 sentence overview
- **Bullet points** — Key takeaways as a list
- **Structured** — Sections with headers (Overview, Key Points, Action Items)
- **One-liner** — Single sentence TLDR

## Best Practices

1. Specify desired output length (e.g., "in 3 sentences")
2. Indicate the audience (e.g., "for a non-technical reader")
3. Request specific sections (e.g., "focus on action items")
4. Use structured format for meeting notes and documents
