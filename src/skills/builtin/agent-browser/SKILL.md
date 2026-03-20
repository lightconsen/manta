---
name: agent-browser
description: "Browse the web, fetch URLs, and extract content from web pages"
version: "1.0.0"
author: "manta"
triggers:
  - type: command
    pattern: "browse"
    priority: 100
  - type: keyword
    pattern: "fetch url"
    priority: 90
  - type: keyword
    pattern: "visit website"
    priority: 80
  - type: keyword
    pattern: "web search"
    priority: 70
openclaw:
  emoji: "🌐"
  category: "research"
  tags:
    - "browser"
    - "web"
    - "fetch"
    - "scraping"
---

# Agent Browser Skill

Browse the web and extract information from web pages.

## Capabilities

- Fetch and read web page content
- Extract text, links, and structured data
- Follow redirects and handle authentication
- Screenshot web pages (when headless browser available)
- Search the web and return results

## Usage Examples

### Fetch a page
"Fetch the content of https://example.com" or "Read this URL for me"

### Extract information
"Get all links from https://example.com/docs"

### Web search
"Search the web for Rust async runtime benchmarks"

### Read documentation
"Read the API docs at https://docs.example.com and summarize the authentication section"

## Supported Operations

- `GET` requests with custom headers
- HTML-to-markdown conversion for readable output
- JSON API responses
- RSS/Atom feed parsing
- Sitemap crawling

## Best Practices

1. Respect robots.txt and rate limits
2. Cache fetched pages to avoid redundant requests
3. Use markdown conversion for cleaner LLM input
4. Handle HTTP errors (404, 429, 5xx) gracefully
5. Set a reasonable timeout (default: 30 seconds)
