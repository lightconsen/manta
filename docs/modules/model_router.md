# Model Router Module

Multi-provider LLM support with fallback chain, auth management, and usage tracking.

## Design

- **`ModelRouter`** — Main router that selects providers based on model aliases, task type, and health
- **`ModelAlias`** — Maps friendly names (e.g., "fast", "smart") to actual provider + model
- **`ProviderConfig`** — Per-provider configuration with auth, timeout, retry settings
- **`AuthProfile`** / **`AuthProfileManager`** — API key rotation with cooldown and failure tracking
- **`AuthProfileStore`** — SQLite persistence for auth profile state
- **`ModelCatalog`** — Dynamic model registry with discovery and suppression
- **`OAuthFlow`** / **`pkce.rs`** — OAuth 2.0 + PKCE authorization flow
- **`UsageTracker`** — Per-provider token consumption and cost estimation
- **`UsageFetcher`** — Remote usage fetch from provider billing APIs
- **`FailureClass`** — Differentiated failure classification with backoff

### Architecture

```
Model Request
    │
    ├──▶ Model Alias Resolution ──▶ ProviderConfig
    │
    ├──▶ Capability-Based Routing ──▶ ModelCatalog
    │       │
    │       └──▶ vision? tools? reasoning? ──▶ best model
    │
    ├──▶ Auth Profile Selection ──▶ AuthProfileManager
    │       │
    │       └──▶ Key rotation on failure
    │
    ├──▶ Provider Execution
    │       │
    │       └──▶ Failure ──▶ Fallback Chain
    │
    └──▶ Usage Tracking ──▶ UsageTracker
```

## Key Types

```rust
pub struct ModelRouter {
    aliases: HashMap<String, ModelAlias>,
    providers: HashMap<String, Arc<dyn Provider>>,
    fallback_chain: Vec<String>,
    usage_tracker: ProviderUsageTracker,
}

pub struct ModelAlias {
    pub name: String,
    pub provider: String,
    pub model: String,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

pub struct ProviderConfig {
    pub provider_type: ProviderType,
    pub api_key: String,
    pub api_keys: Vec<String>,
    pub auth_profile: Option<AuthProfileConfig>,
    pub oauth: Option<OAuthConfig>,
    pub base_url: Option<String>,
    pub timeout: Duration,
    pub max_retries: u32,
    pub retry_delay_ms: u64,
}

pub struct AuthProfile {
    pub label: String,
    pub key: String,
    pub status: ProfileStatus,
    pub failures: u32,
    pub cooldown_until: Option<DateTime<Utc>>,
}

pub enum ProfileStatus {
    Active,
    Cooldown,
    Disabled,
}

pub struct ModelCatalogEntry {
    pub id: String,
    pub provider: String,
    pub capabilities: Vec<String>,
    pub pricing: ModelPricing,
    pub suppressed: bool,
}
```

## Implemented Features

- Model alias resolution with override support
- Multi-provider routing with health checking
- Automatic fallback chain on provider failure
- Auth profile rotation with cooldown and failure tracking
- SQLite persistence for auth state
- OAuth 2.0 + PKCE authorization flow
- Model catalog with dynamic discovery
- Model suppression for unavailable models
- Capability-based model selection (vision, tools, reasoning)
- Usage tracking with time windows (today/hour/month)
- Cost estimation per provider
- Budget enforcement
- Differentiated failure classification with backoff
- Remote usage fetch from OpenAI billing API
- Usage formatting and reporting

