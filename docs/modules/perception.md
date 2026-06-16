# Perception Module

Unified perception fusion layer for Syscity — consolidates fragmented sensor
sources (screenshots, system monitoring, device sensors) into a common data
model and query interface. Lets the Agent ask "what's in the scene?" with a
single tool call instead of invoking multiple disparate tools.

## Current Status

**Prototype — functional for structured testing and mock-driven development.**
The core data model, source adapters, polling, scene graph, and tool
integration are in place. Production gaps (listed below) remain for real-time
streaming, persistent history, and multi-modal fusion.

## Architecture

```
Agent/LLM → "perception_query" tool → PerceptionQueryTool
                                              │
                                              ▼
                                       PerceptionRegistry
                                              │
                                  ┌───────────┼───────────┐
                                  ▼           ▼           ▼
                           Screenshot   SystemMonitor  DeviceSource
                           Adapter      Adapter        Adapter
                                  │           │           │
                                  ▼           ▼           ▼
                           ComputerAdapter  sysinfo    Capability
                           (existing)      (existing)  (existing)
```

### Key Design Property: Adapter-Based

Each existing sensor source gets a lightweight `PerceptionSource` adapter —
no changes needed to the source itself. This makes the perception layer
non-invasive: it wraps what's already there.

### Source Naming Convention

| Adapter | Source name (name()) | Modality |
|---------|---------------------|----------|
| `ScreenshotAdapter` | `"screenshot"` | `Rgb` |
| `SystemMonitorAdapter` | `"system_monitor"` | `System` |
| `DeviceSourceAdapter` | `"device_sensor"` (name()) / `"device:{id}:{cap}"` (observe()) | `Device` |

The `perception_query` tool is registered alongside device-specific tools
(`device_{driver}_{capability}`) — the LLM can choose between fine-grained
and fused access.

## Module Structure

```
src/perception/
├── mod.rs           — Modality enum, module declarations, public re-exports
├── observation.rs   — Observation, PerceptionSource trait, three Adapter impls
├── scene_graph.rs   — SceneGraph, Entity, EntityId, Relationship
├── aggregator.rs    — TemporalAggregator, AggregationStrategy
├── query.rs         — PerceptionQuery, QueryResult
├── registry.rs      — PerceptionRegistry (sources + scene graph + query)
└── mock.rs          — MockPerceptionSource (test utility)

src/tools/
└── perception_tool.rs — PerceptionQueryTool (Tool trait impl)

src/gateway/
└── mod.rs           — Inline perception init (~lines 1587-1650)
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
first-class dimension in the scene graph.

### PerceptionSource Trait

The boundary between Syscity and any sensor. Both poll-based (screenshot,
system monitor) and streaming (observable device capability) sources use
this trait.

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

A single datum from any sensor, with enough metadata to fuse into the
scene graph and filter by query.

```rust
pub struct Observation {
    pub id: ObservationId,           // UUID v4
    pub source: String,              // e.g. "screenshot", "device:sensor-01:temperature"
    pub modality: Modality,
    pub timestamp: Instant,
    pub confidence: f32,             // [0.0, 1.0]; 1.0 = ground truth
    pub spatial: Option<SpatialContext>,
    pub data: serde_json::Value,     // arbitrary payload
}
```

### Entity

A tracked entity in the scene graph, created/updated from observations.

```rust
pub struct Entity {
    pub id: EntityId,
    pub label: String,               // e.g. "Rgb", "Device"
    pub modality: Modality,
    pub first_seen: Instant,
    pub last_seen: Instant,
    pub confidence: f32,
    pub properties: HashMap<String, Value>,
    pub spatial: Option<SpatialContext>,
    pub relationships: Vec<Relationship>,
}
```

Entities are keyed by source name. Subsequent observations from the same
source update `last_seen`, `confidence`, and `properties`.

### SceneGraph

Aggregated world state built from ingested observations.

```rust
pub struct SceneGraph { /* entities: HashMap<EntityId, Entity> */ }

impl SceneGraph {
    pub fn ingest(&mut self, obs: Observation);  // create or update entity
    pub fn prune(&mut self, cutoff: Instant);    // remove stale entities
    pub fn entities(&self) -> Vec<&Entity>;       // all current entities
    pub fn get(&self, id: &EntityId) -> Option<&Entity>;
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

Central entry point — holds sources, scene graph, and aggregator.

```rust
pub struct PerceptionRegistry {
    sources: RwLock<HashMap<String, Arc<dyn PerceptionSource>>>,
    scene_graph: RwLock<SceneGraph>,
    aggregator: RwLock<TemporalAggregator>,
}

impl PerceptionRegistry {
    pub async fn register_source(&self, source: Arc<dyn PerceptionSource>);
    pub async fn poll_all(&self);                          // poll + ingest all sources
    pub async fn query(&self, q: &PerceptionQuery) -> QueryResult;
    pub async fn subscribe(&self, name: &str) -> Option<broadcast::Receiver<Observation>>;
    pub async fn list_sources(&self) -> Vec<String>;
}
```

## Three Adapter Implementations

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
    device_id: String,
    capability: Arc<dyn Capability>,
}
```

- `name()` → `"device_sensor"` (fixed &str for trait lifetime)
- `modality()` → `Modality::Device`
- `observe()` → calls `capability.execute(json!({}))`, wraps result
- Confidence: `1.0` on success, `0.0` on failure
- `subscribe()` — if capability implements `ObservableCapability`, bridges `DeviceEvent` → `Observation` via a new broadcast channel

Note: The `observe()` source name is `"device:{device_id}:{capability_name}"`,
while `name()` returns a fixed `"device_sensor"`. This means the entity
key in the scene graph uses the more specific format, but `list_sources()`
registers under the generic name.

## PerceptionQueryTool

Routable by the LLM through standard function calling. No approval required
(perception is read-only).

```rust
pub struct PerceptionQueryTool {
    registry: Arc<PerceptionRegistry>,
}
```

Parameters accepted by the tool:
- `modalities` — filter by sensor modalities (array of strings)
- `sources` — filter by source name
- `label_contains` — substring match on entity labels
- `min_confidence` — minimum confidence threshold
- `limit` — max entities to return

Execution flow:
1. `registry.poll_all().await` — poll all sources, ingest into scene graph
2. `registry.query(&query).await` — filter current scene graph
3. Return JSON `{ entities, sources }`

## Startup Integration

### Gateway::with_devices() (gateway/mod.rs ~1587-1650)

When `config.perception.enabled = true`:

```
Gateway::with_devices()
  │
  ├─ init_storage()
  ├─ init_devices(drivers)            → DeviceRegistry
  │
  └─ if config.perception.enabled:
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
       ├─ PerceptionQueryTool::new(reg)
       │     └─ tool_registry.register_dynamic()
       │
       └─ if poll_interval_secs > 0:
             tokio::spawn(poll_loop)
```

### Configuration

```rust
pub struct PerceptionConfig {
    pub enabled: bool,                // default false
    pub poll_interval_secs: u64,      // 0 = disable auto-poll
    pub aggregation_window_secs: u64, // default 5
}
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
  │     ├── for each obs: aggregator.push(obs)
  │     └── for each obs: scene_graph.ingest(obs)
  │
  ├── query(&PerceptionQuery)
  │     ├── scene_graph.entities() → filter by query → Vec<Entity>
  │     └── aggregator.observations() → filter by query → Vec<Observation>
  │
  └── return JSON { entities, sources }
```

### Streaming Path (optional, observable capabilities)

```
Device driver control loop
  │
  ├── tx.send(DeviceEvent { capability, data })
  │
  ▼
ObservableCapability::subscribe()
  │
  ▼
DeviceSourceAdapter::subscribe()
  │  bridges DeviceEvent → Observation
  │  spawns tokio task on broadcast channel
  ▼
Consumer (TUI, WebSocket, log)
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
```

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
| Unit | `perception/observation.rs` | Observation creation, id generation, spatial context |
| Unit | `perception/scene_graph.rs` | Ingest, update, prune, multi-modality |
| Unit | `perception/aggregator.rs` | All 4 strategies, empty window, prune |
| Unit | `perception/query.rs` | Modality/source/time/confidence filters, combinations |
| Unit | `perception/registry.rs` | Register, poll, query by modality/source |
| Unit | `perception/mock.rs` | MockPerceptionSource builder behaviour |
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

### 1. Source Name Inconsistency

`DeviceSourceAdapter::name()` returns the generic `"device_sensor"` while
`observe()` returns source-specific `"device:{id}:{cap}"`. This means
`list_sources()` shows the generic name but scene graph entities use the
specific name. This should be unified — either store the full name in a
field or compute it once on construction.

### 2. Persistent Observation History

Observations currently live only in the in-memory sliding window. There is
no persistence to the Gateway's storage backend. Agent restarts lose all
perception state.

### 3. Cross-Modality Fusion

No mechanism to correlate observations across modalities (e.g., linking a
device temperature reading to a system CPU reading that was taken at the
same time). The scene graph treats all entities as flat peers.

### 4. Spatial Reasoning

`SpatialContext` exists in the data model but no adapter populates it
currently. Spatial queries (e.g., "what's in the top-left quadrant?")
return empty results.

### 5. Background Poll Loop

The poll loop is a simple `tokio::spawn` with no backpressure, error
recovery, or adaptive rate limiting. A slow `observe()` call can delay
the entire poll cycle.

### 6. Real-Time Streaming

`DeviceSourceAdapter::subscribe()` bridges events but no consumer in the
system currently uses the streaming path. There's no mechanism for the
LLM to receive real-time observation pushes.

## Comparison: ROS (Robot Operating System)

| Concept | ROS | Syscity (current) | Syscity (future) |
|---------|-----|-------------------|------------------|
| Message type | ROS msg definition | `Observation { data: Value }` | Typed observations |
| Publisher | `ros::Publisher` | `PerceptionSource::observe()` | Streaming subscribe path |
| Subscriber | `ros::Subscriber` | `PerceptionQueryTool::execute()` | `subscribe()` on registry |
| Transform tree | `tf2` | None | SpatialContext + relationships |
| Bag recording | `rosbag` | None | Persistent store integration |
