# Device Module

Physical device abstraction layer for Syscity — provides the types, traits,
and integration plumbing for representing and managing hardware devices
(motors, cameras, sensors, actuators, etc.) that an Agent can discover,
control, and monitor through standard LLM tool calling.

## Current Status

**Prototype — suitable for structured testing and mock-driven development.**
The core abstractions and LLM integration path are in place, but several
critical gaps (listed below) remain before this can be used with real
physical hardware in production.

## Architecture

```
Application layer
    │
    ▼
Gateway::with_devices(config, drivers)
    │
    ├─ init_devices(drivers, &tool_registry)
    │    │
    │    ├─ DeviceRegistry::register(driver)
    │    ├─ probe_all()        → detect present hardware
    │    ├─ connect(name)      → produce Device { capabilities: [...] }
    │    └─ for each capability:
    │         DeviceToolWrapper::new(driver_name, cap)
    │         tool_registry.register_dynamic(Arc::new(wrapper))
    │
    └─ (device_registry stored on Gateway for lifecycle management)
             │
             ▼
        Agent (unaware of devices — dispatches via ToolRegistry)
             │
             ▼
        LLM discovers "device_*" tools via standard function calling
```

### Key Design Property: Agent-Agnostic

The Agent has **zero awareness** of devices. Device operations are wrapped
as `Tool` objects (via `DeviceToolWrapper`) and registered in the standard
`ToolRegistry`. The LLM discovers and calls them through the same function-
calling pipeline as any other tool — no Agent changes needed.

### Tool Naming Convention

device operations appear as: `device_{driver_name}_{capability_name}`

| Driver name | Capability name | Tool name |
|---|---|---|
| `sensor-01` | `sensor.read_temperature` | `device_sensor-01_sensor_read_temperature` |
| `stepper` | `motor.move_to` | `device_stepper_motor_move_to` |
| `webcam` | `camera.capture` | `device_webcam_camera_capture` |

The `device_` prefix avoids collisions with built-in tools like `shell`,
`file_write`, `grep`, etc.

## Module Structure

```
src/device/
├── mod.rs           — Device, DeviceInfo, DeviceStatus
├── capability.rs    — Capability trait, CapabilityResult
├── driver.rs        — DeviceDriver trait
├── registry.rs      — DeviceRegistry (driver/device lifecycle)
├── safety.rs        — SafetyZone, SafetyRule, SafetyRuleKind
└── mock.rs          — MockCapability, MockDeviceDriver (for testing)

src/tools/
└── device_tool.rs   — DeviceToolWrapper (Capability → Tool bridge)

src/gateway/init/
└── devices.rs       — init_devices() — startup wiring
```

## Core Types

### DeviceDriver Trait

The boundary between Syscity and physical hardware. Each physical device
type implements this to provide probe, connect, and lifecycle management.

```rust
#[async_trait]
pub trait DeviceDriver: Send + Sync {
    fn driver_name(&self) -> &str;
    async fn probe(&self) -> Result<bool>;
    async fn connect(&self) -> Result<Device>;
    async fn disconnect(&self) -> Result<()> { Ok(()) }
    async fn health_check(&self) -> Result<bool> { Ok(true) }
}
```

- `probe()` — called at startup to detect if hardware is present
- `connect()` — builds the `Device` object with its capabilities and
  safety zone
- `disconnect()` — releases hardware resources (optional)
- `health_check()` — periodic liveness check (optional, defaults to OK)

### Capability Trait

A single device operation that the Agent can invoke. One per logical
operation (e.g. `motor.move_to`, `camera.capture`, `sensor.read`).

```rust
#[async_trait]
pub trait Capability: Send + Sync {
    fn name(&self) -> &str;
    fn param_schema(&self) -> Value;     // JSON Schema
    async fn execute(&self, params: Value) -> CapabilityResult;
    fn safety_rules(&self) -> Vec<SafetyRule> { vec![] }
}
```

### Device

Represents a connected physical device with its capabilities and safety zone.

```rust
pub struct Device {
    pub info: DeviceInfo,                     // id, model, firmware, location
    pub status: Arc<RwLock<DeviceStatus>>,    // lifecycle state
    pub capabilities: Vec<Arc<dyn Capability>>,
    pub safety_zone: Arc<RwLock<SafetyZone>>,
}
```

### DeviceStatus

```rust
pub enum DeviceStatus {
    Disconnected,
    Connected { since: u64 },
    Error { message: String, since: u64 },
    Degraded { message: String, since: u64 },
}
```

### SafetyZone

Per-device safety constraint enforcement. When tripped (e.g. by an
emergency-stop signal), all capability executions are rejected until
the zone is reset.

```rust
pub struct SafetyZone {
    pub rules: Vec<SafetyRule>,
    pub engaged: bool,
    pub last_triggered: Option<SystemTime>,
}
```

Rule kinds:
- `MaxVelocity(f64)` — motion speed limit
- `MaxForce(f64)` — force/torque limit
- `WorkspaceBoundary([f64; 6])` — spatial bounds `[x_min, x_max, y_min, y_max, z_min, z_max]`
- `RequiresApproval` — human must approve before execution
- `EmergencyStop` — triggers zone trip
- `Custom(String)` — application-defined rule

### DeviceRegistry

Manages the full lifecycle: driver registration → probe → connect →
health check → disconnect.

```
register(driver)  ──▶  Vec<Driver>
    │
probe_all()       ──▶  Vec<driver_name>  (only present hardware)
    │
connect(name)     ──▶  Arc<Device>  (stored internally)
    │
get(id) / list()  ──▶  access connected devices
    │
health_check(id)  ──▶  bool  (mark Degraded on failure)
    │
disconnect(id)    ──▶  remove + mark Disconnected
```

## Bridge: DeviceToolWrapper

`DeviceToolWrapper` implements the `Tool` trait by delegating to a
`Capability`. This is the key integration point — it makes device
operations look like any other tool to the Agent and LLM.

```rust
pub struct DeviceToolWrapper {
    device_id: String,
    name: String,              // "device_{id}_{cap_name}"
    description: String,
    capability: Arc<dyn Capability>,
}

impl Tool for DeviceToolWrapper {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> Value;    // delegates to capability
    async fn execute(&self, args, context) -> Result<ToolExecutionResult>;
    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities { requires_approval: true, ..default() }
    }
}
```

All device tools default to `requires_approval: true` because physical
hardware operations are inherently high-risk. Users can override this in
config for trusted devices.

## Startup Integration

### Gateway::with_devices()

```rust
let drivers: Vec<Arc<dyn DeviceDriver>> = vec![
    Arc::new(SerialMotorDriver { port: "/dev/ttyUSB0" }),
    Arc::new(UsbCameraDriver::new()),
];

let gateway = Gateway::with_devices(config, None, drivers).await?;

// Query device registry at runtime:
let registry = gateway.device_registry();
if let Some(reg) = registry {
    let devices = reg.list().await;
    let healthy = reg.health_check_all().await;
}
```

The `init_devices()` function performs the full startup sequence:
1. Register all drivers in a fresh `DeviceRegistry`
2. `probe_all()` — detect which hardware is physically present
3. `connect()` each present device
4. For each capability: wrap as `DeviceToolWrapper` and register in
   `ToolRegistry` via `register_dynamic()`

Only the tool registration path is used. There is no separate
CapabilityRegistry — the old one was removed as dead code.

## Testing

### Mock Infrastructure

`MockCapability` and `MockDeviceDriver` provide full control over probe
results, connect behaviour, and capability execution results — no real
hardware needed.

```rust
let cap = MockCapability::new("sensor.read_temperature")
    .with_result(json!({"celsius": 23.5}));

let driver = MockDeviceDriver::new("sensor-01", true)
    .with_capabilities(vec![Arc::new(cap)]);
```

### Test Coverage

| Level | File | What it tests |
|---|---|---|
| Unit | `device/mod.rs` | Device creation, status transitions, serialization |
| Unit | `device/safety.rs` | Trip/reset, allow/block |
| Unit | `device/registry.rs` | Probe, connect, list, health check, disconnect |
| Unit | `device/mock.rs` | MockCapability/MockDeviceDriver behaviour |
| Unit | `tools/device_tool.rs` | Name format, execute delegation, schema |
| Unit | `gateway/init/devices.rs` | Init flow with mock drivers |
| Integration | `tests/integrations/device_tests.rs` | Full lifecycle with mock drivers |
| E2E | `tests/e2e/device_tests.rs` | Gateway with mock provider + mock device |

## Gaps / Roadmap

The framework is structurally sound but stops at the prototype stage.
The following sections identify critical gaps and specify the confirmed
implementation path for each.  ✓ marks the chosen approach.

---

### 1. Hardware Auto-Discovery

**Problem:** Drivers must be manually passed as `Vec<Arc<dyn DeviceDriver>>`.
There is no mechanism to scan buses (USB, PCI, I2C, Bluetooth) at startup
and match discovered hardware to registered driver implementations.

**✓ Design decision: Config-file-driven discovery (phase 1), then `inventory` (phase 2)**

Rust cannot dynamically load unknown driver code at runtime, so "auto-
discovery" means **compile-time registration + runtime selection**.

**Phase 1 — Config-driven:**

A `device.toml` declares which driver modules to activate:

```toml
[[drivers]]
type = "serial-motor"
ports = ["/dev/ttyUSB0", "/dev/ttyUSB1"]
baud = 115200

[[drivers]]
type = "uvc-camera"
v4l_device = "/dev/video0"
```

Startup reads config, instantiates matching drivers, calls `probe()`:

```rust
pub async fn init_devices_from_config(
    config: &DeviceConfig,
    tool_registry: &ToolRegistry,
) -> crate::Result<DeviceInit> {
    let mut drivers: Vec<Arc<dyn DeviceDriver>> = Vec::new();
    // Match config entries to known driver constructors via typetag
    for entry in &config.drivers {
        if let Some(driver) = entry.instantiate() {
            drivers.push(driver);
        }
    }
    init_devices(drivers, tool_registry).await
}
```

**Phase 2 — `inventory` (future):**

Use the `inventory` crate to auto-collect driver factories. Driver authors
only need `#[device_driver]` on their `impl DeviceDriver` — no startup
code changes:

```rust
// Driver definition — no wiring needed
#[device_driver]
impl DeviceDriver for SerialMotorDriver { ... }

// Startup — auto-discovers all #[device_driver] types
let drivers: Vec<Arc<dyn DeviceDriver>> = auto_probe().await;
```

Phase 1 is chosen first because:
- Zero new dependencies (`typetag` + toml)
- Config is explicit and debuggable
- `inventory` has platform portability issues (WASM, embedded)
- Phase 2 can be layered later without breaking phase 1

---

### 2. Real-Time / Timing Constraints

**Problem:** All capability execution goes through the LLM tool-calling
pipeline, which introduces seconds of latency. This is unacceptable for
motor control loops, safety interlocks, and high-frequency sensor polling.

**✓ Design decision: Driver-spawned control loops + atomic SafetyZone sharing**

**Syscity provides the safety primitive; drivers implement the control loop.**

The two-tier model:

```
LLM (slow path, 1-5s)
  │  "motor.move_to(position: 180)"
  ▼
CapabilityRuntime
  ├─ check SafetyZone.engaged? → reject if tripped
  ├─ set target position (atomic write)
  └─ return immediately — do NOT block for physics

Driver control loop (fast path, μs-ms, tokio task)
  ├─ reads target from shared Atomic
  ├─ reads encoder/sensor
  ├─ computes error → PID output
  └─ writes PWM/GPIO

Safety interrupt (hardware-fast, μs, independent task)
  ├─ polls limit switch / estop at 1kHz
  ├─ trip SafetyZone via AtomicBool
  ├─ disable motor output directly
  └─ does NOT wait for LLM
```

**Implementation rules:**

1. **Control loop is owned by the driver** — `connect()` spawns its own
   `tokio::spawn()` loop. The `DeviceRegistry` does **not** manage
   `JoinHandle`s. This keeps the driver trait simple and matches hardware
   coupling (each driver knows its own timing needs).

2. **Syscity provides exactly one primitive:** `SafetyZone.engaged`
   as an `AtomicBool`. The slow path (LLM) reads it before each operation;
   the fast path (interrupt task) writes it. No real-time scheduler.

3. **`Capability::execute()` is "set and forget"** — it writes the target
   and returns ASAP. Physical completion is asynchronous and invisible to
   the LLM (except via a status/query capability).

4. **Safety must bypass the LLM** — limit switches, E-stop, over-temp are
   handled entirely within the driver's interrupt task. The LLM only learns
   about them when it tries the next operation and gets rejected.

```rust
// Driver's connect() spawns its own control loop
impl DeviceDriver for StepperMotor {
    async fn connect(&self) -> Result<Device> {
        let engaged = self.safety.engaged.clone();

        // Fast path: 1kHz PID loop (tokio task, driver-owned)
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_micros(1000));
            loop {
                interval.tick().await;
                if engaged.load(Ordering::Acquire) {
                    disable_output();
                    continue;
                }
                // ... PID calculation ...
            }
        });

        // Fast path: safety monitor (independent task)
        let estop = self.estop_pin.clone();
        let engaged2 = self.safety.engaged.clone();
        tokio::spawn(async move {
            loop {
                if estop.read().unwrap_or(false) {
                    engaged2.store(true, Ordering::Release);
                    disable_output();
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        });

        Ok(Device::connected(/* ... */))
    }
}
```

---

### 3. Streaming / Observable Data

**Problem:** `CapabilityResult` is a one-shot response. Sensors that
produce continuous data streams cannot be represented.

**✓ Design decision: Event bus + broadcast channel (not tool-calling path)**

```rust
#[async_trait]
pub trait ObservableCapability: Capability {
    /// Subscribe to a stream of data events from this capability.
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<DeviceEvent>;
}

/// A time-stamped data point from a device.
#[derive(Clone, Debug)]
pub struct DeviceEvent {
    pub device_id: String,
    pub capability: String,
    pub timestamp: u64,
    pub data: serde_json::Value,
}
```

Data flows through a `broadcast::channel` rather than the LLM tool path:

- **Producers** (driver control loops): `tx.send(DeviceEvent { ... })` at
  their natural cadence (e.g. temperature sensor every 5s, camera at 30fps)
- **Consumers**: TUI dashboard, WebSocket event stream, log aggregator.
  Each subscribes independently; slow consumers get dropped without
  blocking the producer (broadcast channel semantics)
- **LLM access**: A `device_read_stream` tool lets the LLM request the
  latest N events, but it does NOT receive the live feed

Rationale for broadcast channel (over database or event bus):
- No external dependency
- Built-in backpressure (lagging subscribers silently drop)
- Matches the existing Gateway event system

---

### 4. Resource Locking / Conflict Resolution

**Problem:** Two concurrent LLM tool calls could operate the same physical
device, causing race conditions or hardware damage.

**✓ Design decision: Per-device semaphore in DeviceRegistry**

```rust
impl DeviceRegistry {
    /// Acquire an exclusive lock on a device.
    ///
    /// Returns `None` if the device is already locked by another task.
    /// The lock is released when `DeviceLock` is dropped.
    pub async fn try_lock(&self, device_id: &str) -> Option<DeviceLock>;
}

#[must_use]
pub struct DeviceLock {
    key: Arc<tokio::sync::OwnedRwLockWriteGuard<()>>,
}
```

Granularity: **per-device** (coarse but simple). A device's capabilities
share the same lock — `motor.move_to` and `motor.stop` can't conflict
because they're on the same device.

The lock is held for the duration of `Capability::execute()`, which is
designed to be fast (set-target-and-return). Long-running operations
(like "record 30 seconds of video") should release the lock by returning
from execute and continuing in a background task.

---

### 5. Cross-Device Orchestration

**Problem:** Multi-step workflows (camera → detect → motor → gripper)
require the LLM to sequence individual tool calls with no atomicity or
rollback.

**✓ Design decision: Leverage existing GoalPlanner, not a new DSL**

The existing `GoalPlanner` already handles multi-step task decomposition
and DAG scheduling. Device operations are just tools — the planner can
sequence them naturally:

```rust
// The LLM sees device_* tools alongside everything else.
// GoalPlanner decomposes "pick and place object" into:
//   1. device_webcam_camera_capture
//   2. device_vision_detect_position (depends on 1)
//   3. device_stepper_motor_move_to (depends on 2)
//   4. device_gripper_gripper_close  (depends on 3)
```

No new workflow DSL is needed. The device module only needs to ensure
that its tools have clean `execute()` semantics (no side effects on
failure, idempotent where possible) so that the GoalPlanner's retry
logic works correctly.

---

### 6. Firmware & Calibration

**Problem:** No mechanism for firmware updates, self-test, calibration,
or device configuration.

**✓ Design decision: Optional `DeviceLifecycle` trait**

```rust
/// Optional lifecycle operations beyond basic probe/connect.
#[async_trait]
pub trait DeviceLifecycle: DeviceDriver {
    /// Run hardware self-test, return diagnostics.
    async fn self_test(&self) -> Result<Diagnostics>;

    /// Run calibration routine.
    async fn calibrate(&self) -> Result<()>;

    /// Flash firmware image to the device.
    async fn update_firmware(&self, image: &[u8]) -> Result<()>;

    /// Read current device configuration.
    async fn read_config(&self) -> Result<Value>;

    /// Write device configuration.
    async fn write_config(&self, config: Value) -> Result<()>;
}
```

This is a separate trait — not added to `DeviceDriver` — because most
drivers don't need it and it should not increase the barrier to entry
for simple devices. The `DeviceRegistry` can attempt a runtime
`Arc::downcast::<dyn DeviceLifecycle>` if it needs to expose these
operations through API endpoints.

---

### 7. Human Approval Routing

**Problem:** `requires_approval: true` is a static flag with no runtime
effect.

**✓ Design decision: Wire approval through Gateway event system**

When `DeviceToolWrapper.execute()` is called with `requires_approval`:

```
Agent calls device_tool.execute()
  │
  ├─ ToolRegistry checks requires_approval
  ├─ emit "tool.approval_required" event via Gateway event bus
  ├─ wait for approval (timeout configurable, e.g. 60s)
  │    ├─ approved  → proceed to Capability::execute()
  │    └─ rejected  → return "Rejected by user"
  │    └─ timeout   → return "Approval timed out"
  └─ emit "tool.result" event
```

Approval channels (future, not in initial implementation):
- TUI prompt (inline approve/deny)
- Telegram / notification callback
- REST endpoint (`POST /devices/{id}/approve`)

Initial implementation: The event is emitted but the tool proceeds
immediately. The `requires_approval` flag becomes an audit trail
rather than a gate until a concrete approval consumer is wired.

---

### 8. Device Groups & Hierarchy

**Problem:** No device composition (a robotic arm = multiple motors +
sensors). All devices are flat peers.

**✓ Design decision: Postponed — no immediate implementation**

Composite devices introduce significant complexity to discovery, locking,
safety zone aggregation, and tool naming. There is no concrete use case
yet. When needed, the design is:

```rust
pub enum DeviceNode {
    Leaf(Device),
    Composite {
        info: DeviceInfo,
        children: Vec<DeviceNode>,
    },
}
```

This is **parked** until a real composite device driver is implemented.

## Implementation Priority

| Priority | Item | Dependencies |
|----------|------|-------------|
| P0 | Resource locking (#4) | None — needed for safety even with mock hardware |
| P1 | Config-driven discovery (#1 phase 1) | `serde` + toml (already in tree) |
| P1 | Event bus streaming (#3) | broadcast channel |
| P2 | Human approval routing (#7) | Event system, approval UI |
| P3 | Fast-path docs (#2) | — (no code change, driver pattern docs) |
| P4 | DeviceLifecycle trait (#6) | — |
| P5 | Inventory auto-discovery (#1 phase 2) | `inventory` crate |
| P6 | Composite device hierarchy (#8) | — |
| P7 | Cross-device orchestration DSL (#5) | GoalPlanner integration |

## Comparison: Linux Kernel Driver Model

| Concept | Linux Kernel | Syscity (current) | Syscity (future) |
|---|---|---|---|
| Driver code | `.ko` module or built-in | Compiled into binary | Compiled into binary |
| Hardware detection | `probe()` via device tree / PCI ID | `DeviceDriver::probe()` | Config-driven + inventory auto-probe |
| Device object | `struct device` | `Device { capabilities }` | Hierarchical `DeviceNode` |
| User interface | `/dev/`, sysfs, ioctl | `DeviceToolWrapper` → `Tool` | Same + event stream |
| Interrupt handling | Hard IRQ / threaded IRQ | None | Driver-spawned safety monitor task |
| Power management | Runtime PM, system suspend | None | `DeviceLifecycle` trait |
| Device tree | DT bindings, overlays | None | Config-driven topology |
