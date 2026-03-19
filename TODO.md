# Manta TODO

## Features to Consider

### 1. Advanced Secrets Management

**Status:** Under consideration
**Priority:** Medium
**Complexity:** High

#### Description

Implement a multi-source secret resolution system similar to OpenClaw's secrets management. This would allow API keys, tokens, and credentials to be stored outside the main configuration file while being dynamically resolved at runtime.

#### Motivation

Currently, Manta stores secrets inline in `manta.toml`:

```toml
[providers.anthropic]
api_key = "sk-ant-xxx..."  # Directly in config file
```

This has several drawbacks:
- Risk of accidentally committing secrets to version control
- No integration with enterprise secret management systems
- Secrets are readable by any process with file access
- No resilience if secrets need rotation

#### Proposed Design

##### Phase 1: Basic Secret References

Support referencing secrets from environment variables:

```toml
[providers.anthropic]
api_key = { env = "ANTHROPIC_API_KEY" }
```

Or using a shorthand syntax:

```toml
[providers.anthropic]
api_key = "$ANTHROPIC_API_KEY"
```

##### Phase 2: File-Based Secrets

Support reading secrets from files (useful for Docker/Kubernetes):

```toml
[secrets.providers.docker]
source = "file"
base_path = "/run/secrets"

[providers.anthropic]
api_key = { source = "file", provider = "docker", id = "anthropic_key" }
# Reads from /run/secrets/anthropic_key
```

##### Phase 3: External Executables

Support calling external executables for secret resolution (Vault, 1Password, etc.):

```toml
[secrets.providers.vault]
source = "exec"
command = "vault kv get -field={{.Field}} {{.Path}}"

[providers.anthropic]
api_key = { source = "exec", provider = "vault", id = "secrets/manta/anthropic" }
```

##### Phase 4: Runtime Snapshot System

Implement a runtime snapshot system for resilience:

1. Resolve all secrets at startup into a "snapshot"
2. Cache resolved values with TTL
3. Support "last-known-good" fallback if resolution fails
4. Hot-reload secrets without restart

#### Implementation Notes

**Rust crates to consider:**
- `handlebars` or `tera` for template interpolation in exec commands
- `tokio::process` for async exec resolution
- `dashmap` or `arc-swap` for thread-safe runtime snapshots

**Key design decisions:**
- Keep it optional - inline secrets should still work
- Fail fast on startup if secrets can't be resolved (unless degraded mode enabled)
- Implement caching with configurable TTL
- Provide clear error messages when resolution fails

#### Reference

See OpenClaw's implementation:
- `src/secrets/runtime.ts` - Runtime snapshot management
- `src/secrets/resolve.ts` - Resolution logic
- `src/secrets/ref-contract.ts` - SecretRef validation
- `src/config/types.secrets.ts` - Type definitions

#### Acceptance Criteria

- [ ] Secret references work for all provider API keys
- [ ] Environment variable source implemented
- [ ] File source implemented
- [ ] Exec source implemented (stretch goal)
- [ ] Documentation updated with examples
- [ ] Backward compatibility maintained

---

### 2. Advanced Cron Scheduler (OpenClaw-Style)

**Status:** Under consideration
**Priority:** Medium
**Complexity:** High

#### Description

Replace Manta's simple shell-command cron with a production-grade scheduler supporting AI agent execution, multi-channel delivery, and enterprise reliability features similar to OpenClaw's cron system.

#### Current State

Manta's current cron is minimal:
- Executes shell commands only (`sh -c "command"`)
- 30-second polling interval
- No retry logic
- stdout delivery only

```rust
// Current: Simple shell execution
let output = Command::new("sh")
    .arg("-c")
    .arg(&job.prompt)
    .output()
    .await;
```

#### Proposed Design

##### Phase 1: Timer-Based Scheduling

Replace polling with exact-time timer-based scheduling:

```rust
pub struct CronService {
    jobs: Arc<RwLock<HashMap<String, CronJob>>>,
    timer: Option<JoinHandle<()>>,
    next_job: Option<String>,
}

impl CronService {
    async fn arm_timer(&mut self) {
        // Find next job
        let now = Utc::now();
        let next = self.find_next_job(now).await;

        if let Some((job_id, run_at)) = next {
            let delay = run_at.signed_duration_since(now);

            // Arm exact timer
            self.timer = Some(tokio::spawn(async move {
                tokio::time::sleep(delay.to_std().unwrap()).await;
                self.execute_job(&job_id).await;
                self.arm_timer().await;  // Rearm for next
            }));
        }
    }
}
```

##### Phase 2: Agent-Based Execution

Support both shell commands AND AI agent execution:

```rust
pub enum ExecutionTarget {
    /// Execute shell command (current behavior)
    Shell { command: String },
    /// Execute via AI agent (new)
    Agent {
        agent_id: Option<String>,  // None = default agent
        prompt: String,
        context: Option<String>,
    },
}

pub enum SessionTarget {
    /// Run in main session (has conversation context)
    Main,
    /// Run in isolated session (clean state: cron:{job_id})
    Isolated,
}
```

Isolated execution example:

```rust
async fn execute_isolated(&self, job: &CronJob) -> Result<String> {
    // Create isolated session
    let session_id = format!("cron:{}", job.id);
    let agent = self.get_agent(job.agent_id.as_deref()).await?;

    // Process through agent
    let message = IncomingMessage::new(
        "system",
        &session_id,
        &job.prompt
    );

    let response = agent.process_message(message).await?;
    Ok(response.content)
}
```

##### Phase 3: Multi-Channel Delivery

Add delivery system for job results:

```rust
pub enum DeliveryMode {
    /// No delivery (fire-and-forget)
    None,
    /// Send to messaging channel
    Announce {
        channel: String,  // "slack", "discord", "telegram"
        to: String,       // channel_id or user_id
    },
    /// POST to webhook URL
    Webhook {
        url: String,
        headers: HashMap<String, String>,
    },
}

pub struct DeliveryConfig {
    pub mode: DeliveryMode,
    pub best_effort: bool,  // Continue on delivery failure
    pub on_failure: Option<DeliveryMode>,  // Fallback delivery
}
```

##### Phase 4: Schedule Types

Support multiple schedule formats:

```rust
pub enum Schedule {
    /// One-shot execution at specific time
    At { timestamp: DateTime<Utc> },
    /// Fixed interval
    Every {
        interval: Duration,
        anchor: Option<DateTime<Utc>>,
    },
    /// Cron expression (5 or 6 field)
    Cron {
        expression: String,
        timezone: Option<String>,  // IANA timezone
        stagger_ms: Option<u64>,   // Random stagger for load
    },
}
```

Features:
- **Timezone support**: `0 9 * * *` in `America/New_York`
- **Automatic stagger**: Jobs at `0 * * * *` get 0-5min random stagger
- **One-shot jobs**: Auto-delete after execution

##### Phase 5: Reliability Features

**Retry Logic:**

```rust
pub struct RetryConfig {
    pub max_retries: u32,
    pub backoff: BackoffStrategy,  // Exponential, Linear, Fixed
}

// Exponential backoff tiers
const BACKOFF_TIERS: [Duration; 5] = [
    Duration::from_secs(30),   // 1st error
    Duration::from_secs(60),   // 2nd error
    Duration::from_secs(300),  // 3rd error
    Duration::from_secs(900),  // 4th error
    Duration::from_secs(3600), // 5th+ error
];
```

**Transient Error Detection:**

```rust
fn is_transient_error(error: &Error) -> bool {
    match error {
        // Rate limits
        Error::Http(429) => true,
        // Server overload
        Error::Http(529) => true,
        // 5xx errors
        Error::Http(500..=599) => true,
        // Timeouts
        Error::Timeout => true,
        // Network errors
        Error::Network(_) => true,
        _ => false,
    }
}
```

**Crash Recovery:**

```rust
async fn start(&mut self) {
    // 1. Load jobs from store
    self.load_jobs().await;

    // 2. Clear stale running markers
    // (Jobs marked running when process crashed)
    for job in self.jobs.values_mut() {
        if job.state.running_at_ms.is_some() {
            job.state.running_at_ms = None;
            job.state.last_error = Some("Recovered from crash".to_string());
        }
    }

    // 3. Run missed jobs with stagger
    let now = Utc::now();
    for job in self.jobs.values_mut() {
        if let Some(next) = job.state.next_run_at {
            if next < now {
                // Queue missed job with random stagger
                self.run_missed_with_stagger(job).await;
            }
        }
    }

    // 4. Arm timer for next job
    self.arm_timer().await;
}
```

##### Phase 6: Run History & Logging

JSONL-based run logs with automatic pruning:

```rust
pub struct RunLogEntry {
    pub run_id: String,
    pub job_id: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub status: RunStatus,  // Ok, Error, Skipped
    pub output: Option<String>,
    pub error: Option<String>,
    pub delivery_status: Option<DeliveryStatus>,
}

pub struct RunLog {
    pub job_id: String,
    pub max_bytes: usize,      // Default: 2MB
    pub max_lines: usize,      // Default: 2000
}
```

Automatic pruning on write:

```rust
async fn append_run(&mut self, entry: RunLogEntry) {
    let line = serde_json::to_string(&entry).unwrap();

    // Check size limits
    let current_size = self.file.metadata().await.unwrap().len();
    if current_size + line.len() > self.max_bytes {
        self.prune_old_entries().await;
    }

    // Append newline-delimited JSON
    self.file.write_all(line.as_bytes()).await.unwrap();
    self.file.write_all(b"\n").await.unwrap();
}
```

#### Configuration

```toml
[cron]
enabled = true
store_path = "~/.manta/cron/jobs.json"
max_concurrent_runs = 3

[cron.retry]
max_retries = 3
backoff = "exponential"

[cron.session_retention]
completed_sessions = "24h"  # Keep for 24 hours
cancelled_sessions = "1h"

[cron.run_log]
max_bytes = "2MB"
keep_lines = 2000

[cron.failure_alert]
enabled = true
threshold_errors = 3
notify_channel = "slack"
notify_to = "#alerts"
```

#### Job Definition

```rust
pub struct CronJob {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub delete_after_run: bool,  // One-shot jobs

    // Schedule
    pub schedule: Schedule,

    // Execution
    pub execution: ExecutionTarget,
    pub session_target: SessionTarget,

    // Delivery
    pub delivery: Option<DeliveryConfig>,

    // Runtime state
    pub state: JobState,
}

pub struct JobState {
    pub next_run_at: Option<DateTime<Utc>>,
    pub running_at_ms: Option<u64>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_run_status: Option<RunStatus>,
    pub last_error: Option<String>,
    pub consecutive_errors: u32,
    pub run_count: u32,
}
```

#### Implementation Notes

**Rust crates to consider:**
- `cron` or `cron-parser` for cron expression parsing
- `cron_clock` for timezone-aware scheduling
- `chrono-tz` for IANA timezone support
- `serde_json` for JSONL run logs
- `tokio::time` for timer-based scheduling
- `dashmap` for concurrent job storage

**Key design decisions:**
- Keep shell execution for backward compatibility
- Agent execution is opt-in per job
- Isolated sessions prevent cron jobs from polluting main conversation
- Delivery failures don't fail the job (if best_effort=true)
- Run logs are optional but recommended

**Migration path:**
1. Parse existing cron jobs into new format
2. Keep shell execution as default
3. Allow gradual migration to agent-based jobs

#### Reference

See OpenClaw's implementation:
- `src/cron/service.ts` - Main cron service
- `src/cron/service/timer.ts` - Timer-based scheduling
- `src/cron/schedule.ts` - Schedule computation
- `src/cron/isolated-agent/run.ts` - Isolated agent execution
- `src/cron/delivery.ts` - Delivery orchestration
- `src/cron/run-log.ts` - Run history logging
- `src/cron/types.ts` - Type definitions

#### Acceptance Criteria

- [ ] Timer-based scheduling (not polling)
- [ ] Multiple schedule types (at, every, cron)
- [ ] Timezone support for cron expressions
- [ ] Agent-based execution (main + isolated sessions)
- [ ] Multi-channel delivery (announce, webhook)
- [ ] Exponential backoff retry
- [ ] Transient error detection
- [ ] Crash recovery (missed job handling)
- [ ] Run history with JSONL logs
- [ ] Automatic log pruning
- [ ] Backward compatibility with shell jobs

---

### 3. Session System Improvements

**Status:** Under consideration
**Priority:** High
**Complexity:** Medium

#### Description

Refactor Manta's session system based on design recommendations. Keep Manta's multi-agent advantage while adding SQLite persistence for reliability. Avoid OpenClaw's complex file-locking JSON approach.

#### Current State

Manta sessions are ephemeral (in-memory only):
```rust
pub struct MultiAgentSession {
    pub id: String,
    pub agents: HashMap<String, SessionAgent>,
    pub shared_context: Arc<RwLock<HashMap<String, String>>>,
    // No persistence - lost on restart
}
```

OpenClaw uses complex JSON files with locking:
- File-based session store (`sessions.json`)
- Complex locking with queues
- 45-second TTL caching
- Manual pruning/capping logic

#### Recommended Design

##### Phase 1: SQLite Session Storage

Replace JSON files with SQLite for simplicity and reliability:

```rust
pub struct SessionStore {
    db: SqlitePool,  // Single database vs many JSON files
}

impl SessionStore {
    // Atomic, reliable, no file locking needed
    pub async fn save_session(&self, session: &Session) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO sessions (id, data, updated_at)
             VALUES (?, ?, ?)"
        )
        .bind(&session.id)
        .bind(serde_json::to_string(session)?)
        .bind(Utc::now())
        .execute(&self.db)
        .await?;
        Ok(())
    }

    pub async fn load_session(&self, session_id: &str) -> Result<Option<Session>> {
        let row = sqlx::query_as::<_, (String,)> (
            "SELECT data FROM sessions WHERE id = ?"
        )
        .bind(session_id)
        .fetch_optional(&self.db)
        .await?;

        match row {
            Some((data,)) => Ok(Some(serde_json::from_str(&data)?)),
            None => Ok(None),
        }
    }
}
```

**Advantages over OpenClaw:**
- No file locking complexity
- Single database file vs scattered JSON
- Built-in ACID guarantees
- Easier querying (SQL vs manual iteration)
- Automatic crash recovery

##### Phase 2: Simplified Session Metadata

Keep UUID session IDs (simpler than OpenClaw's hierarchical keys), add metadata table:

```rust
pub struct SessionMetadata {
    pub session_id: String,      // UUID
    pub agent_id: String,        // "main", "coder"
    pub channel: String,         // "discord", "telegram"
    pub channel_id: String,      // user/channel ID
    pub created_at: DateTime<Utc>,
}
```

**Query by metadata instead of parsing keys:**
```sql
-- Simple query instead of complex key parsing
SELECT * FROM sessions
WHERE agent_id = 'main' AND channel = 'discord' AND channel_id = '12345';
```

**Why it's better than OpenClaw:**
- No complex key parsing logic (`agent:{id}:{channel}:{peerKind}:{peerId}`)
- Flexible querying
- Can change metadata without migrating keys
- Simpler routing logic

##### Phase 3: Hybrid Context Model

Combine OpenClaw's persistence with Manta's flexibility:

```rust
pub struct Session {
    pub id: String,
    pub agents: Vec<SessionAgent>,

    // NEW: Conversation history (learn from OpenClaw)
    pub conversation: Vec<Message>,  // Last N messages (in-memory)

    // NEW: SQLite-backed persistence
    pub storage: Arc<SessionStorage>,
}

impl Session {
    // Persist conversation turns automatically
    pub async fn add_message(&mut self, msg: Message) -> Result<()> {
        self.conversation.push(msg.clone());

        // Async persist to SQLite (non-blocking)
        self.storage.append_message(&self.id, &msg).await?;

        // Trim if too long (keep last 100)
        if self.conversation.len() > 100 {
            self.conversation.drain(0..self.conversation.len() - 100);
        }

        Ok(())
    }
}
```

##### Phase 4: Simplified Thread Binding

Keep Manta's concept but make it clearer for personal use:

```rust
pub enum ContextMode {
    /// Private agent context (default)
    Private,
    /// Share with specific agents
    SharedWith(Vec<String>),
    /// Read-only access to another agent's context
    Observer(String),
}

pub struct SessionAgent {
    pub id: String,
    pub personality: AgentPersonality,
    pub context_mode: ContextMode,
    pub thread_id: String,
    pub is_active: bool,
    // REMOVED: Complex status machine (OpenClaw has this)
    // REMOVED: spawnDepth tracking (overkill for personal use)
    // REMOVED: subagentRole (too enterprisey)
}
```

##### Phase 5: Keep Simple Session Routing

Manta's current approach is better for single-node:

```rust
pub struct GatewayState {
    pub sessions: Arc<RwLock<HashMap<String, Session>>>,
    pub session_routing: Arc<RwLock<HashMap<String, String>>>, // session_id -> agent_id
}
```

**Don't adopt OpenClaw's 7-tier binding resolution** - it's overkill for personal use.

#### Database Schema

```sql
-- Sessions table
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    data BLOB NOT NULL,  -- JSON serialized session
    updated_at DATETIME NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- Session metadata for routing
CREATE TABLE session_metadata (
    session_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    channel TEXT,
    channel_id TEXT,
    created_at DATETIME NOT NULL,
    FOREIGN KEY (session_id) REFERENCES sessions(id)
);

-- Message history (optional persistence)
CREATE TABLE session_messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    role TEXT NOT NULL,  -- "user", "assistant", "system"
    content TEXT NOT NULL,
    timestamp DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (session_id) REFERENCES sessions(id)
);

-- Indexes for fast lookups
CREATE INDEX idx_sessions_updated ON sessions(updated_at);
CREATE INDEX idx_metadata_agent ON session_metadata(agent_id);
CREATE INDEX idx_metadata_channel ON session_metadata(channel, channel_id);
CREATE INDEX idx_messages_session ON session_messages(session_id, timestamp);
```

#### Implementation Notes

**Rust crates to consider:**
- `sqlx` for async SQLite operations
- `serde_json` for serialization
- `tokio::sync::RwLock` for in-memory cache
- `dashmap` for concurrent session access (optional)

**Key design decisions:**
- Keep multi-agent sessions as core differentiator
- SQLite over JSON files (simpler, more reliable)
- UUIDs over hierarchical keys (simpler routing)
- Automatic persistence on message add
- Lazy loading (load from SQLite on demand)
- In-memory LRU cache for active sessions

**Migration path:**
1. Add SQLite storage alongside existing in-memory sessions
2. Persist new sessions to SQLite
3. Load from SQLite on cache miss
4. Remove JSON file dependency (if any existed)

#### Comparison with OpenClaw

| Feature | OpenClaw | **Recommended Manta** |
|---------|----------|----------------------|
| Session type | Single-agent | **Multi-agent (keep)** |
| Persistence | JSON files + complex locking | **SQLite (simpler)** |
| Session IDs | Hierarchical keys | **UUIDs (simpler)** |
| Context sharing | None | **Simplified binding** |
| History | Transcript files | **SQLite messages table** |
| Token tracking | Per session | **Optional, per agent** |
| Max sessions | 1000 cap with pruning | **SQLite performance limit** |
| Routing | 7-tier binding resolution | **Simple HashMap** |

#### Reference

See OpenClaw's implementation (for reference only - don't copy complexity):
- `src/config/sessions/store.ts` - Complex file-based store
- `src/config/sessions/types.ts` - Session entry types
- `src/config/sessions/store-maintenance.ts` - Pruning/capping logic
- `src/routing/session-key.ts` - Hierarchical key parsing

#### Acceptance Criteria

- [ ] SQLite session storage implemented
- [ ] Session metadata table for routing
- [ ] Automatic session persistence on changes
- [ ] Session loading from SQLite on startup
- [ ] Simplified context mode (remove enterprise features)
- [ ] Conversation history persistence (optional)
- [ ] Crash recovery (sessions survive restart)
- [ ] Backward compatibility with existing sessions
- [ ] Performance: <10ms for session load/save
- [ ] Cleanup: Sessions older than N days auto-pruned

---

### 4. Skill System Improvements

**Status:** Under consideration
**Priority:** Medium
**Complexity:** Medium

#### Description

Enhance Manta's skill system with registry support, better organization, and improved developer experience while maintaining OpenClaw compatibility.

#### Current State

Manta has:
- 13 built-in skills embedded in `builtin.rs` as Rust constants
- OpenClaw-compatible `SKILL.md` format with YAML frontmatter
- Hot reloading with file watcher
- Security scanning for suspicious patterns
- 4-level storage (bundled, user, workspace, project)

OpenClaw has:
- 54+ skills as separate files in `/skills/` directory
- Same `SKILL.md` format
- No security scanning
- Easier community contributions

#### Recommended Design

##### Phase 1: Convert Built-in Skills to SKILL.md Files

Move skills from Rust code to embeddable files:

```
src/skills/builtin/
├── github/
│   └── SKILL.md
├── weather/
│   └── SKILL.md
├── cron/
│   └── SKILL.md
└── ...
```

Load at compile time:
```rust
// Instead of const strings in builtin.rs
macro_rules! include_builtin_skills {
    () => {{
        let mut skills = HashMap::new();
        skills.insert(
            "github",
            include_str!("builtin/github/SKILL.md")
        );
        skills.insert(
            "weather",
            include_str!("builtin/weather/SKILL.md")
        );
        // ...
        skills
    }};
}}
```

**Benefits:**
- Easier to edit (no recompile during development)
- Skills can be overridden by user skills
- Cleaner separation of code and content
- Version history per skill

##### Phase 2: Skill Registry

Create a discoverable skill registry:

```rust
pub struct SkillRegistry {
    url: String,  // e.g., "https://skills.manta.dev"
    cache_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct SkillListing {
    pub name: String,
    pub description: String,
    pub author: String,
    pub version: String,
    pub downloads: u64,
    pub rating: f32,
    pub categories: Vec<String>,
    pub tags: Vec<String>,
}

impl SkillRegistry {
    /// Search for skills
    pub async fn search(&self, query: &str) -> Result<Vec<SkillListing>> {
        let url = format!("{}/api/v1/skills/search?q={}", self.url, query);
        let response = reqwest::get(&url).await?;
        Ok(response.json().await?)
    }

    /// Install skill from registry
    pub async fn install(&self, name: &str) -> Result<()> {
        let url = format!("{}/api/v1/skills/{}/download", self.url, name);
        let response = reqwest::get(&url).await?;
        let content = response.bytes().await?;

        // Extract to user skills directory
        let skill_dir = dirs::skill_dir().join(name);
        tokio::fs::create_dir_all(&skill_dir).await?;
        tokio::fs::write(skill_dir.join("SKILL.md"), content).await?;

        Ok(())
    }

    /// List popular skills
    pub async fn list_popular(&self) -> Result<Vec<SkillListing>> {
        let url = format!("{}/api/v1/skills/popular", self.url);
        let response = reqwest::get(&url).await?;
        Ok(response.json().await?)
    }

    /// Check for updates
    pub async fn check_updates(&self) -> Result<Vec<SkillUpdate>> {
        // Compare local versions with registry
    }
}
```

**CLI commands:**
```bash
manta skill search github           # Search registry
manta skill install github          # Install from registry
manta skill list --remote           # Show available skills
manta skill list --category dev     # Filter by category
manta skill list --tag git          # Filter by tag
manta skill info github             # Show skill details
manta skill update github           # Update to latest
manta skill update --all            # Update all skills
manta skill outdated                # Show outdated skills
```

##### Phase 3: Skill Categories and Tags

Add metadata for better organization:

```yaml
---
name: github
description: "GitHub operations via gh CLI"
metadata:
  openclaw:
    emoji: "🐙"
    category: "dev-tools"           # NEW
    tags:                           # NEW
      - "git"
      - "github"
      - "version-control"
      - "ci-cd"
    author: "manta-team"           # NEW
    version: "1.2.0"               # NEW
    updated_at: "2024-03-15"       # NEW
    requires:
      bins: ["gh"]
---
```

**Categories:**
- `dev-tools` - Development tools
- `productivity` - Task management, notes
- `communication` - Messaging, email
- `media` - Images, video, audio
- `system` - System administration
- `data` - Databases, analytics
- `ai` - AI/ML specific tools

**Benefits:**
- Better discoverability
- Thematic grouping
- Filtered listings

##### Phase 4: Skill Dependencies

Allow skills to depend on other skills:

```yaml
metadata:
  openclaw:
    requires:
      bins: ["gh"]
    skills:                           # NEW
      - name: "git-basics"
        version: ">=1.0.0"
        optional: false
      - name: "shell-helpers"
        optional: true
```

**Resolution:**
```rust
pub async fn resolve_dependencies(
    &self,
    skill_name: &str
) -> Result<Vec<Skill>> {
    let skill = self.get_skill(skill_name).await?;
    let mut resolved = Vec::new();

    for dep in &skill.metadata.openclaw.skills {
        if self.is_installed(&dep.name).await {
            resolved.push(self.get_skill(&dep.name).await?);
        } else if dep.optional {
            warn!("Optional dependency '{}' not installed", dep.name);
        } else {
            return Err(Error::MissingDependency(dep.name.clone()));
        }
    }

    Ok(resolved)
}
```

##### Phase 5: Skill Testing Framework

Add built-in testing for skills:

```bash
manta skill test github             # Test specific skill
manta skill test --all              # Test all skills
manta skill test --eligibility      # Check eligibility only
manta skill test --triggers         # Test trigger matching
```

**Implementation:**
```rust
pub struct SkillTester;

impl SkillTester {
    pub async fn test_skill(skill: &Skill) -> TestReport {
        TestReport {
            name: skill.name.clone(),
            eligibility: Self::test_eligibility(skill),
            triggers: Self::test_triggers(skill),
            install: Self::test_install(skill).await,
            security: Self::test_security(skill),
        }
    }

    fn test_eligibility(skill: &Skill) -> EligibilityResult {
        let mut errors = Vec::new();

        // Check binaries
        for bin in &skill.metadata.openclaw.requires.bins {
            if !is_binary_available(bin) {
                errors.push(format!("Binary '{}' not found", bin));
            }
        }

        // Check environment variables
        for env in &skill.metadata.openclaw.requires.env {
            if std::env::var(env).is_err() {
                errors.push(format!("Environment variable '{}' not set", env));
            }
        }

        EligibilityResult {
            passed: errors.is_empty(),
            errors,
        }
    }

    fn test_triggers(skill: &Skill) -> Vec<TriggerTest> {
        skill.triggers.iter().map(|t| {
            TriggerTest {
                pattern: t.pattern.clone(),
                test_cases: generate_test_cases(t),
                passed: true,
            }
        }).collect()
    }

    async fn test_install(skill: &Skill) -> InstallResult {
        // Test installation specs without actually installing
        for spec in &skill.metadata.openclaw.install {
            if !spec.is_available().await {
                return InstallResult::Unavailable(spec.clone());
            }
        }
        InstallResult::Available
    }

    fn test_security(skill: &Skill) -> SecurityReport {
        guard::scan_skill(skill)
    }
}
```

##### Phase 6: Enhanced Security Scanning

Manta already has security scanning. Enhance it:

```rust
const SUSPICIOUS_PATTERNS: &[(&str, &str, Severity)] = &[
    // Existing patterns
    ("system_prompt_injection", r"(?i)(system|assistant)\s*:\s*", Severity::High),
    ("command_injection", r"(?i)(;|\|\||&&|`)", Severity::High),

    // NEW: Additional patterns
    ("data_exfiltration", r"(?i)(curl|wget|http).*(password|secret|key)", Severity::Critical),
    ("privilege_escalation", r"(?i)(sudo|doas|pkexec)", Severity::Medium),
    ("file_deletion", r"(?i)rm\s+-rf", Severity::High),
    ("network_call", r"(?i)(curl|wget|fetch).*https?://", Severity::Low),
    ("code_execution", r"(?i)(eval|exec|system)\s*\(", Severity::Critical),
    ("path_traversal", r"(?i)\.\./|\.\.\\", Severity::Medium),
];

pub fn scan_skill(skill: &Skill) -> SecurityReport {
    let mut issues = Vec::new();

    // Check prompt content
    for (name, pattern, severity) in SUSPICIOUS_PATTERNS {
        if let Ok(re) = regex::Regex::new(pattern) {
            if re.is_match(&skill.prompt) {
                issues.push(SecurityIssue {
                    issue_type: name.to_string(),
                    description: format!("Found potentially dangerous pattern: {}", name),
                    severity: *severity,
                });
            }
        }
    }

    // NEW: Check for excessive length (possible DoS)
    if skill.prompt.len() > 500_000 {
        issues.push(SecurityIssue {
            issue_type: "excessive_length".to_string(),
            description: "Skill prompt exceeds 500KB".to_string(),
            severity: Severity::Medium,
        });
    }

    // NEW: Check for encoded/obfuscated content
    if is_obfuscated(&skill.prompt) {
        issues.push(SecurityIssue {
            issue_type: "obfuscated_content".to_string(),
            description: "Skill appears to contain obfuscated content".to_string(),
            severity: Severity::High,
        });
    }

    SecurityReport {
        passed: !issues.iter().any(|i| i.severity == Severity::Critical),
        issues,
    }
}
```

#### Configuration

```toml
[skills]
registry_url = "https://skills.manta.dev"
auto_update = false
security_scan = true

[skills.builtin]
enabled = true
allow_override = true

[skills.registry]
cache_ttl = "1h"
show_previews = true
```

#### Implementation Notes

**Rust crates to consider:**
- `reqwest` for registry HTTP requests
- `serde_yaml` for SKILL.md parsing (already used)
- `notify` for file watching (already used)
- `minijinja` for skill templating (optional)

**Key design decisions:**
- Keep OpenClaw `SKILL.md` format for compatibility
- Skills are loaded at startup, cached in memory
- Built-in skills can be overridden by user skills
- Security scanning happens on load and periodically
- Registry is optional (offline mode works)

**Migration path:**
1. Create `src/skills/builtin/` directory
2. Move each built-in skill to its own file
3. Update build script to embed files
4. Remove old `builtin.rs` constants
5. Add registry client
6. Add CLI commands

#### Comparison with OpenClaw

| Feature | OpenClaw | **Recommended Manta** |
|---------|----------|----------------------|
| Built-in skills | External files | **External files + embed** |
| Skill registry | Git repo only | **Web registry + search** |
| Security scanning | ❌ None | **✅ Pattern matching** |
| Categories | ❌ None | **✅ Categories + tags** |
| Dependencies | ❌ None | **✅ Skill dependencies** |
| Testing | ❌ Manual | **✅ Built-in tester** |
| Updates | Git pull | **✅ Registry updates** |
| Override built-in | ✅ Yes | ✅ Yes |
| Hot reload | ✅ Yes | ✅ Yes |

#### Reference

See OpenClaw's implementation:
- `/skills/` - 54 bundled skills as examples
- `src/skills/` - Skill loading and management

See Manta's current implementation:
- `src/skills/mod.rs` - Core skill types and manager
- `src/skills/builtin.rs` - Built-in skill definitions
- `src/skills/watcher.rs` - Hot reload file watcher
- `src/skills/guard.rs` - Security scanning

#### Acceptance Criteria

- [ ] Built-in skills moved to SKILL.md files
- [ ] Skill registry client implemented
- [ ] `manta skill search` command working
- [ ] `manta skill install` command working
- [ ] Category and tag metadata supported
- [ ] Skill dependencies resolved
- [ ] `manta skill test` command working
- [ ] Enhanced security scanning
- [ ] Backward compatibility maintained
- [ ] Offline mode works (no registry)

---

### 5. Channel System Improvements

**Status:** Under consideration
**Priority:** High
**Complexity:** Medium

#### Description

Enhance Manta's channel system with reliability features (auto-restart, health monitoring, state persistence) while keeping the simple trait-based design. Avoid OpenClaw's over-engineered adapter pattern.

#### Current State

Manta has:
- Simple `Channel` trait with 7 methods
- 7 channel implementations (Telegram, Discord, Slack, WhatsApp, QQ, Feishu/Lark, WebSocket)
- Feature-flagged compilation
- Basic `ChannelRegistry` for management
- Manual start/stop lifecycle

OpenClaw has:
- Complex plugin adapter pattern (20+ adapters per channel)
- 9+ channel implementations
- Auto-restart with exponential backoff
- Health monitoring with staleness detection
- State persistence (update offsets)
- Runtime plugin loading

#### Recommended Design

##### Phase 1: Auto-Restart with Backoff

Add lifecycle manager with automatic restart:

```rust
pub struct ChannelLifecycle {
    channel: Arc<dyn Channel>,
    policy: RestartPolicy,
    state: RwLock<LifecycleState>,
}

pub struct RestartPolicy {
    pub max_restarts: u32,      // default: 10
    pub initial_delay: Duration, // default: 5s
    pub max_delay: Duration,     // default: 5min
    pub backoff_factor: f32,     // default: 2.0
    pub reset_after: Duration,   // default: 5min (reset counter after success)
}

pub struct LifecycleState {
    pub restart_count: u32,
    pub last_restart: Option<Instant>,
    pub last_success: Option<Instant>,
    pub status: ChannelStatus,
}

pub enum ChannelStatus {
    Starting,
    Running,
    Stopping,
    Stopped,
    Crashed,
    Restarting,
}

impl ChannelLifecycle {
    pub async fn start_managed(&self) -> Result<()> {
        loop {
            // Update status
            self.set_status(ChannelStatus::Starting).await;

            match self.channel.start().await {
                Ok(()) => {
                    self.set_status(ChannelStatus::Running).await;
                    self.record_success().await;

                    // Wait for channel to stop (or crash)
                    self.wait_for_stop().await;

                    // Check if we should restart
                    if !self.should_restart() {
                        break;
                    }

                    let delay = self.calculate_backoff();
                    self.set_status(ChannelStatus::Restarting).await;
                    tokio::time::sleep(delay).await;
                }
                Err(e) => {
                    error!("Channel failed to start: {}", e);
                    self.set_status(ChannelStatus::Crashed).await;

                    if !self.should_restart() {
                        return Err(e);
                    }

                    let delay = self.calculate_backoff();
                    tokio::time::sleep(delay).await;
                }
            }
        }
        Ok(())
    }

    fn calculate_backoff(&self) -> Duration {
        let state = self.state.read().blocking_lock();
        let attempts = state.restart_count.min(5); // Cap at 5 for calculation

        let delay_ms = (self.policy.initial_delay.as_millis() as f32
            * self.policy.backoff_factor.powi(attempts as i32)) as u64;

        let delay = Duration::from_millis(delay_ms.min(self.policy.max_delay.as_millis() as u64));

        // Add jitter (±10%)
        let jitter = delay.as_millis() as f32 * 0.1;
        let jitter_ms = rand::random::<f32>() * jitter * 2.0 - jitter;

        delay + Duration::from_millis(jitter_ms as u64)
    }
}
```

**Usage in Gateway:**
```rust
pub async fn start_channel_managed(&self, name: &str) -> Result<()> {
    let channel = self.get_channel(name).await?;

    let lifecycle = ChannelLifecycle::new(
        channel,
        RestartPolicy::default(),
    );

    // Spawn managed channel
    tokio::spawn(async move {
        if let Err(e) = lifecycle.start_managed().await {
            error!("Channel {} failed permanently: {}", name, e);
        }
    });

    Ok(())
}
```

##### Phase 2: Health Monitoring

Add periodic health checks:

```rust
pub struct ChannelHealthMonitor {
    channels: Arc<RwLock<HashMap<String, ChannelHealth>>>,
    check_interval: Duration,
    stale_threshold: Duration,
}

pub struct ChannelHealth {
    pub channel_name: String,
    pub last_heartbeat: Instant,
    pub consecutive_failures: u32,
    pub status: HealthStatus,
    pub message_count: AtomicU64,
    pub last_message_at: Option<Instant>,
}

pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
    Stale,
}

impl ChannelHealthMonitor {
    pub async fn start_monitoring(&self) {
        let mut interval = tokio::time::interval(self.check_interval);

        loop {
            interval.tick().await;

            let channels = self.channels.read().await;
            for (name, health) in channels.iter() {
                match self.check_health(health).await {
                    HealthStatus::Stale => {
                        warn!("Channel {} is stale, triggering restart", name);
                        self.trigger_restart(name).await;
                    }
                    HealthStatus::Unhealthy => {
                        warn!("Channel {} is unhealthy", name);
                        self.alert_unhealthy(name).await;
                    }
                    _ => {}
                }
            }
        }
    }

    async fn check_health(&self, health: &ChannelHealth) -> HealthStatus {
        // Check if we received heartbeats
        let elapsed = health.last_heartbeat.elapsed();

        if elapsed > self.stale_threshold {
            return HealthStatus::Stale;
        }

        if elapsed > self.stale_threshold / 2 {
            return HealthStatus::Degraded;
        }

        if health.consecutive_failures > 3 {
            return HealthStatus::Unhealthy;
        }

        HealthStatus::Healthy
    }

    pub async fn record_heartbeat(&self, channel_name: &str) {
        let mut channels = self.channels.write().await;
        if let Some(health) = channels.get_mut(channel_name) {
            health.last_heartbeat = Instant::now();
            health.consecutive_failures = 0;
        }
    }

    pub async fn record_message(&self, channel_name: &str) {
        let mut channels = self.channels.write().await;
        if let Some(health) = channels.get_mut(channel_name) {
            health.message_count.fetch_add(1, Ordering::Relaxed);
            health.last_message_at = Some(Instant::now());
        }
    }
}
```

**Integration with Channel trait:**
```rust
#[async_trait]
pub trait Channel: Send + Sync {
    // Existing methods...

    /// Report health status (optional - default implementation provided)
    async fn health_check(&self) -> Result<HealthStatus> {
        Ok(HealthStatus::Healthy)
    }

    /// Get current state for persistence
    async fn get_state(&self) -> Option<ChannelState> {
        None // Default: no state to persist
    }

    /// Restore state on startup
    async fn restore_state(&self, state: ChannelState) -> Result<()> {
        Ok(()) // Default: no state to restore
    }
}
```

##### Phase 3: Channel State Persistence

Persist channel state (offsets, session mappings) to SQLite:

```rust
pub struct ChannelStateStore {
    db: SqlitePool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelState {
    pub channel_name: String,
    pub account_id: Option<String>,
    pub update_offset: Option<i64>,
    pub session_mappings: HashMap<String, String>,
    pub last_activity: DateTime<Utc>,
}

impl ChannelStateStore {
    pub async fn save_state(&self, state: &ChannelState) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO channel_states
             (channel_name, account_id, update_offset, session_mappings, last_activity)
             VALUES (?, ?, ?, ?, ?)"
        )
        .bind(&state.channel_name)
        .bind(&state.account_id)
        .bind(state.update_offset)
        .bind(serde_json::to_string(&state.session_mappings)?)
        .bind(state.last_activity)
        .execute(&self.db)
        .await?;

        Ok(())
    }

    pub async fn load_state(
        &self,
        channel_name: &str,
        account_id: Option<&str>
    ) -> Result<Option<ChannelState>> {
        let row = sqlx::query_as::
            <_, (String, Option<String>, Option<i64>, String, DateTime<Utc>)>(
            "SELECT channel_name, account_id, update_offset, session_mappings, last_activity
             FROM channel_states
             WHERE channel_name = ? AND (account_id = ? OR account_id IS NULL)"
        )
        .bind(channel_name)
        .bind(account_id)
        .fetch_optional(&self.db)
        .await?;

        match row {
            Some((name, account, offset, mappings, activity)) => {
                Ok(Some(ChannelState {
                    channel_name: name,
                    account_id: account,
                    update_offset: offset,
                    session_mappings: serde_json::from_str(&mappings)?,
                    last_activity: activity,
                }))
            }
            None => Ok(None),
        }
    }
}
```

**Usage in Telegram channel:**
```rust
impl TelegramChannel {
    async fn save_offset(&self, offset: i32) {
        let state = ChannelState {
            channel_name: "telegram".to_string(),
            account_id: Some(self.config.token.clone()),
            update_offset: Some(offset as i64),
            session_mappings: self.session_map.read().await.clone(),
            last_activity: Utc::now(),
        };

        if let Err(e) = self.state_store.save_state(&state).await {
            error!("Failed to save Telegram state: {}", e);
        }
    }

    async fn restore_offset(&self) -> Option<i32> {
        match self.state_store.load_state("telegram", Some(&self.config.token)).await {
            Ok(Some(state)) => {
                // Restore session mappings
                if let Ok(mut sessions) = self.session_map.write().await {
                    sessions.extend(state.session_mappings);
                }
                state.update_offset.map(|o| o as i32)
            }
            _ => None,
        }
    }
}
```

##### Phase 4: Graceful Shutdown

Improve shutdown handling:

```rust
pub struct GracefulShutdown {
    timeout: Duration,
    notify: Arc<Notify>,
}

impl GracefulShutdown {
    pub async fn shutdown(&self, channels: &ChannelRegistry) {
        // Signal all channels to stop
        self.notify.notify_waiters();

        // Wait for channels with timeout
        let shutdown_future = async {
            for channel in channels.iter() {
                if let Err(e) = channel.stop().await {
                    error!("Error stopping channel: {}", e);
                }
            }
        };

        match tokio::time::timeout(self.timeout, shutdown_future).await {
            Ok(()) => info!("All channels stopped gracefully"),
            Err(_) => warn!("Shutdown timed out after {:?}", self.timeout),
        }
    }
}
```

##### Phase 5: Channel Metrics

Add basic metrics collection:

```rust
pub struct ChannelMetrics {
    pub messages_received: AtomicU64,
    pub messages_sent: AtomicU64,
    pub errors: AtomicU64,
    pub latency_ms: RwLock<Vec<u64>>, // Last 100 latencies
    pub connected_at: Option<Instant>,
}

impl ChannelMetrics {
    pub fn record_receive(&self) {
        self.messages_received.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_send(&self, latency: Duration) {
        self.messages_sent.fetch_add(1, Ordering::Relaxed);

        let mut latencies = self.latency_ms.write().blocking_lock();
        latencies.push(latency.as_millis() as u64);
        if latencies.len() > 100 {
            latencies.remove(0);
        }
    }

    pub fn average_latency(&self) -> Option<Duration> {
        let latencies = self.latency_ms.read().blocking_lock();
        if latencies.is_empty() {
            return None;
        }
        let avg = latencies.iter().sum::<u64>() / latencies.len() as u64;
        Some(Duration::from_millis(avg))
    }
}
```

#### Database Schema

```sql
-- Channel states for persistence
CREATE TABLE channel_states (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    channel_name TEXT NOT NULL,
    account_id TEXT,
    update_offset INTEGER,
    session_mappings TEXT NOT NULL DEFAULT '{}',
    last_activity DATETIME NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(channel_name, account_id)
);

-- Channel health history
CREATE TABLE channel_health_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    channel_name TEXT NOT NULL,
    status TEXT NOT NULL,
    message TEXT,
    recorded_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- Channel metrics (optional, for analytics)
CREATE TABLE channel_metrics (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    channel_name TEXT NOT NULL,
    messages_received INTEGER DEFAULT 0,
    messages_sent INTEGER DEFAULT 0,
    errors INTEGER DEFAULT 0,
    recorded_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_channel_states_name ON channel_states(channel_name);
CREATE INDEX idx_channel_health_log_name ON channel_health_log(channel_name);
```

#### Implementation Notes

**Rust crates to consider:**
- `tokio::sync::Notify` for shutdown signaling (already used)
- `sqlx` for state persistence
- `rand` for jitter in backoff
- `metrics` crate for metrics export (optional)

**Key design decisions:**
- Keep simple `Channel` trait - don't add complexity
- Auto-restart is opt-in per channel
- State persistence is optional (channels can opt-out)
- Health monitoring runs in background task
- All timeouts are configurable

**Migration path:**
1. Add `ChannelLifecycle` struct
2. Add `ChannelStateStore` with SQLite
3. Update `ChannelRegistry` to use lifecycle
4. Add health monitor background task
5. Update each channel to use state store (gradually)

#### Comparison with OpenClaw

| Feature | OpenClaw | **Recommended Manta** |
|---------|----------|----------------------|
| Architecture | 20+ adapters per channel | **Simple trait (keep)** |
| Auto-restart | ✅ Exponential backoff | **✅ Add with policy** |
| Health monitoring | ✅ Stale detection | **✅ Add with heartbeats** |
| State persistence | ✅ Update offsets | **✅ Add SQLite store** |
| Graceful shutdown | ✅ AbortController | **✅ Add timeout** |
| Metrics | ❌ Limited | **✅ Add basic metrics** |
| Feature flags | ❌ Runtime loading | **✅ Keep compile-time** |
| Plugin system | ✅ Complex SDK | **❌ Keep simple - no SDK** |

#### What NOT to Adopt from OpenClaw

| OpenClaw Feature | Why Skip |
|------------------|----------|
| `ChannelDirectoryAdapter` | Overkill for personal use |
| `ChannelElevatedAdapter` | Not needed |
| `ChannelResolverAdapter` | Use simple session routing |
| `ChannelAgentToolFactory` | Tools are separate concern |
| Complex delivery modes | Simple `send()` is fine |
| Runtime plugin loading | Use compile-time features |
| 20+ adapter interfaces | Trait is sufficient |

#### Reference

See OpenClaw's implementation (for reference):
- `src/gateway/server-channels.ts` - Lifecycle manager
- `src/gateway/channel-health-monitor.ts` - Health checks
- `src/channels/plugins/types.plugin.ts` - Adapter interfaces
- `src/telegram/monitor.ts` - Telegram lifecycle
- `src/discord/monitor/provider.lifecycle.ts` - Discord lifecycle

See Manta's current implementation:
- `src/channels/mod.rs` - Channel trait and types
- `src/channels/telegram.rs` - Telegram implementation
- `src/channels/discord.rs` - Discord implementation
- `src/gateway/mod.rs` - Channel registry in gateway

#### Acceptance Criteria

- [ ] `ChannelLifecycle` with auto-restart implemented
- [ ] Exponential backoff with jitter working
- [ ] `ChannelHealthMonitor` with periodic checks
- [ ] Stale channel detection and restart
- [ ] `ChannelStateStore` with SQLite persistence
- [ ] Update offset restoration for Telegram
- [ ] Graceful shutdown with timeout
- [ ] Basic metrics collection (messages, latency)
- [ ] All channels use new lifecycle system
- [ ] Configuration for restart policies
- [ ] Backward compatibility maintained

---

### 6. Agent System Improvements

**Status:** Under consideration
**Priority:** Medium
**Complexity:** Medium

#### Description

Enhance Manta's agent system with structured subagent spawning and tool hooks while keeping the in-process simplicity. Avoid OpenClaw's ACP (external process) complexity.

#### Current State

Manta has:
- Single in-process `Agent` struct
- Direct async message processing
- Response caching with LLM cacheability check
- Task planner for auto-decomposition
- Dynamic prompt building
- Progress callbacks for real-time updates

OpenClaw has:
- Dual runtime: Embedded + ACP (external process)
- Complex subagent spawning with registry
- Tool hooks (before/after)
- Session-based context with lineage
- XML tag parsing for reasoning

#### Recommended Design

##### Phase 1: Tool Hooks

Add pre/post execution hooks for tools:

```rust
pub struct ToolHooks {
    before_call: Vec<Arc<dyn Fn(&str, &Value) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>>,
    after_call: Vec<Arc<dyn Fn(&str, &Value, &Value) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>>,
}

impl ToolRegistry {
    pub async fn execute_with_hooks(
        &self,
        name: &str,
        args: Value,
        context: ToolContext,
        hooks: &ToolHooks,
    ) -> Result<Value> {
        // Run before hooks
        for hook in &hooks.before_call {
            hook(name, &args).await;
        }

        // Execute
        let result = self.execute(name, args.clone(), context).await;

        // Run after hooks
        for hook in &hooks.after_call {
            hook(name, &args, &result).await;
        }

        result
    }
}

// Usage example
let hooks = ToolHooks::new()
    .before("shell", |name, args| {
        println!("About to execute shell command: {:?}", args);
    })
    .after("shell", |name, args, result| {
        println!("Shell command completed with result: {:?}", result);
    });

let result = registry.execute_with_hooks("shell", args, context, &hooks).await?;
```

**Use cases:**
- Audit logging
- Result caching
- Permission checks
- Metrics collection

##### Phase 2: Subagent Registry

Add structured subagent spawning like OpenClaw:

```rust
pub struct SubagentRegistry {
    runs: RwLock<HashMap<String, SubagentRun>>,
    max_depth: u32,
    max_concurrent: usize,
}

pub struct SubagentRun {
    pub run_id: String,
    pub parent_session: String,
    pub child_session: String,
    pub target_agent: String,
    pub status: SubagentStatus,
    pub spawn_depth: u32,
    pub started_at: Instant,
    pub completed_at: Option<Instant>,
    pub output: Option<String>,
}

pub enum SubagentStatus {
    Running,
    Completed(String),
    Failed(String),
    Killed,
}

impl SubagentRegistry {
    /// Spawn a subagent to handle a task
    pub async fn spawn(
        &self,
        parent_session: &str,
        target_agent: &str,
        task: &str,
    ) -> Result<String> {
        // Check depth limit
        let current_depth = self.get_depth(parent_session).await;
        if current_depth >= self.max_depth {
            return Err(Error::MaxSpawnDepth(self.max_depth));
        }

        // Check concurrent limit
        let active_count = self.active_count().await;
        if active_count >= self.max_concurrent {
            return Err(Error::MaxConcurrentSubagents(self.max_concurrent));
        }

        // Create child session
        let child_session = format!("{}:subagent:{}", parent_session, Uuid::new_v4());

        // Register run
        let run_id = Uuid::new_v4().to_string();
        let run = SubagentRun {
            run_id: run_id.clone(),
            parent_session: parent_session.to_string(),
            child_session: child_session.clone(),
            target_agent: target_agent.to_string(),
            status: SubagentStatus::Running,
            spawn_depth: current_depth + 1,
            started_at: Instant::now(),
            completed_at: None,
            output: None,
        };

        self.runs.write().await.insert(run_id.clone(), run);

        // Spawn subagent task
        let registry = self.clone();
        tokio::spawn(async move {
            let result = registry.run_subagent(&run_id, target_agent, task).await;
            registry.complete_run(&run_id, result).await;
        });

        Ok(run_id)
    }

    /// Wait for subagent completion
    pub async fn wait_for_completion(&self, run_id: &str, timeout: Duration) -> Result<String> {
        let start = Instant::now();
        loop {
            if start.elapsed() > timeout {
                return Err(Error::SubagentTimeout);
            }

            let runs = self.runs.read().await;
            if let Some(run) = runs.get(run_id) {
                match &run.status {
                    SubagentStatus::Completed(output) => return Ok(output.clone()),
                    SubagentStatus::Failed(error) => return Err(Error::SubagentFailed(error.clone())),
                    SubagentStatus::Killed => return Err(Error::SubagentKilled),
                    SubagentStatus::Running => {
                        // Continue waiting
                    }
                }
            } else {
                return Err(Error::SubagentNotFound);
            }
            drop(runs);

            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Kill a running subagent
    pub async fn kill(&self, run_id: &str) -> Result<()> {
        let mut runs = self.runs.write().await;
        if let Some(run) = runs.get_mut(run_id) {
            run.status = SubagentStatus::Killed;
            run.completed_at = Some(Instant::now());
            Ok(())
        } else {
            Err(Error::SubagentNotFound)
        }
    }

    /// Get spawn depth for a session
    async fn get_depth(&self, session: &str) -> u32 {
        let runs = self.runs.read().await;
        runs.values()
            .find(|r| r.child_session == session)
            .map(|r| r.spawn_depth)
            .unwrap_or(0)
    }
}
```

**Integration with tools:**
```rust
// delegate_tool.rs - Updated to use registry
pub struct DelegateTool {
    subagent_registry: Arc<SubagentRegistry>,
}

#[async_trait]
impl Tool for DelegateTool {
    async fn execute(&self, args: Value, context: ToolContext) -> Result<Value> {
        let target_agent = args.get("agent").as_str().ok_or(Error::MissingArgument)?;
        let task = args.get("task").as_str().ok_or(Error::MissingArgument)?;

        let run_id = self.subagent_registry.spawn(
            &context.conversation_id,
            target_agent,
            task,
        ).await?;

        // Wait for completion with timeout
        let timeout = Duration::from_secs(args.get("timeout").as_u64().unwrap_or(300));
        let result = self.subagent_registry.wait_for_completion(&run_id, timeout).await?;

        Ok(json!({"result": result}))
    }
}
```

##### Phase 3: Tool Sandboxing

Add optional sandboxing for tools:

```rust
pub struct SandboxConfig {
    pub allow_file_access: bool,
    pub allow_network_access: bool,
    pub allowed_paths: Vec<PathBuf>,
    pub blocked_paths: Vec<PathBuf>,
    pub timeout: Duration,
    pub max_memory_mb: usize,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            allow_file_access: true,
            allow_network_access: false,
            allowed_paths: vec![],
            blocked_paths: vec![],
            timeout: Duration::from_secs(60),
            max_memory_mb: 512,
        }
    }
}

pub struct SandboxedTool {
    inner: Arc<dyn Tool>,
    sandbox: SandboxConfig,
}

#[async_trait]
impl Tool for SandboxedTool {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn schema(&self) -> Value {
        self.inner.schema()
    }

    async fn execute(&self, args: Value, context: ToolContext) -> Result<Value> {
        // Check file access permissions
        if let Some(path) = args.get("path").and_then(|p| p.as_str()) {
            if !self.sandbox.allow_file_access {
                return Err(Error::SandboxViolation("File access not allowed".to_string()));
            }

            let path = Path::new(path);

            // Check blocked paths
            for blocked in &self.sandbox.blocked_paths {
                if path.starts_with(blocked) {
                    return Err(Error::SandboxViolation(
                        format!("Access to {:?} is blocked", path)
                    ));
                }
            }

            // Check allowed paths (if specified)
            if !self.sandbox.allowed_paths.is_empty() {
                let allowed = self.sandbox.allowed_paths.iter()
                    .any(|allowed| path.starts_with(allowed));
                if !allowed {
                    return Err(Error::SandboxViolation(
                        format!("Access to {:?} not in allowed paths", path)
                    ));
                }
            }
        }

        // Apply timeout
        let result = tokio::time::timeout(
            self.sandbox.timeout,
            self.inner.execute(args, context)
        ).await;

        match result {
            Ok(r) => r,
            Err(_) => Err(Error::SandboxViolation("Tool execution timed out".to_string())),
        }
    }
}

// Usage
let sandboxed_shell = SandboxedTool::new(
    shell_tool,
    SandboxConfig {
        allow_file_access: true,
        allowed_paths: vec![
            PathBuf::from("/home/user/projects"),
            PathBuf::from("/tmp"),
        ],
        blocked_paths: vec![
            PathBuf::from("/etc/passwd"),
            PathBuf::from("/home/user/.ssh"),
        ],
        timeout: Duration::from_secs(30),
        ..Default::default()
    },
);
```

##### Phase 4: Agent Actor Model (Optional)

For better concurrency control, consider an actor model:

```rust
pub struct AgentActor {
    agent: Agent,
    mailbox: mpsc::Receiver<AgentMessage>,
    active_runs: HashMap<String, JoinHandle<Result<OutgoingMessage>>>,
}

pub enum AgentMessage {
    ProcessMessage {
        message: IncomingMessage,
        respond_to: oneshot::Sender<Result<OutgoingMessage>>,
    },
    GetStatus {
        respond_to: oneshot::Sender<AgentStatus>,
    },
    CancelRun {
        run_id: String,
    },
    Shutdown,
}

impl AgentActor {
    pub fn spawn(agent: Agent) -> mpsc::Sender<AgentMessage> {
        let (tx, rx) = mpsc::channel(100);
        let actor = Self {
            agent,
            mailbox: rx,
            active_runs: HashMap::new(),
        };

        tokio::spawn(actor.run());
        tx
    }

    async fn run(mut self) {
        while let Some(msg) = self.mailbox.recv().await {
            match msg {
                AgentMessage::ProcessMessage { message, respond_to } => {
                    let agent = self.agent.clone();
                    let handle = tokio::spawn(async move {
                        agent.process_message(message).await
                    });

                    // Store handle for possible cancellation
                    let run_id = Uuid::new_v4().to_string();
                    self.active_runs.insert(run_id.clone(), handle);

                    // Wait for completion and respond
                    tokio::spawn(async move {
                        // ... handle completion
                    });
                }
                AgentMessage::Shutdown => break,
                _ => {}
            }
        }
    }
}
```

#### What NOT to Adopt from OpenClaw

| OpenClaw Feature | Why Skip |
|------------------|----------|
| ACP (external process) | Overkill for personal use, adds complexity |
| XML tag parsing | Use structured JSON/tool calls instead |
| Complex session lineage | Simple parent-child is sufficient |
| Lane-based routing | Direct method calls are clearer |
| 20+ session metadata fields | Keep minimal, extensible struct |

#### Comparison with OpenClaw

| Feature | OpenClaw | **Recommended Manta** |
|---------|----------|----------------------|
| Runtime | Dual (Embedded + ACP) | **Single in-process** |
| Subagent spawning | ✅ Registry with depth limits | **✅ Add registry** |
| Tool hooks | ✅ before/after | **✅ Add hooks** |
| Sandboxing | ✅ Configurable | **✅ Add sandboxing** |
| Response caching | ❌ Not mentioned | **✅ Keep (advantage)** |
| Task planning | ❌ Not mentioned | **✅ Keep (advantage)** |
| Progress callbacks | ❌ Event streaming only | **✅ Keep (advantage)** |
| Actor model | ❌ Not used | **⚠️ Optional** |

#### Implementation Notes

**Rust crates to consider:**
- `tokio::sync::mpsc` for actor mailboxes
- `tokio::sync::oneshot` for request-response
- `tokio::time::timeout` for sandboxing

**Key design decisions:**
- Keep in-process model (don't add ACP)
- Subagent registry is optional component
- Tool hooks are opt-in per execution
- Sandboxing is opt-in per tool
- Maintain backward compatibility

#### Reference

See OpenClaw's implementation (for reference):
- `src/agents/subagent-spawn.ts` - Subagent spawning
- `src/agents/subagent-registry.ts` - Registry management
- `src/agents/acp-spawn.ts` - ACP spawning (don't copy)
- `src/agents/pi-embedded-subscribe.handlers.tools.ts` - Tool hooks

See Manta's current implementation:
- `src/agent/mod.rs` - Main Agent struct
- `src/agent/context.rs` - Context management
- `src/tools/mod.rs` - Tool registry
- `src/tools/delegate_tool.rs` - Subagent delegation

#### Acceptance Criteria

- [ ] `ToolHooks` with before/after execution
- [ ] `SubagentRegistry` with spawn depth limits
- [ ] `SubagentRun` lifecycle tracking
- [ ] `wait_for_completion` with timeout
- [ ] `SandboxedTool` with path restrictions
- [ ] Tool timeout enforcement
- [ ] Integration with delegate_tool
- [ ] Proper cleanup on shutdown
- [ ] Metrics for subagent runs
- [ ] Backward compatibility maintained

---

### 7. Memory System Improvements (OpenClaw Diff)

**Status:** Under consideration
**Priority:** See per-item priorities below
**Complexity:** Low–Medium per item

#### Background

A detailed diff of Manta's memory implementation against OpenClaw's revealed the following gaps. Items are ordered by impact.

---

#### 8.1 Raise Personality File Size Cap

**Priority:** High | **Complexity:** Low

**Current state:** `DEFAULT_MAX_FILE_SIZE = 4096` bytes (~1 page of text) in `src/memory/personality.rs`. Files are hard-truncated at that limit.

**OpenClaw:** 20,000 chars/file, 150,000 chars total. Files over the limit keep the first 70% + last 20% with a truncation marker between them.

**Proposed change:**

```rust
// src/memory/personality.rs
const DEFAULT_MAX_FILE_SIZE: usize = 20_000;  // raise from 4096
const DEFAULT_TOTAL_MAX_SIZE: usize = 150_000;

fn truncate_with_head_tail(content: &str, max_chars: usize) -> String {
    if content.len() <= max_chars {
        return content.to_string();
    }
    let head = (max_chars as f32 * 0.70) as usize;
    let tail = (max_chars as f32 * 0.20) as usize;
    format!(
        "{}\n\n[... {} chars truncated ...]\n\n{}",
        &content[..head],
        content.len() - head - tail,
        &content[content.len() - tail..]
    )
}
```

Add a total budget check in `format_for_prompt()` to enforce `DEFAULT_TOTAL_MAX_SIZE` across all files combined.

#### Acceptance Criteria

- [ ] Per-file cap raised to 20,000 chars (configurable)
- [ ] Total cap of 150,000 chars across all files
- [ ] Head/tail truncation preserves beginning and end
- [ ] Truncation marker inserted between head and tail
- [ ] Existing tests updated

---

#### 8.2 Add `memory/*.md` Glob Support

**Priority:** High | **Complexity:** Low

**Current state:** `PersonalityMemory` only reads named files (SOUL.md, IDENTITY.md, etc.). There is no support for dated memory fragments.

**OpenClaw:** Loads `memory/YYYY-MM-DD.md` and any `memory/*.md` files from the workspace directory, injecting them into the system prompt alongside MEMORY.md.

**Proposed change:**

In `src/memory/personality.rs`, add a `load_memory_fragments()` method:

```rust
pub async fn load_memory_fragments(&self) -> Vec<(String, String)> {
    let memory_dir = self.base_dir.join("memory");
    if !memory_dir.exists() {
        return vec![];
    }

    let mut entries = match fs::read_dir(&memory_dir).await {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    let mut fragments = vec![];
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("md") {
            if let Ok(content) = fs::read_to_string(&path).await {
                let name = path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("memory")
                    .to_string();
                fragments.push((name, content));
            }
        }
    }

    // Sort chronologically (dated files sort naturally)
    fragments.sort_by(|a, b| a.0.cmp(&b.0));
    fragments
}
```

Update `format_for_prompt()` to include these fragments under `### Memory Fragments`.

Also create `~/.manta/workspace/memory/` directory in `initialize_defaults()`.

#### Acceptance Criteria

- [ ] `memory/*.md` files loaded from workspace memory dir
- [ ] Fragments appended to system prompt under `### Memory Fragments`
- [ ] Files sorted chronologically
- [ ] Individual fragment size capped at per-file limit
- [ ] `initialize_defaults()` creates `memory/` subdirectory

---

#### 8.3 Hybrid Search (Vector + FTS5)

**Priority:** High | **Complexity:** Medium

**Current state:** `VectorMemoryService` (vector) and `SessionSearch` (FTS5) exist independently. There is no combined search.

**OpenClaw:** Hybrid search merging cosine similarity (weight 0.7) and BM25 FTS5 (weight 0.3), with configurable weights.

**Proposed change:**

Add `HybridSearch` in `src/memory/` combining both backends:

```rust
pub struct HybridSearchConfig {
    pub vector_weight: f32,   // default: 0.7
    pub text_weight: f32,     // default: 0.3
    pub max_results: usize,   // default: 6
    pub min_score: f32,       // default: 0.35
}

impl Default for HybridSearchConfig {
    fn default() -> Self {
        Self { vector_weight: 0.7, text_weight: 0.3, max_results: 6, min_score: 0.35 }
    }
}

pub struct HybridSearchResult {
    pub content: String,
    pub score: f32,
    pub source: String,   // "memory" or "sessions"
    pub citation: String, // e.g. "MEMORY.md#L5-L12"
}

pub async fn hybrid_search(
    query: &str,
    vector_service: &VectorMemoryService,
    session_search: &SessionSearch,
    config: &HybridSearchConfig,
) -> Vec<HybridSearchResult> {
    // Run both searches concurrently
    let (vector_results, fts_results) = tokio::join!(
        vector_service.search(query, config.max_results * 2),
        session_search.search(SessionSearchQuery::new(query).limit(config.max_results * 2)),
    );

    // Normalize scores and merge by deduplicating on content hash
    merge_hybrid_results(vector_results, fts_results, config)
}
```

Wire `hybrid_search` into `MemoryTool` and expose via the `memory_search` tool action.

#### Acceptance Criteria

- [ ] `HybridSearchConfig` with configurable vector/text weights
- [ ] Vector and FTS5 searches run concurrently
- [ ] Results merged and deduplicated by content hash
- [ ] Scores normalized to 0–1 range before merging
- [ ] `min_score` threshold filters low-quality results
- [ ] Used by `MemoryTool` when both backends are available
- [ ] Config keys added to `manta.toml` schema

---

#### 8.4 History Limiting by Turn Count

**Priority:** High | **Complexity:** Low

**Current state:** `Context::prune_if_needed()` prunes only by token estimate. No per-channel or per-user configurable turn-count limit exists.

**OpenClaw:** `limitHistoryTurns(messages, N)` with per-channel (`historyLimit`) and per-DM (`dmHistoryLimit`) config overrides.

**Proposed change:**

Add `max_turns: Option<usize>` to `AgentConfig` in `src/agent/mod.rs`:

```rust
pub struct AgentConfig {
    // ... existing fields ...
    /// Maximum number of user turns to retain in context window.
    /// When exceeded, oldest turns are dropped before token pruning.
    pub max_turns: Option<usize>,
}
```

In `Context::add_message()`, after appending the message, apply turn limiting:

```rust
fn limit_turns(&mut self, max_turns: usize) {
    // Count user-role messages
    let user_turn_indices: Vec<usize> = self.messages.iter()
        .enumerate()
        .filter(|(_, m)| m.role == Role::User)
        .map(|(i, _)| i)
        .collect();

    if user_turn_indices.len() > max_turns {
        let drop_before = user_turn_indices[user_turn_indices.len() - max_turns];
        self.messages.drain(..drop_before);
        // Recalculate token_count after drain
        self.token_count = self.messages.iter().map(|m| m.content.len() / 4).sum();
    }
}
```

Add `dm_history_limit` and `channel_history_limit` to channel configs.

#### Acceptance Criteria

- [ ] `max_turns: Option<usize>` added to `AgentConfig`
- [ ] `limit_turns()` applied after each `add_message()`
- [ ] Tool call pairs protected from mid-pair truncation
- [ ] `dm_history_limit`/`channel_history_limit` in channel config
- [ ] Default: no limit (backward compatible)

---

#### 8.5 Workspace File Cache

**Priority:** Medium | **Complexity:** Low

**Current state:** `PersonalityMemory::read()` calls `fs::read_to_string()` on every invocation — a disk read per personality file per turn.

**OpenClaw:** `workspaceFileCache` — a `Map<path, {content, identity}>` keyed by file path, invalidated when inode/mtime/size changes.

**Proposed change:**

Add an in-process cache to `PersonalityMemory`:

```rust
#[derive(Clone)]
struct CachedFile {
    content: String,
    mtime: SystemTime,
    size: u64,
}

pub struct PersonalityMemory {
    base_dir: PathBuf,
    max_size: usize,
    cache: Arc<RwLock<HashMap<PathBuf, CachedFile>>>,
}

async fn read_with_cache(&self, path: &Path) -> String {
    // Check cache validity
    if let Ok(meta) = fs::metadata(path).await {
        let cache = self.cache.read().await;
        if let Some(cached) = cache.get(path) {
            if cached.mtime == meta.modified().unwrap_or(SystemTime::UNIX_EPOCH)
                && cached.size == meta.len()
            {
                return cached.content.clone();
            }
        }
    }

    // Cache miss or stale — re-read
    let content = fs::read_to_string(path).await.unwrap_or_default();
    let meta = fs::metadata(path).await.ok();
    let mut cache = self.cache.write().await;
    cache.insert(path.to_path_buf(), CachedFile {
        content: content.clone(),
        mtime: meta.as_ref().and_then(|m| m.modified().ok()).unwrap_or(SystemTime::UNIX_EPOCH),
        size: meta.map(|m| m.len()).unwrap_or(0),
    });
    content
}
```

#### Acceptance Criteria

- [ ] File cache added to `PersonalityMemory`
- [ ] Cache invalidated on mtime or size change
- [ ] Cache is per-`PersonalityMemory` instance (no global state)
- [ ] Thread-safe via `Arc<RwLock<>>`
- [ ] Existing tests pass

---

#### 8.6 LLM-Generated Compaction

**Priority:** Medium | **Complexity:** Medium

**Current state:** The `Summarize` compression strategy in `ContextCompressor` keeps first 2 + last 4 messages and inserts `[N earlier messages omitted]` — not an actual LLM-generated summary.

**OpenClaw:** `session.compact()` calls an LLM (optionally a cheaper model) to generate a prose summary of dropped messages, which is injected as a system message.

**Proposed change:**

Add a `compact_with_llm()` method to `ContextCompressor`:

```rust
pub async fn compact_with_llm(
    &self,
    context: &mut Context,
    provider: Arc<dyn Provider>,
    model: Option<&str>,  // e.g. a cheaper/faster alias
) -> Result<CompressionStats> {
    let before_count = context.messages.len();
    let before_tokens = context.token_count;

    // Identify messages to summarize (all but last 4)
    let keep_tail = 4;
    if context.messages.len() <= keep_tail {
        return Ok(CompressionStats::no_op());
    }

    let to_summarize = &context.messages[..context.messages.len() - keep_tail];
    let summary_prompt = format!(
        "Summarize the following conversation history concisely, \
         preserving key decisions, facts, and context:\n\n{}",
        to_summarize.iter()
            .map(|m| format!("{}: {}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n")
    );

    // Call LLM for summary (use cheaper model if configured)
    let summary = provider.complete_simple(&summary_prompt, model).await?;

    // Replace summarized messages with a single system summary message
    let summary_msg = Message {
        role: Role::System,
        content: format!("[Conversation summary: {}]", summary),
    };

    context.messages.drain(..context.messages.len() - keep_tail);
    context.messages.insert(0, summary_msg);
    context.recalculate_tokens();

    Ok(CompressionStats {
        before_message_count: before_count,
        after_message_count: context.messages.len(),
        before_token_count: before_tokens,
        after_token_count: context.token_count,
    })
}
```

Add `compaction_model: Option<String>` to `AgentConfig` to route compaction through a cheaper model alias.

#### Acceptance Criteria

- [ ] `compact_with_llm()` added to `ContextCompressor`
- [ ] Summary injected as system message, not just omission marker
- [ ] Tool call pairs excluded from summarized range (to avoid orphaned tool results)
- [ ] `compaction_model: Option<String>` in `AgentConfig`
- [ ] Falls back to `Summarize` strategy if LLM call fails
- [ ] Triggered automatically when token count exceeds threshold

---

#### 8.7 Embedding Dedup Cache

**Priority:** Medium | **Complexity:** Low

**Current state:** Every call to `VectorMemoryService::store_memory()` re-embeds content via the API, even if the content hasn't changed since the last run.

**OpenClaw:** `embedding_cache` SQLite table keyed by `(provider, model, content_hash)` — unchanged content skips the embedding API call entirely.

**Proposed change:**

Add an `embedding_cache` table to `~/.manta/memory/memory.db`:

```sql
CREATE TABLE IF NOT EXISTS embedding_cache (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    content_hash TEXT NOT NULL,   -- SHA-256 of input text
    embedding BLOB NOT NULL,       -- serialized f32 little-endian
    created_at INTEGER NOT NULL,
    UNIQUE(provider, model, content_hash)
);
```

In `ApiEmbeddingProvider::embed()`, check the cache before calling the API:

```rust
async fn embed_with_cache(
    &self,
    text: &str,
    cache: &SqlitePool,
) -> Result<Vec<f32>> {
    use sha2::{Sha256, Digest};
    let hash = format!("{:x}", Sha256::digest(text.as_bytes()));

    // Cache hit?
    let row = sqlx::query_as::<_, (Vec<u8>,)>(
        "SELECT embedding FROM embedding_cache
         WHERE provider = ? AND model = ? AND content_hash = ?"
    )
    .bind(&self.provider_id)
    .bind(&self.model)
    .bind(&hash)
    .fetch_optional(cache)
    .await?;

    if let Some((blob,)) = row {
        return Ok(blob.chunks(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect());
    }

    // Cache miss — call API
    let embedding = self.embed(text).await?;

    // Store in cache
    let blob: Vec<u8> = embedding.iter()
        .flat_map(|f| f.to_le_bytes())
        .collect();
    sqlx::query(
        "INSERT OR IGNORE INTO embedding_cache
         (provider, model, content_hash, embedding, created_at)
         VALUES (?, ?, ?, ?, ?)"
    )
    .bind(&self.provider_id)
    .bind(&self.model)
    .bind(&hash)
    .bind(blob)
    .bind(chrono::Utc::now().timestamp())
    .execute(cache)
    .await?;

    Ok(embedding)
}
```

**Rust crates to consider:** `sha2` for content hashing.

#### Acceptance Criteria

- [ ] `embedding_cache` table added to memory DB
- [ ] SHA-256 content hash used as cache key
- [ ] Cache keyed by `(provider, model, content_hash)`
- [ ] Cache hit skips API call entirely
- [ ] Cache miss stores result for future use
- [ ] `cleanup_expired_cache(days: u32)` maintenance function added

---

#### 8.8 Split `MemoryTool` into `memory_search` + `memory_get`

**Priority:** Medium | **Complexity:** Low

**Current state:** `MemoryTool` in `src/tools/memory.rs` handles store/retrieve/search/list/delete/update in one tool. The `search` action returns full memory entries.

**OpenClaw:** Two separate tools:
- `memory_search` — returns path + line range + score + snippet (max 700 chars)
- `memory_get` — reads specific lines from a memory file (precision read after search)

This separation keeps context usage low: the agent first searches to find the relevant location, then reads only the needed lines.

**Proposed change:**

Keep `MemoryTool` as-is for CRUD operations. Add two new tools to the registry when the memory search backend is available:

```rust
pub struct MemorySearchTool {
    memory_service: Arc<VectorMemoryService>,
    session_search: Arc<SessionSearch>,
    config: HybridSearchConfig,  // from item 8.3
}

// Returns: [{path, start_line, end_line, score, snippet, citation}]
// snippet capped at 700 chars

pub struct MemoryGetTool {
    memory_service: Arc<VectorMemoryService>,
}

// Parameters: { path: String, from: Option<usize>, lines: Option<usize> }
// Reads raw lines from a memory file — used after memory_search
```

Add a `## Memory Recall` section to the system prompt (when these tools are registered) instructing the agent to call `memory_search` before answering questions about prior work, decisions, or preferences.

#### Acceptance Criteria

- [ ] `MemorySearchTool` registered when vector backend is available
- [ ] `MemoryGetTool` registered alongside `MemorySearchTool`
- [ ] Search results include `citation` field (`path#L5-L12` format)
- [ ] Snippets capped at 700 chars
- [ ] `## Memory Recall` instruction injected into system prompt when tools present
- [ ] Existing `MemoryTool` CRUD actions unaffected

---

#### 8.9 Subagent Personality Filtering

**Priority:** Medium | **Complexity:** Low

**Current state:** When spawning agents via `AgentRegistry`, `AgentPersonality::to_agent_config()` builds a full system prompt from all personality files (Bootstrap → Identity → Soul → Agents → Tools).

**OpenClaw:** Subagents and cron sessions receive only AGENTS.md, SOUL.md, TOOLS.md, IDENTITY.md, USER.md. BOOTSTRAP.md and HEARTBEAT.md are excluded (they contain startup-only instructions irrelevant to subagents).

**Proposed change:**

Add a `subagent` variant to the prompt builder:

```rust
pub enum PersonalityContext {
    /// Full prompt for the primary interactive session
    Primary,
    /// Reduced prompt for spawned subagents and cron jobs
    Subagent,
}

impl AgentPersonality {
    pub fn to_agent_config_for(&self, ctx: PersonalityContext) -> AgentConfig {
        let system_prompt = match ctx {
            PersonalityContext::Primary => self.build_system_prompt(),
            PersonalityContext::Subagent => self.build_subagent_prompt(),
        };
        AgentConfig { system_prompt, ..AgentConfig::default() }
    }

    fn build_subagent_prompt(&self) -> String {
        // Include: Identity, Soul, Agents, Tools
        // Exclude: Bootstrap (startup-only), User (not relevant for subagents)
        let mut sections = Vec::new();
        if !self.identity.is_empty() {
            sections.push(format!("## Identity\n{}\n", self.identity.trim()));
        }
        if !self.soul.is_empty() {
            sections.push(format!("## Soul\n{}\n", self.soul.trim()));
        }
        if !self.agents.is_empty() {
            sections.push(format!("## Agents\n{}\n", self.agents.trim()));
        }
        if !self.tools.is_empty() {
            sections.push(format!("## Tools\n{}\n", self.tools.trim()));
        }
        sections.join("\n")
    }
}
```

Use `PersonalityContext::Subagent` in `spawn_agent_from_personality()` in `src/gateway/mod.rs`.

#### Acceptance Criteria

- [ ] `PersonalityContext` enum with `Primary` and `Subagent` variants
- [ ] `build_subagent_prompt()` excludes Bootstrap and User files
- [ ] `spawn_agent_from_personality()` uses `PersonalityContext::Subagent`
- [ ] Primary sessions unaffected
- [ ] Tests for both prompt variants

---

#### 8.10 Temporal Decay for Dated Memory Files

**Priority:** Low | **Complexity:** Low

**Current state:** All memory search results are ranked purely by similarity score with no time-awareness.

**OpenClaw:** Optional exponential decay `score * e^(-λt)` with a configurable half-life (default 30 days). Date parsed from `memory/YYYY-MM-DD.md` filename. "Evergreen" files (MEMORY.md, non-dated `memory/*.md`) are exempt from decay.

**Proposed change:**

Add a `TemporalDecayConfig` and apply decay as a post-processing step on search results:

```rust
pub struct TemporalDecayConfig {
    pub enabled: bool,
    pub half_life_days: f32,   // default: 30.0
}

pub fn apply_temporal_decay(
    results: &mut Vec<HybridSearchResult>,
    config: &TemporalDecayConfig,
) {
    if !config.enabled {
        return;
    }

    let lambda = (2.0_f32.ln()) / config.half_life_days;
    let now = chrono::Utc::now();

    for result in results.iter_mut() {
        // Parse YYYY-MM-DD from path like "memory/2025-01-15.md"
        if let Some(date) = parse_date_from_path(&result.source) {
            let age_days = (now - date).num_days() as f32;
            let decay = (-lambda * age_days).exp();
            result.score *= decay;
        }
        // else: evergreen file, no decay applied
    }

    // Re-sort after decay
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
}
```

#### Acceptance Criteria

- [ ] `TemporalDecayConfig` with `enabled` flag and `half_life_days`
- [ ] Date parsed from `memory/YYYY-MM-DD.md` filename pattern
- [ ] Evergreen files (no parseable date) exempt from decay
- [ ] Disabled by default (`enabled: false`)
- [ ] Applied after hybrid score merge, before result truncation
- [ ] Config exposed in `manta.toml`

---

#### 8.11 Session Transcript Indexing in Semantic Search (Experimental)

**Priority:** Low | **Complexity:** Medium

**Current state:** `SessionSearch` FTS5 indexes session messages, but this is completely separate from `VectorMemoryService`. The agent cannot semantically search past conversations.

**OpenClaw:** `experimental.sessionMemory` — session JSONL transcripts are indexed as `source: "sessions"` chunks in the same SQLite DB as memory files, with delta-based re-indexing (only re-index if session file grew by 100KB or 50 messages).

**Proposed change:**

Add an optional `index_sessions` flag to `VectorMemoryService` config. When enabled:
1. On each session end, append new messages from `~/.manta/memory/*.db` chat history to the vector index.
2. Use delta-based indexing: track `last_indexed_message_id` per session and only embed new messages.
3. Mark these chunks with `source: "sessions"` in the vector store.
4. Include session chunks in `memory_search` results (alongside memory file chunks).

Disable by default. Enable via `memory.experimental.session_memory = true` in `manta.toml`.

#### Acceptance Criteria

- [ ] `memory.experimental.session_memory` config flag (default: false)
- [ ] New session messages delta-indexed into vector store
- [ ] `last_indexed_message_id` tracked per session to avoid re-embedding
- [ ] Session chunks included in `memory_search` results when enabled
- [ ] Results from sessions labeled `source: "sessions"` in citations
- [ ] No performance impact when disabled

---

#### 8.12 File Watching for Memory Files

**Priority:** Low | **Complexity:** Low

**Current state:** Changes to `~/.manta/workspace/` memory files require a Manta restart to take effect.

**OpenClaw:** `chokidar` watches the workspace directory for changes, debounces for 1.5s, then re-indexes changed files.

**Proposed change:**

Use the `notify` crate (already a common Rust dep) to watch `~/.manta/workspace/`:

```rust
use notify::{Watcher, RecursiveMode, Event};

pub async fn watch_memory_dir(
    memory_dir: PathBuf,
    cache: Arc<RwLock<HashMap<PathBuf, CachedFile>>>,
    debounce_ms: u64,
) {
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let mut watcher = notify::recommended_watcher(move |res: Result<Event, _>| {
        if let Ok(event) = res {
            let _ = tx.blocking_send(event);
        }
    }).expect("failed to create watcher");

    watcher.watch(&memory_dir, RecursiveMode::Recursive)
        .expect("failed to watch memory dir");

    let mut debounce_timer: Option<tokio::time::Sleep> = None;

    while let Some(event) = rx.recv().await {
        // Invalidate cache entries for changed paths
        let mut cache = cache.write().await;
        for path in event.paths {
            cache.remove(&path);
        }
        // Additional: trigger re-index if vector service is available
    }
}
```

Debounce with `debounce_ms = 1500` (configurable). Integrates with the workspace file cache from item 8.5.

**Rust crates to consider:** `notify` (already widely used in Rust ecosystem).

#### Acceptance Criteria

- [ ] `notify` watcher added for `~/.manta/workspace/` directory
- [ ] File changes invalidate the workspace file cache (item 8.5)
- [ ] 1500ms debounce before re-indexing (configurable)
- [ ] Watcher shuts down cleanly on gateway stop
- [ ] Memory file changes reflected in next turn without restart
- [ ] `memory.sync.watch: bool` config flag (default: true)

---

#### 8.13 System Prompt Memory Recall Mandate

**Priority:** Low | **Complexity:** Low

**Current state:** The agent has `MemoryTool` available but no instruction in the system prompt requiring it to search memory before answering questions about prior work.

**OpenClaw:** `buildMemorySection()` injects a `## Memory Recall` block into the system prompt when `memory_search`/`memory_get` tools are registered:

> "Before answering anything about prior work, decisions, dates, people, preferences, or todos: run memory_search on MEMORY.md + memory/*.md"

**Proposed change:**

In `AgentConfig::full_system_prompt_with_personality()` (`src/agent/mod.rs`), when `MemorySearchTool` is in the registry, append:

```
## Memory Recall
Before answering questions about prior work, past decisions, dates, people,
preferences, or todos: call memory_search to check MEMORY.md and memory/*.md.
Then use memory_get to read only the needed lines. If uncertain after searching,
say you checked but did not find a match.
```

This is a one-line change gated on tool availability.

#### Acceptance Criteria

- [ ] `## Memory Recall` section injected when `memory_search` tool is registered
- [ ] Section absent when memory tools are not available (no false instructions)
- [ ] Wording matches the enforced recall pattern
- [ ] Covered by prompt builder tests

---

#### 8.14 Per-Channel History Limits in Config

**Priority:** Low | **Complexity:** Low

**Current state:** No per-channel history configuration exists. All sessions use the same `max_turns` (item 8.4) or token-based pruning.

**OpenClaw:** `channels.<provider>.historyLimit` and `channels.<provider>.dmHistoryLimit` allow different history depths per channel type.

**Proposed change:**

Add optional history limit fields to each channel config struct:

```rust
// In each channel config (TelegramConfig, DiscordConfig, etc.)
pub struct TelegramConfig {
    // ... existing fields ...
    /// Max turns retained for group/channel sessions
    pub history_limit: Option<usize>,
    /// Max turns retained for direct message sessions
    pub dm_history_limit: Option<usize>,
}
```

TOML configuration:

```toml
[channels.telegram]
history_limit = 50      # group channels
dm_history_limit = 200  # DMs retain more history

[channels.discord]
history_limit = 30
dm_history_limit = 100
```

Resolved in the agent message processing path: use `dm_history_limit` when the channel metadata indicates a DM (`is_dm = true`), otherwise use `history_limit`. Falls back to the agent-level `max_turns`.

#### Acceptance Criteria

- [ ] `history_limit` and `dm_history_limit` added to all channel configs
- [ ] DM detection uses `is_dm` metadata flag set by each channel
- [ ] Falls back to agent-level `max_turns` when channel limit is unset
- [ ] TOML config documented with examples
- [ ] No behavior change when fields are absent

---

### 8. MCP System Improvements (OpenClaw Diff)

**Status:** Under consideration
**Priority:** See per-item priorities below
**Complexity:** Low–High per item

#### Background

A detailed diff of Manta's MCP implementation against OpenClaw's revealed that Manta has a more complete native MCP *client* than OpenClaw (which delegates entirely to external CLIs). However, Manta's MCP system has critical wiring gaps that prevent it from functioning end-to-end. OpenClaw's approach — a thin proxy/delegate — is not worth emulating, but specific ergonomic patterns from it inform recommendations.

**Current Manta MCP state summary:**
- Transport: stdio only, hand-rolled JSON-RPC 2.0 in `src/tools/mcp.rs`
- `Config` struct has no `mcp` field — config.example.yaml MCP section is never parsed
- `McpToolWrapper` is defined but never instantiated — discovered tools cannot be called by the LLM
- No auto-connect at startup, no CLI subcommand, no SSE/HTTP transport
- No process-exit detection, no reconnect logic
- No resources or prompts support

---

#### 9.1 Wire MCP Config into `Config` Struct and Auto-Connect at Startup

**Priority:** High | **Complexity:** Low

**Current state:** `config.example.yaml` documents an `mcp.servers` section but `Config` in `src/config.rs` has no `mcp` field. MCP servers defined in config are silently ignored. Servers can only be connected by the agent calling the `mcp` meta-tool at chat time.

**Proposed change:**

Add `McpServerConfig`, `McpSettings`, and `McpConfig` to `src/config.rs`:

```rust
#[derive(Debug, Clone, Deserialize, Default)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    pub working_dir: Option<PathBuf>,
    #[serde(default = "default_true")]
    pub reconnect: bool,
    #[serde(default = "default_max_reconnect")]
    pub max_reconnect_attempts: u32,  // default: 5
}

#[derive(Debug, Clone, Deserialize)]
pub struct McpSettings {
    #[serde(default = "default_max_tools")]
    pub max_tools: usize,        // default: 50
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,       // default: 30
    #[serde(default = "default_true")]
    pub auto_connect: bool,      // default: true
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: IndexMap<String, McpServerConfig>,
    #[serde(default)]
    pub settings: McpSettings,
}

pub struct Config {
    // ... existing fields ...
    #[serde(default)]
    pub mcp: McpConfig,
}
```

`manta.toml` configuration:

```toml
[mcp.settings]
max_tools = 50
timeout_secs = 30
auto_connect = true

[mcp.servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/home/user"]
reconnect = true

[mcp.servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_TOKEN = "$GITHUB_TOKEN" }  # resolved via secrets system

[mcp.servers.postgres]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-postgres", "postgresql://localhost/mydb"]
```

In `Gateway::start()`, after tool registry initialization, auto-connect all configured servers:

```rust
if self.config.mcp.settings.auto_connect {
    let mcp_tool = self.get_mcp_connection_tool();
    for (name, server_cfg) in &self.config.mcp.servers {
        let cfg = McpClientConfig::from(server_cfg);
        match mcp_tool.connect(name, cfg).await {
            Ok(_) => info!("✅ Auto-connected MCP server '{}'", name),
            Err(e) => warn!("Failed to auto-connect MCP server '{}': {}", name, e),
        }
    }
}
```

#### Acceptance Criteria

- [ ] `McpServerConfig`, `McpSettings`, `McpConfig` added to `src/config.rs`
- [ ] `Config::mcp` field added and deserialized from `manta.toml`
- [ ] `mcp.settings.timeout_secs` wired into `McpClient` request timeout
- [ ] `mcp.settings.max_tools` enforced when registering discovered tools
- [ ] `auto_connect = true` triggers connection on gateway start
- [ ] `auto_connect = false` leaves servers dormant until agent connects them
- [ ] Connection failures are logged as warnings, not hard errors
- [ ] Config documented in `config.example.toml`

---

#### 9.2 Auto-Register Discovered MCP Tools into `ToolRegistry`

**Priority:** High | **Complexity:** Medium

**Current state:** `McpToolWrapper` is defined in `src/tools/mcp.rs` but is **never instantiated**. After `connect` + `tools/list`, discovered tools are stored in `McpClient::tools` but cannot be invoked by the LLM as individual tools. The agent must awkwardly route calls through the meta-tool, which it often does incorrectly.

**Proposed change:**

Pass `Arc<RwLock<ToolRegistry>>` into `McpConnectionTool` at construction time:

```rust
pub struct McpConnectionTool {
    clients: Arc<RwLock<HashMap<String, McpClient>>>,
    tool_registry: Arc<RwLock<ToolRegistry>>,
}

impl McpConnectionTool {
    pub fn new(tool_registry: Arc<RwLock<ToolRegistry>>) -> Self { ... }
}
```

After a successful `connect`, register each discovered tool:

```rust
// In connect logic, after tools/list succeeds:
let discovered = client.get_tools();
let mut registry = self.tool_registry.write().await;

for tool_def in &discovered {
    // Namespace: mcp__{server_name}__{tool_name}
    let tool_name = format!("mcp__{}__{}", server_name, tool_def.name);
    let wrapper = McpToolWrapper {
        tool_name: tool_name.clone(),
        definition: tool_def.clone(),
        client: Arc::clone(&client_arc),
    };
    registry.register(tool_name, Box::new(wrapper));
}
info!("Registered {} MCP tools from server '{}'", discovered.len(), server_name);
```

On `disconnect`, deregister all tools from that server:

```rust
// In disconnect logic:
let mut registry = self.tool_registry.write().await;
let prefix = format!("mcp__{}", server_name);
registry.deregister_prefix(&prefix);
```

Add `deregister_prefix(prefix: &str)` to `ToolRegistry`.

**Naming convention:** `mcp__{server}__{tool}` (double underscore separator avoids conflicts with tool names containing single underscores).

#### Acceptance Criteria

- [ ] `McpConnectionTool` holds `Arc<RwLock<ToolRegistry>>`
- [ ] Each discovered tool wrapped in `McpToolWrapper` and registered on connect
- [ ] Tool names follow `mcp__{server}__{tool}` convention
- [ ] `max_tools` limit enforced (skip registration after limit reached)
- [ ] All `mcp__{server}__*` tools deregistered on disconnect
- [ ] `ToolRegistry::deregister_prefix()` added
- [ ] LLM can invoke MCP tools directly without routing through meta-tool
- [ ] Tool schema (JSON Schema from MCP `inputSchema`) correctly forwarded to LLM

---

#### 9.3 Make Request Timeout Configurable

**Priority:** High | **Complexity:** Low

**Current state:** `send_request()` in `McpClient` has a hardcoded 30-second timeout. `config.example.yaml` documents `timeout_seconds` but it is never parsed or used.

**Proposed change:**

Store timeout on `McpClient`:

```rust
pub struct McpClient {
    // ... existing fields ...
    pub timeout_secs: u64,
}
```

Set from `McpSettings::timeout_secs` on construction. Use in `send_request()`:

```rust
tokio::time::timeout(
    Duration::from_secs(self.timeout_secs),
    rx.recv(),
).await
.map_err(|_| MantaError::Timeout(format!(
    "MCP server '{}' did not respond within {}s",
    self.server_name, self.timeout_secs
)))?
```

Also add per-server override: `McpServerConfig::timeout_secs: Option<u64>` that takes precedence over `McpSettings::timeout_secs`.

#### Acceptance Criteria

- [ ] `McpClient::timeout_secs` field added
- [ ] Global timeout from `mcp.settings.timeout_secs`
- [ ] Per-server `timeout_secs` override in `McpServerConfig`
- [ ] Timeout error message includes server name and configured duration
- [ ] Default remains 30 seconds (backward compatible)

---

#### 9.4 Detect Process Exit and Auto-Reconnect

**Priority:** Medium | **Complexity:** Medium

**Current state:** `McpClient::is_connected()` only checks if the mpsc sender channel is alive. If the server process exits unexpectedly, `is_connected()` returns `true` until the next request times out after 30 seconds.

**Proposed change:**

Monitor the child process in a background task and notify on exit:

```rust
pub struct McpClient {
    // ... existing fields ...
    child_exited: Arc<AtomicBool>,
    reconnect_tx: Option<mpsc::Sender<String>>,  // server_name for reconnect queue
}

// In connect_stdio(), spawn a process watcher:
let child_exited = Arc::clone(&self.child_exited);
let server_name = self.server_name.clone();
let reconnect_tx = self.reconnect_tx.clone();
tokio::spawn(async move {
    let _ = child.wait().await;
    warn!("MCP server '{}' process exited", server_name);
    child_exited.store(true, Ordering::SeqCst);
    if let Some(tx) = reconnect_tx {
        let _ = tx.send(server_name).await;
    }
});
```

Update `is_connected()`:
```rust
pub fn is_connected(&self) -> bool {
    self.request_tx.is_some() && !self.child_exited.load(Ordering::SeqCst)
}
```

In `McpConnectionTool`, run a reconnect loop (when `McpServerConfig::reconnect = true`):

```rust
tokio::spawn(async move {
    while let Some(server_name) = reconnect_rx.recv().await {
        let mut attempts = 0;
        let max = config.max_reconnect_attempts;
        loop {
            attempts += 1;
            let delay = Duration::from_secs(5 * 2_u64.pow(attempts.min(5) - 1));
            warn!("Reconnecting MCP '{}' in {:?} (attempt {}/{})", server_name, delay, attempts, max);
            tokio::time::sleep(delay).await;
            match reconnect(&server_name).await {
                Ok(_) => { info!("✅ Reconnected MCP server '{}'", server_name); break; }
                Err(e) if attempts >= max => {
                    error!("MCP server '{}' failed after {} attempts: {}", server_name, max, e);
                    break;
                }
                Err(_) => continue,
            }
        }
    }
});
```

#### Acceptance Criteria

- [ ] `child_exited: Arc<AtomicBool>` flag on `McpClient`
- [ ] Background task watches child process and sets flag on exit
- [ ] `is_connected()` checks both channel and exit flag
- [ ] Reconnect queue notified on unexpected process exit
- [ ] Exponential backoff: 5s, 10s, 20s, 40s, 80s
- [ ] `reconnect = false` in config disables auto-reconnect
- [ ] `max_reconnect_attempts` cap (default 5) respected
- [ ] Reconnect re-registers tools in `ToolRegistry`

---

#### 9.5 Add `manta mcp` CLI Subcommand

**Priority:** Medium | **Complexity:** Low

**Current state:** No dedicated CLI subcommand. MCP servers can only be managed by the AI agent calling the `mcp` meta-tool at chat time. There is no way to introspect or test MCP connections from the shell.

**Proposed change:**

Add `Mcp` variant to the `Commands` enum in `src/cli/mod.rs`:

```rust
#[derive(Subcommand)]
pub enum McpCommands {
    /// List configured and connected MCP servers
    List,
    /// Connect a configured MCP server
    Connect { name: String },
    /// Disconnect an MCP server
    Disconnect { name: String },
    /// List tools from a connected MCP server
    Tools { name: String },
    /// Call an MCP tool directly
    Call {
        server: String,
        tool: String,
        #[arg(long, value_name = "JSON")]
        args: Option<String>,
    },
}
```

Add REST endpoints to the gateway:

```
GET  /api/v1/mcp/servers                    # list all servers + status
POST /api/v1/mcp/servers/:name/connect      # connect a configured server
POST /api/v1/mcp/servers/:name/disconnect   # disconnect
GET  /api/v1/mcp/servers/:name/tools        # list discovered tools
POST /api/v1/mcp/servers/:name/call         # invoke a tool { tool, args }
```

CLI usage:

```bash
manta mcp list
# NAME         STATUS     TOOLS
# filesystem   connected  5
# github       connected  12
# postgres     error      0

manta mcp tools filesystem
# read_file, write_file, list_directory, create_directory, delete_file

manta mcp call filesystem read_file --args '{"path": "/tmp/test.txt"}'
```

#### Acceptance Criteria

- [ ] `manta mcp list` shows all configured servers with connection status and tool count
- [ ] `manta mcp connect <name>` triggers connect for a configured server
- [ ] `manta mcp disconnect <name>` disconnects and deregisters tools
- [ ] `manta mcp tools <name>` lists discovered tools with descriptions
- [ ] `manta mcp call <server> <tool> [--args JSON]` invokes a tool and prints result
- [ ] All commands call gateway REST API (consistent with other CLI commands)
- [ ] REST endpoints added to gateway router
- [ ] Error output goes to stderr with non-zero exit code

---

#### 9.6 Add SSE Transport

**Priority:** Medium | **Complexity:** Medium

**Current state:** Only stdio transport is implemented. Remote/cloud-hosted MCP servers (which use HTTP+SSE or streamable-HTTP) cannot be connected.

**OpenClaw context:** OpenClaw explicitly advertises `sse: false` in its ACP bridge, meaning it does not support SSE either. However, the MCP ecosystem is increasingly adopting streamable-HTTP as the standard for remote servers.

**Proposed change:**

Add `connect_sse(url, headers)` to `McpClient` using `reqwest` (already a Manta dependency):

```rust
pub async fn connect_sse(
    &mut self,
    url: &str,
    headers: HashMap<String, String>,
) -> crate::Result<()> {
    // MCP SSE transport:
    // - GET {url} opens SSE stream for server->client messages
    // - POST {url}/message sends client->server JSON-RPC
    // Streamable-HTTP (newer):
    // - POST {url} with Accept: text/event-stream
    //   returns SSE stream in response body
    let client = reqwest::Client::new();
    // ... setup SSE reader + POST writer tasks similar to stdio
}
```

Add `transport` field to `McpServerConfig`:

```rust
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    #[default]
    Stdio,
    Sse { url: String, headers: HashMap<String, String> },
    StreamableHttp { url: String, headers: HashMap<String, String> },
}

pub struct McpServerConfig {
    // For stdio (existing):
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    // Transport selector:
    #[serde(default)]
    pub transport: McpTransport,
}
```

`manta.toml` for remote servers:

```toml
[mcp.servers.remote-tool]
transport = "sse"
url = "https://mcp.example.com/sse"
headers = { Authorization = "Bearer $API_TOKEN" }

[mcp.servers.cloud-search]
transport = "streamable_http"
url = "https://mcp.run/api/v1/servers/websearch/mcp"
headers = { Authorization = "Bearer $MCP_RUN_TOKEN" }
```

**Rust crates to consider:** `reqwest` (already a dep) with `stream` feature for SSE; `eventsource-client` for robust SSE handling.

#### Acceptance Criteria

- [ ] `McpTransport` enum: `Stdio`, `Sse`, `StreamableHttp`
- [ ] `connect_sse(url, headers)` implemented on `McpClient`
- [ ] SSE reader task deserializes server-sent JSON-RPC lines
- [ ] HTTP POST writer task sends client JSON-RPC messages
- [ ] `connect_streamable_http()` implemented (POST with SSE response)
- [ ] Auth headers passed through to HTTP requests
- [ ] Config: `transport`, `url`, `headers` fields in `McpServerConfig`
- [ ] `command` becomes optional (required only for stdio)
- [ ] Reconnect logic works for SSE (re-opens GET/POST on connection drop)

---

#### 9.7 Add `resources/list` and `resources/read` Support

**Priority:** Low | **Complexity:** Low

**Current state:** Only `initialize`, `tools/list`, and `tools/call` are implemented. MCP resources (file-like content exposed by servers) are not supported.

**Proposed change:**

Add methods to `McpClient`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    pub description: Option<String>,
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct McpResourceContent {
    pub uri: String,
    pub mime_type: Option<String>,
    pub text: Option<String>,
    pub blob: Option<String>,  // base64 encoded
}

impl McpClient {
    pub async fn list_resources(&self) -> crate::Result<Vec<McpResource>> {
        let resp = self.send_request("resources/list", json!({})).await?;
        Ok(serde_json::from_value(resp["result"]["resources"].clone())?)
    }

    pub async fn read_resource(&self, uri: &str) -> crate::Result<McpResourceContent> {
        let resp = self.send_request("resources/read", json!({ "uri": uri })).await?;
        Ok(serde_json::from_value(resp["result"]["contents"][0].clone())?)
    }
}
```

Register a `McpResourceTool` that agents can call:

```rust
// Tool name: mcp__{server}__resource_read
// Parameters: { uri: string }
// Returns: resource text content or base64 blob
```

Add `resources/list` call after `tools/list` in the connect flow.

#### Acceptance Criteria

- [ ] `McpResource` and `McpResourceContent` types defined
- [ ] `list_resources()` and `read_resource()` on `McpClient`
- [ ] `resources/list` called during connect flow alongside `tools/list`
- [ ] `McpResourceTool` registered per-server as `mcp__{server}__resource_read`
- [ ] Text and base64 blob resource types handled
- [ ] Resource list included in `manta mcp tools <name>` output

---

#### 9.8 Integrate MCP Env Vars with Secrets System

**Priority:** Low | **Complexity:** Low

**Current state:** `McpServerConfig::env` values are passed verbatim to the server subprocess. There is no resolution of `$VAR` references through Manta's secrets system (TODO item 1).

**Proposed change:**

In the connect flow, resolve env values through the secrets resolver before passing to `Command::envs()`:

```rust
// When building the Command for stdio transport:
let resolved_env: HashMap<String, String> = server_cfg.env
    .iter()
    .map(|(k, v)| {
        let resolved = secrets.resolve(v).unwrap_or_else(|_| v.clone());
        (k.clone(), resolved)
    })
    .collect();
cmd.envs(&resolved_env);
```

This means API tokens for MCP servers can be stored as secrets rather than hardcoded in `manta.toml`:

```toml
[mcp.servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_TOKEN = "$GITHUB_TOKEN" }  # resolved from env or secrets store
```

Depends on TODO item 1 (Advanced Secrets Management) for full benefit, but a simple `std::env::var()` fallback resolution is useful immediately.

#### Acceptance Criteria

- [ ] `$VAR` syntax in env values resolved via `std::env::var()` at connect time
- [ ] If secrets system (TODO 1) is available, resolution goes through it
- [ ] Unresolved references log a warning but do not block connection
- [ ] Resolved values are not logged (security)

---

#### 9.9 Expose Manta as an MCP Server

**Priority:** Low | **Complexity:** High

**Current state:** Manta is MCP client-only. External tools like Claude Desktop, Cursor, or other MCP clients cannot connect to Manta to use its built-in tools (shell, file ops, web search, memory, todo, etc.).

**Proposed change:**

Add an optional MCP server endpoint to the Axum gateway:

```toml
[mcp_server]
enabled = false
path = "/mcp"                          # streamable-HTTP endpoint
auth_token = ""                        # optional bearer token
allowed_tools = []                     # empty = all tools; list = allowlist
```

Implement a streamable-HTTP MCP server handler in `src/gateway/mcp_server.rs`:

```rust
// POST /mcp - accepts JSON-RPC, returns SSE stream
async fn mcp_server_handler(
    State(state): State<Arc<GatewayState>>,
    headers: HeaderMap,
    body: Json<Value>,
) -> impl IntoResponse {
    // Validate auth_token if configured
    // Dispatch JSON-RPC method:
    //   initialize -> server capabilities
    //   tools/list -> state.tool_registry tools
    //   tools/call -> state.tool_registry.execute(name, args)
}
```

Add to gateway router when `mcp_server.enabled = true`:
```rust
.route("/mcp", post(mcp_server_handler))
```

This allows Claude Desktop `claude_desktop_config.json`:
```json
{
  "mcpServers": {
    "manta": {
      "url": "http://localhost:8080/mcp",
      "headers": { "Authorization": "Bearer <token>" }
    }
  }
}
```

**Rust crates to consider:** The existing Axum + tokio-tungstenite stack is sufficient.

#### Acceptance Criteria

- [ ] `[mcp_server]` config section added
- [ ] `enabled = false` by default (opt-in)
- [ ] `POST /mcp` endpoint implements streamable-HTTP MCP transport
- [ ] `initialize` response advertises `tools` capability
- [ ] `tools/list` returns all (or allowlisted) tools from `ToolRegistry`
- [ ] `tools/call` executes tools and streams results
- [ ] Optional bearer token auth via `auth_token`
- [ ] `allowed_tools` list enforced for security
- [ ] Works with Claude Desktop `claude_desktop_config.json`
- [ ] Works with Cursor MCP config
