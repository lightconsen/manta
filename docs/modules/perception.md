# Perception Module

Unified perception fusion layer for Syscity — consolidates fragmented sensor
sources (screenshots, system monitoring, device sensors, microphone) into a
common data model and query interface. Lets the Agent ask "what's in the scene?"
with a single tool call instead of invoking multiple disparate tools.

## Current Status

**Multi-modal sensor fusion — functional.**

The core data model, source adapters, polling, streaming, cross-modal fusion engine, and tool integration are in place. Production gaps (listed below) remain for persistent history and real-time consumer integration.

## Architecture

```
Agent/LLM → "perception_query" tool → PerceptionQueryTool
                                              │
                                              ▼
                                       PerceptionRegistry
                                              │
                                  ┌───────────┼───────────┬──────────┐
                                  ▼           ▼           ▼          ▼
                           Screenshot   SystemMonitor  Device     Microphone
                           Adapter      Adapter        Source     Adapter
                                                        Adapter
                                  │           │           │          │
                                  ▼           ▼           ▼          ▼
                           ComputerAdapter  sysinfo   Capability  AudioCapture
                           (existing)      (existing)  (existing)  (cpal)

                                              │
                                              ▼
                                       FusionEngine
                                          │
                                          ├── spatial grouping
                                          ├── temporal clustering
                                          ├── conflict resolution
                                          └── entity building → FusedEntity
```

### Key Design Property: Adapter-Based

Each existing sensor source gets a lightweight `PerceptionSource` adapter —
no changes needed to the source itself. This makes the perception layer
non-invasive: it wraps what's already there.

### Fusion Pipeline

```
fuse(&[Observation])
  │
  ├── 1. Filter by min_confidence
  │       Remove observations below config.min_confidence threshold
  │
  ├── 2. Temporal clustering
  │       Sort by timestamp, greedy single-pass: start new cluster,
  │       add subsequent observations within temporal_window_ms
  │
  ├── 3. Conflict resolution (per cluster)
  │       Per modality: pick highest confidence observation
  │       Tiebreak by most recent timestamp
  │
  └── 4. Entity building
        Merge properties from contributing observations
        Aggregate confidence (weighted average)
        Record contributing modality types and observation IDs
```

| Adapter | Source name (`name()`) | Modality |
|---------|------------------------|----------|
| `ScreenshotAdapter` | `"screenshot"` | `Rgb` |
| `SystemMonitorAdapter` | `"system_monitor"` | `System` |
| `DeviceSourceAdapter` | `"device:{id}:{cap}"` | `Device` |
| `MicrophoneAdapter` | `"audio:{source}"` | `Audio` |

The `perception_query` tool is registered alongside device-specific tools
(`device_{driver}_{capability}`) — the LLM can choose between fine-grained
and fused access.

## Module Structure

```
src/perception/
├── mod.rs                 — Modality enum, module declarations, public re-exports
├── observation.rs         — Observation, PerceptionSource trait, four Adapter impls
├── aggregator.rs          — TemporalAggregator, AggregationStrategy, Entity, EntityId
├── query.rs               — PerceptionQuery, QueryResult
├── registry.rs            — PerceptionRegistry (sources + aggregator + query)
├── fusion.rs              — FusionEngine, FusedEntity, FusionConfig (cross-modal fusion)
├── audio_adapter.rs       — MicrophoneAdapter, AudioAdapterConfig
├── template_summarizer.rs — TemplateSummarizer (rule-based, zero-LLM, default)
├── local_summarizer.rs    — LocalLlamaSummarizer (Qwen2.5 GGUF, needs `local-summarizer` feature)
├── llm_summarizer.rs      — LlmProviderSummarizer (uses agent's LLM provider)
└── mock.rs                — MockPerceptionSource (test utility)

src/tools/
└── perception_tool.rs — PerceptionQueryTool (Tool trait impl, with fusion support)

src/gateway/
└── mod.rs           — init_perception() helper (~lines 1279-1330)
```

## Core Types

### Modality

```rust
pub enum Modality {
    Rgb, Depth, Audio, Tactile,
    System, Device, UiTree,
    FileSystem, Network, Other,
}
```

Classifies the sensor modality. Used for filtering in queries and as a
first-class dimension in the fusion engine.

### PerceptionSource Trait

The boundary between Syscity and any sensor. Both poll-based (screenshot,
system monitor) and streaming (microphone, observable device capability)
sources use this trait.

```rust
#[async_trait]
pub trait PerceptionSource: Send + Sync {
    fn name(&self) -> &str;
    fn modality(&self) -> Modality;
    async fn observe(&self) -> Vec<Observation>;
    fn subscribe(&self) -> Option<broadcast::Receiver<Observation>> { None }
}
```

- `observe()` — poll once, return latest observation(s)
- `subscribe()` — optional streaming channel; returns `None` for poll-only sources

### Observation

A single datum from any sensor, with enough metadata to filter by query.

```rust
pub struct Observation {
    pub id: ObservationId,           // UUID v4
    pub source: String,              // e.g. "screenshot", "device:sensor-01:temperature"
    pub modality: Modality,
    pub timestamp: Instant,
    pub confidence: f32,             // [0.0, 1.0]; 1.0 = ground truth
    pub data: serde_json::Value,     // arbitrary payload
}
```

### Entity & EntityId

A tracked entity produced by the [`TemporalAggregator`], keyed by source name.

```rust
pub struct Entity {
    pub id: EntityId,                // stable identifier, keyed by source name
    pub label: String,               // e.g. "Rgb", "Device"
    pub modality: Modality,
    pub first_seen: Instant,
    pub last_seen: Instant,
    pub confidence: f32,
    pub properties: HashMap<String, Value>,
}
```

### AggregationStrategy

Controls how the sliding window of observations is collapsed into entities.

| Strategy | Behaviour |
|----------|-----------|
| `Latest` | Last observation per source wins (default) |
| `Majority` | Entity must appear in >50% of window observations |
| `CountThreshold(N)` | Entity must appear at least N times |
| `ConfidenceWeighted(T)` | Sum of confidences must reach threshold T |

### PerceptionQuery

All fields are optional — unset filters are ignored.

```rust
pub struct PerceptionQuery {
    pub modalities: Option<Vec<Modality>>,
    pub sources: Option<Vec<String>>,
    pub time_range: Option<Duration>,
    pub min_confidence: Option<f32>,
    pub label_contains: Option<String>,
    pub limit: Option<usize>,
}
```

### PerceptionRegistry

Central entry point — holds sources, aggregator, and routes queries.

```rust
pub struct PerceptionRegistry {
    sources: RwLock<HashMap<String, Arc<dyn PerceptionSource>>>,
    aggregator: RwLock<TemporalAggregator>,
}

impl PerceptionRegistry {
    pub async fn register_source(&self, source: Arc<dyn PerceptionSource>);
    pub async fn poll_all(&self);                          // poll + ingest all sources
    pub async fn query(&self, q: &PerceptionQuery) -> QueryResult;
    pub async fn subscribe(&self, name: &str) -> Option<broadcast::Receiver<Observation>>;
    pub async fn list_sources(&self) -> Vec<String>;
    pub async fn all_observations(&self) -> Vec<Observation>;  // for fusion
    pub async fn deregister_source(&self, name: &str);    // hotplug support
    pub async fn deregister_prefix(&self, prefix: &str);  // hotplug support
}
```

## Four Adapter Implementations

Each adapter is a `PerceptionSource` that wraps an existing subsystem.

### ScreenshotAdapter

```rust
pub struct ScreenshotAdapter {
    adapter: Arc<dyn ComputerAdapter>,
}
```

- `name()` → `"screenshot"`
- `modality()` → `Modality::Rgb`
- `observe()` → calls `adapter.screenshot(None)`, extracts dimensions + base64 length
- Confidence is always `1.0` (screenshot is ground truth)

### SystemMonitorAdapter

```rust
pub struct SystemMonitorAdapter {
    monitor: Arc<Mutex<SystemMonitor>>,
}
```

- `name()` → `"system_monitor"`
- `modality()` → `Modality::System`
- `observe()` → locks monitor, calls `get_status()`, wraps as Observation
- Confidence is always `1.0`

### DeviceSourceAdapter

```rust
pub struct DeviceSourceAdapter {
    source_name: String,   // "device:{device_id}:{capability_name}"
    device_id: String,
    capability: Arc<dyn Capability>,
}
```

- `name()` → `"device:{device_id}:{capability_name}"` (unique per device × capability)
- `modality()` → `Modality::Device`
- `observe()` → calls `capability.execute(json!({}))`, wraps result
- Confidence: `1.0` on success, `0.0` on failure
- `subscribe()` — if capability implements `ObservableCapability`, bridges `DeviceEvent` → `Observation` via a new broadcast channel

Note: Each device capability gets a unique source name (e.g.
`"device:sensor-01:temperature"`), avoiding the name collision that
previously caused multiple capabilities to overwrite each other in the
registry HashMap.

### MicrophoneAdapter

```rust
pub struct MicrophoneAdapter {
    source_name: String,       // "audio:microphone" or "audio:system_output"
    modality: Modality,
    config: AudioAdapterConfig,
    tx: broadcast::Sender<Observation>,
}
```

- `name()` → `"audio:{audio_source}"` (e.g. `"audio:microphone"`)
- `modality()` → `Modality::Audio`
- `observe()` → always returns empty (audio is inherently stream-only)
- `subscribe()` → spawns a background task that:
  1. Creates an `AudioCapture` (cpal) instance
  2. Receives `AudioSegment` frames via mpsc
  3. Analyzes each segment for `DetectedAudioEvent`s (Speech, Silence)
  4. Maps segment + events → `Observation` with `Modality::Audio`
  5. Sends via broadcast channel
- Confidence: `0.9` if events detected, `0.5` for silent segments
- On capture failure (no mic hardware), logs warning and exits silently

```rust
pub struct AudioAdapterConfig {
    pub audio_source: AudioSource,          // Microphone or SystemOutput
    pub sample_rate: u32,                   // default 16_000
    pub silence_threshold_db: f32,          // default -40.0
    pub channel_capacity: usize,            // default 256
}
```

## FusionEngine

Cross-modal fusion engine that correlates observations across modalities
(visual, audio, device) into unified `FusedEntity` objects.

```rust
pub struct FusionEngine {
    config: FusionConfig,
}

pub struct FusionConfig {
    pub temporal_window_ms: u64,        // default 500ms
    pub min_confidence: f32,            // default 0.3
}

pub struct FusedEntity {
    pub id: String,
    pub label: String,
    pub confidence: f32,
    pub modalities: Vec<Modality>,
    pub observation_ids: Vec<String>,
    pub properties: HashMap<String, Value>,
    pub correlation_key: String,
}
```

Stateless and trivially thread-safe — `fuse()` is a pure function on
`&[Observation]`.

### Fusion Pipeline

Already shown above in the Key Design Property section — see the Fusion Pipeline diagram.

## PerceptionQueryTool

Routable by the LLM through standard function calling. No approval required
(perception is read-only).

```rust
pub struct PerceptionQueryTool {
    registry: Arc<PerceptionRegistry>,
    fusion_engine: Option<FusionEngine>,  // optional cross-modal fusion
}

impl PerceptionQueryTool {
    pub fn new(registry: Arc<PerceptionRegistry>) -> Self;
    pub fn with_fusion(mut self, config: FusionConfig) -> Self;
}
```

Parameters accepted by the tool:
- `modalities` — filter by sensor modalities (array of strings)
- `sources` — filter by source name
- `label_contains` — substring match on entity labels
- `min_confidence` — minimum confidence threshold
- `limit` — max entities to return
- `enable_fusion` — when true, run cross-modal fusion and include `fused_entities`

Execution flow:
1. `registry.poll_all().await` — poll all sources, ingest into aggregator
2. `registry.query(&query).await` — filter current aggregated entities
3. If `enable_fusion` and `fusion_engine` configured: call `engine.fuse(&observations)`
4. Return JSON `{ entities, sources, fused_entities? }`

## Startup Integration

### init_perception() helper (gateway/mod.rs ~1279-1330)

When `config.perception.enabled = true`:

```
Gateway construction
  │
  ├─ init_storage()
  ├─ init_devices(drivers)            → DeviceRegistry
  │
  └─ init_perception(config, state, &mut background_tasks)
       │
       ├─ PerceptionRegistry::new(Latest, window_secs)
       │
       ├─ ScreenshotAdapter::new(computer_adapter)
       │     └─ register_source()
       │
       ├─ SystemMonitorAdapter::new(monitor)
       │     └─ register_source()
       │
       ├─ for each device in device_registry:
       │     for each capability in device.capabilities:
       │       DeviceSourceAdapter::new(device_id, cap)
       │       └─ register_source()
       │
       ├─ if config.perception.enable_microphone:
       │     MicrophoneAdapter::new(AudioAdapterConfig { ... })
       │     └─ register_source()
       │
       ├─ PerceptionQueryTool::new(reg)
       │     .with_fusion(FusionConfig::default())
       │     └─ tool_registry.register_dynamic()
       │
       ├─ if poll_interval_secs > 0:
       │     tokio::spawn(poll_loop)
       │     └─ background_tasks.push(handle)
       │
       ├─ store PerceptionInit { registry, poll_handle } on state
       │
       └─ return Some(registry)
```

### Configuration

```rust
/// Backend selection for the perception summary engine.
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SummarizerKind {
    #[default]
    Template,  // rule-based, zero-LLM (free)
    Local,     // Qwen2.5-1.5B GGUF via llama-cpp-2 (requires `local-summarizer` feature)
    Llm,       // existing LLM provider (billed like a normal model call)
}

pub struct PerceptionConfig {
    pub enabled: bool,                  // default false
    pub poll_interval_secs: u64,        // 0 = disable auto-poll
    pub scene_history: usize,           // default 1000
    pub aggregation_window_secs: u64,   // default 5
    pub audio_source: String,           // "microphone" or "system_output"
    pub audio_sample_rate: u32,         // default 16000
    pub silence_threshold_db: f32,      // default -40.0
    pub enable_microphone: bool,        // default false
    pub enable_summary: bool,           // default false — master switch for periodic summary
    pub summarizer_kind: SummarizerKind, // default Template
    pub summary_refresh_secs: Option<u64>, // default None (runtime default 60s)
}
```

Example `syscity.toml`:
```toml
[perception]
enabled = true
enable_microphone = true
audio_source = "microphone"
audio_sample_rate = 16000
silence_threshold_db = -40.0
poll_interval_secs = 5

# Summary section (off by default — no LLM tokens paid).
# enable_summary = true
# summarizer_kind = "template"   # "template" (default, free) / "local" / "llm"
# summary_refresh_secs = 60
```

## Data Flow

### Poll-and-Query Path (synchronous, on tool call)

```
LLM calls "perception_query"
  │
  ▼
PerceptionQueryTool::execute(args)
  │
  ├── poll_all()
  │     ├── for each source: source.observe() → Vec<Observation>
  │     └── for each obs: aggregator.push(obs)
  │
  ├── query(&PerceptionQuery)
  │     ├── aggregator.aggregate() → filter by query → Vec<Entity>
  │     └── aggregator.observations() → filter by query → Vec<Observation>
  │
  ├── if enable_fusion:
  │     engine.fuse(&all_observations) → Vec<FusedEntity>
  │     └── include fused_entities in output
  │
  └── return JSON { entities, sources, [fused_entities] }
```

### Streaming Path (microphone / observable capabilities)

```
AudioCapture / Device driver control loop
  │
  ├── tx.send(AudioSegment / DeviceEvent)
  │
  ▼
MicrophoneAdapter::subscribe() / DeviceSourceAdapter::subscribe()
  │  bridges raw data → Observation with Modality::Audio / Modality::Device
  │  spawns tokio task on broadcast channel
  ▼
Consumer (TUI, WebSocket, log)
```

### Fusion Path (cross-modal, on demand)

```
After poll_all() and query():
  │
  ▼
FusionEngine::fuse(&observations)
  │
  ├── min_confidence filter
  ├── temporal clustering (500ms window)
  ├── conflict resolution (highest confidence per modality)
  │
  └── foreach cluster → FusedEntity {
        modalities: [Rgb, Device],           // multiple contributing modalities
        confidence: 0.95,                     // weighted average
        observation_ids: [...],               // traceable back to sources
        correlation_key: "temporal",          // what correlated them
      }
```

### Background Poll Loop (optional, configurable interval)

```
If poll_interval_secs > 0:
  tokio::spawn(async {
      let mut ticker = time::interval(Duration::from_secs(interval));
      loop {
          ticker.tick().await;
          registry.poll_all().await;
      }
  })
  The handle is tracked in background_tasks for graceful shutdown.
```

## Streaming Pipeline & Agent Context Injection

Beyond the synchronous `perception_query` path, perception ships an
end-to-end streaming pipeline that lifts raw observations into a curated,
per-agent `Snapshot` and splices it directly into the agent's system
prompt as a `## Perception` section.

### Topology

```
[Poll Source]                   [Stream Source]
    │ observe()                       │ subscribe()
    │ (timeout-bounded by HealthTracker)
    └──────► PerceptionRegistry ◄─────┘
                    │
                    │ aggregator (sliding-window stats)
                    │ raw_hub.send(Observation)         ── broadcast::Sender<Observation>
                    │
                    ├──► TemporalProcessor ────┐
                    │     (debounce, throttle) │
                    │                          ├──► derived_hub
                    └──► FusionEngine ─────────┘    broadcast::Sender<Event>
                          (cross-modal binding)            │
                                                           │
                          HealthTracker transitions ───────┤
                          → Event::Anomaly { SourceFault } │
                                                           │
                                                           ▼
                                              ┌─────────────────────────┐
                                              │  Per-Agent              │
                                              │  MinimalAdapter         │
                                              │  ┌────────────────────┐ │
                                              │  │ AttentionGate      │ │ task-keyword filter
                                              │  │ SalienceFilter     │ │ severity / novelty
                                              │  │ Bounded queue (64) │ │
                                              │  │ Summary refresh    │ │ template / local / LLM (60s)
                                              │  └────────────────────┘ │
                                              └────────────┬────────────┘
                                                           │
                                              now() → Snapshot { entities,
                                                                 aggregates,
                                                                 recent_events,
                                                                 summary }
                                                           │
                                                           ▼
                                              Agent::build_fresh_context()
                                              → Snapshot::format_for_prompt(8)
                                              → "## Perception\n### Summary…"
```

### The Two Hubs

| Hub | Carries | Producers | Subscribers |
|---|---|---|---|
| `raw_hub` (`broadcast::Sender<Observation>`) | every raw observation, poll-mirrored or streamed | `PerceptionRegistry::poll_all` (mirrors poll results), `PerceptionSource::subscribe()` (forwards stream sources) | `TemporalProcessor`, `FusionEngine` |
| `derived_hub` (`broadcast::Sender<Event>`) | discrete cognitive events | `TemporalProcessor` (`Tick`, debounced `Discrete`), `FusionEngine` (`Bound`/`Unbound`), `PerceptionRegistry::emit_anomaly` (health transitions), stream sources that natively emit anomalies | per-agent `MinimalAdapter` |

The two-hub split keeps high-volume raw samples away from agent attention,
and lets the cognitive layer (TemporalProcessor + FusionEngine) be the
sole producer of agent-visible events for normal operation.

### Event Types on derived_hub

```rust
pub enum Event {
    Tick { source, modality, at },                  // routine pulse
    Discrete { source, modality, kind, payload },   // debounced state change
    Bound { entity: FusedEntity },                  // cross-modal binding
    Unbound { entity_id },                          // entity dropped
    Anomaly { source, reason: AnomalyKind, severity: u8 },
}
```

### Health-Driven Anomalies

`HealthTracker::record_success/record_timeout/record_error` return
`Option<HealthState>` — `Some(state)` only when the call **transitions**
the source's state. `PerceptionRegistry::poll_all` and `probe_source`
collect those transitions and emit one `Event::Anomaly` per transition:

| Transition | `severity` |
|---|---|
| `→ Healthy` (recovery) | 32 |
| `→ Degraded` | 128 |
| `→ Quarantined` | 220 |

Per-agent adapters surface these as `### Recent events` lines, so a slow
or quarantined source becomes immediately visible in the agent's prompt
without polling a `/health` endpoint.

### MinimalAdapter

One `MinimalAdapter` per agent. Subscribes to `derived_hub`, applies
attention + salience filters, holds a bounded recent-events queue
(default last 64), and serves cheap synchronous `Snapshot`s.

```rust
pub struct AdapterConfig {
    pub enable_summary: bool,                         // default false
    pub recent_events_capacity: usize,                // default 64
    pub summary_refresh_interval: Option<Duration>,   // default Some(60s) when enable_summary=true
    pub summary_window: Duration,                     // default 60s
    // …attention / salience tunables
}
```

Two consumption paths:

- **Streaming** — `next_event() -> Option<Event>` pulls one at a time and
  removes it from the queue. Used by event-driven agent loops.
- **Snapshot** — `now() -> Snapshot` mirrors the queue + aggregates
  without consuming. Idempotent; safe to call once per LLM turn.

### Snapshot

```rust
pub struct Snapshot {
    pub at: SystemTime,
    pub entities: Vec<FusedEntity>,
    pub aggregates: HashMap<(String, Modality), Aggregate>,
    pub recent_events: Vec<Event>,
    pub summary: Option<String>,   // LLM-cached narrative
}
```

`format_for_prompt(max_recent)` renders the snapshot as a Markdown block
with stable ordering. Returns `None` when **all four sections** are empty,
so the agent prompt never grows an empty `## Perception` heading.

```markdown
## Perception

### Summary
<one-sentence environment narrative — when summarizer configured>

### Sensors (current)
- cpu (System): {"mean": 12.3, "p95": 30.1}
- mic_level (Audio): {"rms": 0.04}

### Entities
- "Submit Button" ([Vision], conf=0.92)

### Recent events
- [SourceFault] camera0: Quarantined (sev=220)
- [Discrete] file_watcher: ./src/main.rs Modified
```

### Cached Summary (Three Backends)

The summary is **off by default** (`AdapterConfig.enable_summary = false`).
When enabled, `MinimalAdapter::new` spawns a background task that calls
the configured summarizer every `refresh_interval`:

```
enable_summary = true?
  │
  ├── build_summarizer(kind)
  │     ├── SummarizerKind::Template  → TemplateSummarizer (rule-based, free)
  │     ├── SummarizerKind::Local     → LocalLlamaSummarizer (Qwen2.5 GGUF)
  │     └── SummarizerKind::Llm       → LlmProviderSummarizer (existing LLM)
  │
  ├── tokio::time::interval(refresh_interval)
  │     │ tick (skip first; skip if pipeline idle — no recent events / aggregates)
  │     ▼
  │   PerceptionSummarizer::summarize(window = summary_window)
  │     │
  │     ▼
  │   inner.last_summary (Mutex<Option<String>>)
  │     │
  │     └──► next now() splices into Snapshot.summary
```

#### Backend Comparison

| Backend | Cost | Quality | Latency | Dependencies |
|---------|------|---------|---------|--------------|
| `Template` | Free | Deterministic, rule-based | < 1 ms | None (always available) |
| `Local` | Free (CPU) | Natural language, ~1.5B params | < 250 ms (M-series) | `local-summarizer` feature, ~1 GB download |
| `Llm` | LLM tokens | Full model capability | Network-bound | LLM provider configured |

#### Template Summarizer Output

The `TemplateSummarizer` parses the JSON structured input and emits ordered
clauses up to 200 characters:
1. **Anomalies** with severity ≥ 128 (e.g. `"camera0 quarantined (sev=220)"`)
2. **Top-3 changes** by absolute numeric delta (e.g. `"cpu 12→87"`)
3. **Top-3 aggregates** with mean (e.g. `"cpu avg 45.3"`)
4. Fallback: `"Environment nominal"` when nothing notable

#### Idle-Skip Guard

Even when `enable_summary = true`, the refresh task skips calling the
summarizer if there are no recent events **and** no aggregates in the
current window. This prevents wasteful no-op refreshes during quiet
periods. On-demand `adapter.summarize()` calls still write through to
the same cache, so manual calls benefit subsequent `now()`s.

#### When to enable

- **Default (`enable_summary = false`)**: recommended for capable LLMs
  that can read `### Recent events` and `### Sensors` directly. The
  summary section largely duplicates those blocks.
- **Enable (`enable_summary = true`)**: small / context-tight agents
  that benefit from a one-line environment narrative. Use `template`
  for zero cost, `local` for free natural language, or `llm` if you
  already have a provider configured.

### Per-Source Stream Routing

| Source kind | Path |
|---|---|
| **CPU sample** (poll) | `observe()` → registry → aggregator → `Aggregate` → `Snapshot.aggregates` → `### Sensors` |
| **Screen OCR** (poll) | `observe()` → raw_hub → `FusionEngine` → `Bound` event → adapter queue → `### Entities` + `### Recent events` |
| **File watcher** (stream) | `subscribe()` → raw_hub → `TemporalProcessor` (debounce) → `Discrete` event → adapter → `### Recent events` |
| **Microphone** (stream) | `subscribe()` → raw_hub → optional fusion → adapter → `### Entities` (if bound to vision) or `### Recent events` |
| **Any source timeout** | `HealthTracker` transition → registry `emit_anomaly` → derived_hub → adapter → `### Recent events` |
| **Environmental narrative** | adapter background task (60s) → summarizer LLM → cache → `### Summary` |

### Gateway Wiring

In `init_perception`:

```rust
let reg = Arc::new(PerceptionRegistry::new(...));
reg.set_raw_hub_sender(Some(raw_hub.sender())).await;
reg.set_derived_hub_sender(Some(perception_context.derived_hub().sender())).await;
```

Per-agent: `Gateway::spawn_agent_inner` constructs a `MinimalAdapter`
bound to the agent's task keywords, hands it a `derived_hub` receiver,
and stores it on the agent. `Agent::build_fresh_context` calls
`adapter.now().format_for_prompt(8)` and splices the result before the
agent's task block.

## Lifecycle Management

### Hotplug / Config Reload

When the admin API reloads device config or an OS bridge event arrives:

1. **Deregister old perception sources** via `deregister_prefix("device:")`
2. **Disconnect all devices** from the old DeviceInit
3. **Re-discover** drivers from new config
4. **Re-register sources** for each connected device
5. New `DeviceSourceAdapter`s are registered with unique names per capability

The `PerceptionInit` is stored in `GatewayState.perception_init` and
replaced atomically on reload.

## Testing

### Mock Infrastructure

`MockPerceptionSource` provides full control over source name, modality,
and observation data — no real sensors needed.

```rust
let src = MockPerceptionSource::new("e2e_test_sensor")
    .with_modality(Modality::Device)
    .with_data(serde_json::json!({"value": 42}));
```

### Test Coverage

| Level | File | What it tests |
|-------|------|---------------|
| Unit | `perception/observation.rs` | Observation creation, id generation, DeviceSourceAdapter unique names |
| Unit | `perception/aggregator.rs` | All 4 strategies, Entity/EntityId, empty window, prune |
| Unit | `perception/query.rs` | Modality/source/time/confidence filters, combinations |
| Unit | `perception/registry.rs` | Register, poll, query, deregister_source, deregister_prefix |
| Unit | `perception/mock.rs` | MockPerceptionSource builder behaviour |
| Unit | `perception/fusion.rs` | Temporal clustering, conflict resolution, min_confidence filter, empty input |
| Unit | `perception/audio_adapter.rs` | Name/modality, observe empty, subscribe receiver, segment-to-observation mapping (silence & speech), no-hardware-no-panic |
| Unit | `tools/perception_tool.rs` | Parse query, tool name, no-approval |
| Integration | `tests/integrations/perception_tests.rs` | Multi-source, modality filter, source filter, label filter, limit, tool execution |
| E2E | `tests/e2e/perception_tests.rs` | Gateway wiring: device sources registered, poll-and-query via registry, device+perception tools both registered |

### E2E Test Notes

Tests use in-memory storage (`storage_type = "memory"`) to avoid SQLite
global-state conflicts when multiple gateways exist in the same process.
They verify:

1. Perception registry exists and has device sources when perception is enabled
2. Poll-and-query returns entities via the registry API
3. Both device-specific tools and the `perception_query` tool are registered

## Gaps / Roadmap

### 1. Persistent Observation History

Observations currently live only in the in-memory sliding window. There is
no persistence to the Gateway's storage backend. Agent restarts lose all
perception state.

### 2. Background Poll Loop

The poll loop is a simple `tokio::spawn` with no backpressure, error
recovery, or adaptive rate limiting. A slow `observe()` call can delay
the entire poll cycle.

### 3. Real-Time Streaming Consumers — RESOLVED

Per-agent `MinimalAdapter`s now subscribe to `derived_hub` and inject
`Snapshot.recent_events` into the agent prompt's `### Recent events`
section every turn. See **Streaming Pipeline & Agent Context Injection**
above.

### 4. Adaptive Fusion Parameters

FusionConfig is static (set at startup). Future work could adapt the
temporal window and confidence thresholds dynamically based on sensor
noise characteristics or environmental conditions.

## Comparison: ROS (Robot Operating System)

| Concept | ROS | Syscity (current) | Syscity (future) |
|---------|-----|-------------------|------------------|
| Message type | ROS msg definition | `Observation { data: Value }` | Typed observations |
| Publisher | `ros::Publisher` | `PerceptionSource::observe()` | Streaming subscribe path |
| Subscriber | `ros::Subscriber` | `PerceptionQueryTool::execute()` | `subscribe()` on registry |
| Bag recording | `rosbag` | None | Persistent store integration |
| Fusion | None | `FusionEngine::fuse()` + `with_fusion()` | Adaptive fusion parameters |
