---
name: api-gateway
description: "Make HTTP API calls to external services and process responses"
version: "1.0.0"
author: "manta"
triggers:
  - type: command
    pattern: "api"
    priority: 100
  - type: keyword
    pattern: "api call"
    priority: 90
  - type: keyword
    pattern: "http request"
    priority: 80
  - type: keyword
    pattern: "rest api"
    priority: 80
openclaw:
  emoji: "🔌"
  category: "integration"
  tags:
    - "api"
    - "http"
    - "rest"
    - "integration"
---

# API Gateway Skill

Make HTTP requests to external APIs and process structured responses.

## Capabilities

- HTTP GET, POST, PUT, PATCH, DELETE requests
- JSON and form-data request bodies
- Custom headers and authentication (Bearer, API key, Basic)
- Response parsing (JSON, XML, text)
- Request chaining and data transformation
- Error handling and retry logic

## Usage Examples

### Simple GET request
"Call GET https://api.example.com/users and show me the results"

### POST with body
"POST to https://api.example.com/items with { name: 'test', value: 42 }"

### Authenticated request
"Fetch my profile from the API using Bearer token {token}"

### Chain requests
"Get the user list, then fetch details for each user"

## Authentication Methods

- **Bearer token** — `Authorization: Bearer <token>`
- **API key header** — `X-API-Key: <key>`
- **Basic auth** — `Authorization: Basic <base64>`
- **Query parameter** — `?api_key=<key>`

## Response Handling

- Extract specific fields with JSONPath expressions
- Transform responses for downstream use
- Handle pagination (next page links, offset/limit)
- Parse error responses with status codes

## Best Practices

1. Store API keys in secrets, not inline
2. Implement exponential backoff for rate limits
3. Validate response schemas before processing
4. Log request/response for debugging
5. Use idempotent requests where possible
