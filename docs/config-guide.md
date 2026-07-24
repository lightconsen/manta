# Configuration Guide

Reference for all gateway-level configuration options in `syscity.toml`.

## File Location

Syscity loads configuration from the following locations (first found wins):

1. `./syscity.toml` (current working directory)
2. `~/.config/syscity/syscity.toml`
3. `$XDG_CONFIG_HOME/syscity/syscity.toml`
4. Environment variables (`SYSCITY_*`)

## Structure Overview

```toml
# ── Network ──────────────────────────────────────────────────────────
host = "127.0.0.1"
port = 18080

# ── LLM ──────────────────────────────────────────────────────────────
model = "claude-3-sonnet-20240229"
model_provider = "anthropic"

# ── Providers ────────────────────────────────────────────────────────
[providers]
anthropic = { type = "anthropic", api_key = "$ANTHROPIC_API_KEY" }
openai = { type = "openai", api_key = "$OPENAI_API_KEY" }

# ── Default Agent ────────────────────────────────────────────────────
[default_agent]
system_prompt = "You are a helpful assistant."

# ── Memory ───────────────────────────────────────────────────────────
[vector_memory]
enabled = false
# ...

# ── Channels ─────────────────────────────────────────────────────────
[channels]
# ...

# ── Security ─────────────────────────────────────────────────────────
[security]
enabled = true
# ...

# ── Plugins ──────────────────────────────────────────────────────────
[plugins]
enabled = true

# ── Cron ─────────────────────────────────────────────────────────────
[cron]
enabled = true

# ── Heartbeat ────────────────────────────────────────────────────────
[heartbeat]
# ...

# ── Storage ──────────────────────────────────────────────────────────
[storage]
type = "sqlite"
# ...

# ── Hot Reload ───────────────────────────────────────────────────────
[hot_reload]
enabled = true

# ── Cost Guard ───────────────────────────────────────────────────────
[cost_guard]
daily_limit_cents = 0

# ── Workspace ────────────────────────────────────────────────────────
workspace_dir = "~/.syscity/workspace"
workspace_only = false

# ── MCP Servers ──────────────────────────────────────────────────────
[mcp]
# ...

# ── ACP (Subagent Control) ──────────────────────────────────────────
[acp]
enabled = true
```

## `[vector_memory]` — Semantic Memory

Controls vector-based semantic search and retrieval-augmented generation (RAG).

```toml
[vector_memory]
# Master switch. Disabled by default to avoid blocking startup on model download.
enabled = false

# ── Embedding Provider ───────────────────────────────────────────────
# "open_ai" or "local_gguf"
provider = "local_gguf"

# API-based embeddings (OpenAI-compatible)
embedding_api_key = "$OPENAI_API_KEY"
embedding_model = "text-embedding-3-small"
embedding_dimension = 1536
api_base_url = "https://api.openai.com/v1"   # or Azure endpoint

# Local GGUF embeddings
local_model_path = "hf:unsloth/embedding-gemma-2b-GGUF/embedding-gemma-2b-Q4_K_M.gguf"

# ── Chunking Strategy ────────────────────────────────────────────────
# Controls how documents are split before embedding.
# Options:
#   [vector_memory.chunk_strategy]
#   type = "fixed"       # word-level sliding window (legacy)
#   chunk_size = 512
#   chunk_overlap = 50
#
#   [vector_memory.chunk_strategy]
#   type = "recursive"   # hierarchical separator splitting (default)
#   chunk_size = 512
#   # separators = ["\n\n", "\n", ". ", " "]  # default, can override

# ── Query Transformer (HyDE) ─────────────────────────────────────────
# Rewrites the query before embedding for better retrieval.
[vector_memory.query_transformer]

# Enable Hypothetical Document Embeddings: uses the default LLM to generate
# a hypothetical answer, then embeds that instead of the raw query.
# Note: adds one LLM call per search. Disabled by default.
enable_hyde = false

# Optional model override for HyDE generation (default: system default model)
# hyde_model = "claude-3-haiku-20240307"

# ── Cross-encoder Reranker ───────────────────────────────────────────
# Re-ranks initial results with a cross-encoder for finer relevance scoring.
[vector_memory.reranker]

enabled = false
api_key = "your-cohere-api-key"
model = "rerank-english-v3.0"
top_k = 10

# ── Context Window Budget ────────────────────────────────────────────
# Limits how many memories can be injected into context by token budget,
# preventing context overflow.
[vector_memory.context_window]

enabled = false
max_tokens = 128000           # LLM context window size
reserved_for_response = 4096  # tokens reserved for generation
min_chunks = 1                # minimum memories to keep even if over budget
```

### Feature Defaults Summary

| Feature | Default | Rationale |
|---------|---------|-----------|
| Vector memory | `enabled = false` | Avoids blocking startup on model download |
| Chunking strategy | `recursive` | Pure improvement over `fixed`, no downside |
| HyDE | `disabled` | Adds LLM call per search (latency + cost) |
| Reranker | `disabled` | Requires Cohere API key (external dependency) |
| Context window | `disabled` | Memory truncation should be opt-in |

## `[providers]` — LLM Providers

```toml
[providers]
anthropic = { type = "anthropic", api_key = "$ANTHROPIC_API_KEY", base_url = "https://api.anthropic.com" }
openai = { type = "openai", api_key = "$OPENAI_API_KEY" }
gemini = { type = "gemini", api_key = "$GEMINI_API_KEY" }
openrouter = { type = "openai", api_key = "$OPENROUTER_API_KEY", base_url = "https://openrouter.ai/api/v1" }
```

## `[security]` — Authentication & Rate Limiting

```toml
[security]
enabled = true
auth_required = true
auth_mode = "jwt"           # jwt, none, tailscale, trusted_proxy
# shared_token = "..."      # simple shared secret auth

[security.rate_limit]
enabled = true
capacity = 60               # max requests per window
refill_rate = 1.0           # tokens per second
multi_tier = false          # use sliding window instead of token bucket
```

## `[cron]` — Scheduled Tasks

```toml
[cron]
enabled = true
check_interval_seconds = 60
```

## `[hot_reload]` — Runtime Config Changes

```toml
[hot_reload]
enabled = true
watch_config = true
watch_agents = true
watch_plugins = true
debounce_seconds = 2
```

## `[heartbeat]` — Health Monitoring

```toml
[heartbeat]
enabled = true
interval_seconds = 30
```

## `[storage]` — Data Persistence

```toml
[storage]
type = "sqlite"             # sqlite (default)
# path = "~/.syscity/data/syscity.db"
```

## Environment Variables

All config values can be overridden via environment variables with the `SYSCITY_` prefix:

```bash
export SYSCITY_HOST=0.0.0.0
export SYSCITY_PORT=8080
export SYSCITY_VECTOR_MEMORY_ENABLED=true
export SYSCITY_VECTOR_MEMORY_EMBEDDING_API_KEY=sk-...
```

Secret values (`$ANTHROPIC_API_KEY`, `$OPENAI_API_KEY`, etc.) in the config file
are resolved via `SecretResolver` which supports:

- `$ENV_VAR` — environment variable
- `file:///path/to/secret` — file content
- `exec://command` — command stdout
