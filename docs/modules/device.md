# Device Module

Physical device abstraction layer for Syscity — provides the types, traits,
and integration plumbing for representing and managing hardware devices
(motors, cameras, sensors, actuators, etc.) that an Agent can discover,
control, and monitor through standard LLM tool calling.

## Current Status

**Production-ready.** The core abstractions, real hardware drivers, runtime
extensibility (native plugins), and OS-level device discovery are all in
place. Devices work across config-driven startup, hot-reload, OS event-
driven discovery (udev on Linux), and third-party plugin loading.

## Architecture

```
Application layer — Gateway
    │
    ├─ DriverFactory (shared runtime registry, Arc<RwLock<..>>)
    │    ├─ register("mock",        MockDeviceDriver::from_config)     [always]
    │    ├─ register("serialport",  SerialPortDriver::from_config)     [cfg feature]
    │    ├─ register("hid",         HidDriver::from_config)            [cfg feature]
    │    ├─ register("gpio",        GpioDriver::from_config)           [target_os = "linux"]
    │    ├─ scan_native_plugins_dir("/path/to/plugins")                [cfg feature]
    │    └─ register_native_plugin("/path/to/plugin.so")              [cfg feature]
    │
    ├─ Config-driven discovery (device.drivers[].kind → factory.build)
    │
    ├─ OS Device Bridge (udev / IOKit / dev notify)
    │    │  listens for plug/unplug events
    │    │  matches via DeviceMatcher (AllOf, KernelDriver, UsbDevice, ...)
    │    │  builds driver via factory.build(kind, params)
    │    └  probes + connects + registers tools
    │
    ├─ init_devices()
    │    ├─ DeviceRegistry::register(driver)
    │    ├─ probe_all()        → detect present hardware
    │    ├─ connect(name)      → produce Device { capabilities: [...] }
    │    ├─ for each capability:
    │    │    DeviceToolWrapper::new(driver_name, cap)
    │    │    tool_registry.register_dynamic(Arc::new(wrapper))
    │    └─ spawn health check + hot-plug loops
    │
    └─ Agent (unaware of devices — dispatches via ToolRegistry)
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
├── mod.rs              — Device, DeviceInfo, DeviceStatus, re-exports
├── capability.rs       — Capability trait, CapabilityResult
├── driver.rs           — DeviceDriver trait, DeviceLifecycle trait
├── driver_factory.rs   — DriverFactory (shared runtime registry)
├── registry.rs         — DeviceRegistry (driver/device lifecycle)
├── safety.rs           — SafetyZone, SafetyRule, SafetyRuleKind
├── status_bus.rs       — DeviceStatusEvent broadcast bus
├── health.rs           — HealthCheckConfig + periodic health loop
├── hotplug.rs          — HotPlugConfig + periodic probe loop
├── mock.rs             — MockCapability, MockDeviceDriver (for testing)
├── serialport.rs       — SerialPortDriver (cfg: serialport)
├── hid.rs              — HidDriver (cfg: hidapi)
├── gpio.rs             — GpioDriver (cfg: target_os = "linux")
├── native_plugin.rs    — NativeDriverLoader (cfg: native-plugins)
└── os_bridge/
    ├── mod.rs           — DeviceMatcher, OsDeviceEvent, OsDeviceMonitor trait
    ├── bridge.rs        — spawn_os_bridge_loop event dispatch
    ├── linux.rs         — LinuxUdevMonitor (NETLINK_KOBJECT_UEVENT)
    ├── macos.rs         — MacOsDevMonitor (notify on /dev/)
    └── noop.rs          — NoopOsMonitor (fallback)

src/tools/
└── device_tool.rs      — DeviceToolWrapper (Capability → Tool bridge)

src/gateway/init/
└── devices.rs          — init_devices, discover_drivers_from_config,
                          spawn_os_bridge_from_config, reload_devices
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
    pub engaged: Arc<AtomicBool>,
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

## DriverFactory

`DriverFactory` is the central runtime registry for driver constructors.
It is a shared object behind `Arc<RwLock<..>>`, stored in
`GatewayState.infra.driver_factory`, and used by all driver instantiation
paths (config init, OS bridge, hot-reload, native plugins).

```rust
#[derive(Clone)]
pub struct DriverFactory {
    inner: Arc<RwLock<HashMap<String, DriverConstructor>>>,
}

pub type DriverConstructor =
    Arc<dyn Fn(Value) -> Result<Arc<dyn DeviceDriver>> + Send + Sync>;
```

### Built-in Drivers

Registered in `DriverFactory::new()`:

| Config `kind` | Driver | Feature flag | Platform |
|---|---|---|---|
| `"mock"` | MockDeviceDriver | always | all |
| `"serialport"` | SerialPortDriver | `serialport` | all |
| `"hid"` | HidDriver | `hidapi` | all |
| `"gpio"` | GpioDriver | — | `target_os = "linux"` |

### Methods

- `register(kind, ctor)` — register an `Arc<dyn Fn>` constructor
- `register_fn(kind, fn_ptr)` — register a bare function pointer
- `build(kind, params)` — construct a driver instance by kind
- `has_kind(kind)` — check if a driver kind is registered
- `kinds()` — list all registered kinds
- `register_native_plugin(path)` — load and register a driver from a `.so`
- `scan_native_plugins_dir(dir)` — scan directory for plugins

## Real Hardware Drivers

### SerialPortDriver

Communicates with serial devices (`/dev/ttyUSB0`, etc.) using the `serialport` crate.
Feature: `serialport` (included in default features).

**Config:**
```json
{
  "path": "/dev/ttyUSB0",
  "baud_rate": 115200,
  "data_bits": 8,
  "stop_bits": "1",
  "parity": "none",
  "name": "my-sensor"
}
```

**Capabilities:**
- `serial.read` — read bytes from the serial port (hex-encoded)
- `serial.write` — write hex-encoded bytes to the serial port

**Safety:** `serial.write` requires approval by default.

### HidDriver

Communicates with USB HID devices (joysticks, barcode scanners, etc.)
using the `hidapi` crate. Feature: `hidapi` (included in default features).

**Config:**
```json
{
  "vid": "0x1234",
  "pid": "0x5678",
  "serial": "A1B2C3",
  "usage_page": 1,
  "name": "barcode-scanner"
}
```

**Capabilities:**
- `hid.read` — read input report from HID device (hex-encoded)
- `hid.write` — write output report to HID device (hex-encoded)

**Safety:** `hid.write` requires approval by default.

### GpioDriver

Controls GPIO pins via Linux sysfs (`/sys/class/gpio`). No external crate
needed. `target_os = "linux"` only (no feature flag).

**Config:**
```json
{
  "pins": [17, 22, 27],
  "name": "relay-bank"
}
```

**Capabilities:**
- `gpio.read` — read digital value of a single pin or all pins
- `gpio.write` — write digital value to a pin
- `gpio.set_mode` — set pin direction to "in" or "out"

**Safety:** `gpio.write` requires approval by default.

## Native Plugin Loading

Third-party drivers can be loaded from shared libraries (`.so`, `.dylib`,
`.dll`) at runtime. Feature: `native-plugins`.

### Plugin C ABI

Each shared library must export three `extern "C"` functions:

| Function | Signature | Description |
|---|---|---|
| `syscity_driver_kind` | `() -> *const c_char` | Returns null-terminated UTF-8 driver kind string |
| `syscity_driver_create` | `(params: *const c_char) -> *mut c_void` | Allocates a driver from JSON params, returns opaque pointer |
| `syscity_driver_free` | `(ptr: *mut c_void)` | Deallocates a driver previously created |

### Double-Box Pattern

`Box<dyn DeviceDriver>` is a fat pointer (16 bytes: data + vtable), but
`*mut c_void` is thin (8 bytes). The plugin uses a double-box to cross
the FFI boundary:

```rust
// Plugin side:
let driver: Box<dyn DeviceDriver> = Box::new(MyDriver::new());
let double_box: Box<Box<dyn DeviceDriver>> = Box::new(driver);
Box::into_raw(double_box) as *mut c_void

// Host side (NativeDriverLoader):
let box_ptr: *mut Box<dyn DeviceDriver> = ptr as *mut Box<dyn DeviceDriver>;
let inner: Box<dyn DeviceDriver> = *Box::from_raw(box_ptr);
```

### Config

```toml
[device]
native_plugins_dir = "/usr/lib/syscity/plugins"
```

The directory is scanned at startup (and on hot-reload). All plugins must
use the same Rust compiler version as the host binary.

## OS Device Bridge

The OS bridge subscribes to host OS device plug/unplug events and
auto-discovers Syscity devices. On Linux it uses `NETLINK_KOBJECT_UEVENT`
(no external dependencies, just `libc`). On macOS it uses `notify` on
`/dev/`. On other platforms it is a no-op.

### DeviceMatcher

Events are matched against configurable matchers:

| Variant | Description | Example |
|---|---|---|
| `UsbDevice { vid, pid }` | Match by USB vendor/product ID | `{ vid: "2341", pid: "0043" }` |
| `Subsystem(name)` | Match by kernel subsystem | `"tty"`, `"hid"`, `"usb"` |
| `DevPattern(pattern)` | Match by devnode glob | `"/dev/ttyUSB*"` |
| `KernelDriver(name)` | Match by kernel driver name | `"ftdi_sio"`, `"usbhid"` |
| `AllOf(matchers)` | AND combination of matchers | See below |

**Example config:**
```toml
[device.os_bridge]
matchers = [
  { driver_kind = "serialport", params = { baud_rate = 115200 },
    matcher = { type = "AllOf", matchers = [
      { type = "Subsystem", 0 = "tty" },
      { type = "KernelDriver", 0 = "ftdi_sio" }
    ]}},
  { driver_kind = "hid",
    matcher = { type = "UsbDevice", vid = "2341", pid = "0043" }}
]
```

### Event Flow

```
OS (udev / IOKit / /dev/ notify)
   │  OsDeviceEvent { action, subsystem, devnode, properties }
   ▼
OsDeviceMonitor (trait, platform-specific)
   │
   ├── Added   → match DeviceMatcher → factory.build(kind, params)
   │              → probe → connect → register tools
   ├── Removed → disconnect by devnode → deregister tools
   └── Changed → re-probe / reconnect
```

### Property Mapping (Linux udev)

Properties from uevent `KEY=VALUE` pairs are lowercased and available
via `event.get(key)`:

| uevent field | Properties key | Used by |
|---|---|---|
| `ACTION` | — (parsed to `OsDeviceAction`) | all |
| `SUBSYSTEM` | — (parsed to `event.subsystem`) | `Subsystem` matcher |
| `DEVNAME` | — (parsed to `event.devnode`) | `DevPattern` matcher |
| `ID_VENDOR_ID` | `"id_vendor_id"` | `UsbDevice` matcher |
| `ID_MODEL_ID` | `"id_model_id"` | `UsbDevice` matcher |
| `DRIVER` | `"driver"` | `KernelDriver` matcher |
| `ID_DRIVER` | `"id_driver"` | `KernelDriver` matcher (fallback) |

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

### Config-Driven Discovery

Drivers can be specified in the configuration file:

```toml
[device]
enabled = true

[[device.drivers]]
kind = "serialport"
params = { path = "/dev/ttyUSB0", baud_rate = 115200 }

[[device.drivers]]
kind = "mock"
params = { name = "sim-sensor", present = true }

[device.os_bridge]
enabled = true

[device.os_bridge.matchers]
# ... matcher entries ...

[device.health_check]
interval_secs = 30

[device.hot_plug]
scan_interval_secs = 5
```

### Gateway::with_devices()

Explicit driver injection (for tests or programmatic use):

```rust
let drivers: Vec<Arc<dyn DeviceDriver>> = vec![
    Arc::new(SerialPortDriver::from_config(json!({ "path": "/dev/ttyUSB0" }))?),
];

let gateway = Gateway::with_devices(config, None, drivers).await?;

// Query device registry at runtime:
let registry = gateway.device_registry();
if let Some(reg) = registry {
    let devices = reg.list().await;
    let healthy = reg.health_check_all().await;
}
```

### Startup Sequence

1. `DriverFactory::new()` registers all built-in driver constructors
2. `discover_drivers_from_config(&factory, &config)` builds drivers from config entries
3. `factory.scan_native_plugins_dir(&dir)` loads external plugins (if configured)
4. `init_devices(config, drivers, &tool_registry)` probes and connects
5. `spawn_os_bridge_from_config(&factory, registry, &config, &tool_registry)` starts OS event listener

## Hot-Reload

The device subsystem supports hot-reload via `syscity reload` or the admin API:

1. Disconnects all devices
2. Aborts health check, hot-plug, and OS bridge loops
3. Deregisters all device tools from `ToolRegistry`
4. Re-scans native plugins directory
5. Re-runs config-driven discovery and init
6. Spawns new OS bridge loop

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
| Unit | `device/registry.rs` | Probe, connect, list, health check, disconnect, lock |
| Unit | `device/mock.rs` | MockCapability/MockDeviceDriver behaviour |
| Unit | `device/driver_factory.rs` | Build, register, clone sharing, kinds |
| Unit | `device/serialport.rs` | Config parsing, probe (absent) |
| Unit | `device/hid.rs` | Config parsing, VID/PID parsing, probe (absent) |
| Unit | `device/gpio.rs` | Config parsing, pin validation, probe (absent) |
| Unit | `device/native_plugin.rs` | Kind/created/free C ABI, scan directory, null free |
| Unit | `device/os_bridge/mod.rs` | AllOf, KernelDriver, UsbDevice matchers |
| Unit | `device/os_bridge/linux.rs` | Uevent parsing, DRIVER/ID_DRIVER fields |
| Unit | `device/os_bridge/bridge.rs` | Added/removed/changed event dispatch |
| Unit | `tools/device_tool.rs` | Name format, execute delegation, schema |
| Unit | `gateway/init/devices.rs` | Init flow, config-driven discovery |
| Integration | `tests/integrations/device_tests.rs` | Full lifecycle with mock drivers |
| E2E | `tests/e2e/device_tests.rs` | Gateway with mock provider + mock device |

## Gaps / Roadmap

The following sections identify remaining gaps and the confirmed
implementation path for each. ✓ marks items that are already implemented.

---

### ~~1. Hardware Auto-Discovery~~ ✓ **DONE**

**Solved by:** Config-file-driven discovery + OS device bridge + native
plugin loading. Drivers can be specified statically in config, loaded
from shared libraries at runtime, or auto-discovered via the OS bridge
when devices are plugged in.

---

### ~~2. Runtime Extensibility (Native Plugins)~~ ✓ **DONE**

**Solved by:** `libloading`-based plugin system with stable C ABI.
Third-party `.so`/`.dylib`/`.dll` files can register any `DeviceDriver`
by exporting `syscity_driver_*` functions.

---

### 3. Real-Time / Timing Constraints

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
   `JoinHandle`s.
2. **Syscity provides exactly one primitive:** `SafetyZone.engaged`
   as an `AtomicBool`.
3. **`Capability::execute()` is "set and forget"** — it writes the target
   and returns ASAP.
4. **Safety must bypass the LLM** — limit switches, E-stop, over-temp are
   handled entirely within the driver's interrupt task.

```rust
impl DeviceDriver for StepperMotor {
    async fn connect(&self) -> Result<Device> {
        let engaged = self.safety.engaged.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_micros(1000));
            loop {
                interval.tick().await;
                if engaged.load(Ordering::Acquire) { disable_output(); continue; }
                // PID calculation ...
            }
        });
        Ok(Device::connected(/* ... */))
    }
}
```

---

### ~~4. Streaming / Observable Data~~ ✓ **DONE**

**Solved by:** `ObservableCapability` trait + `broadcast::channel` for
continuous data streams, and `DeviceStatusEvent` bus for status changes.

---

### ~~5. Resource Locking / Conflict Resolution~~ ✓ **DONE**

**Solved by:** Per-device `try_lock()` in `DeviceRegistry` returning
`DeviceLock` that is released on drop.

---

### ~~6. Cross-Device Orchestration~~ ✓ **DONE**

**Solved by:** GoalPlanner ToolCall integration — device operations are
just tools and can be sequenced in DAGs.

---

### ~~7. Device Lifecycle (FW update, calibration, self-test)~~ ✓ **DONE**

**Solved by:** `DeviceLifecycle` trait with `self_test()`, `calibrate()`,
`update_firmware()`, `read_config()`, `write_config()`.

---

### 8. Human Approval Routing

**Problem:** `requires_approval: true` is a static flag with no runtime
effect.

**✓ Design decision: Wire approval through Gateway event system**

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

---

### 9. Device Groups & Hierarchy

**Problem:** No device composition (a robotic arm = multiple motors +
sensors). All devices are flat peers.

**✓ Design decision: Postponed — no immediate implementation**

Composite devices introduce significant complexity. When needed:

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

## Implementation Status

| Status | Item | Phase |
|---|---|---|
| ✓ DONE | Resource locking | Phase 1 |
| ✓ DONE | Config-driven discovery | Phase 1 |
| ✓ DONE | Shared DriverFactory (runtime extensible) | Phase 1 |
| ✓ DONE | Real hardware drivers (serialport, hid, gpio) | Phase 2 |
| ✓ DONE | ObservableCapability + status bus | Phase 2 |
| ✓ DONE | DeviceLifecycle trait | Phase 2 |
| ✓ DONE | Native plugin loading (.so/.dylib/.dll) | Phase 3 |
| ✓ DONE | OS device bridge (udev, IOKit) | Phase 4 |
| ✓ DONE | Advanced DeviceMatcher (AllOf, KernelDriver) | Phase 4 |
| ✓ DONE | Native plugins dir config + startup wiring | Phase 4 |
| ◐ PENDING | Human approval routing | — |
| ◐ PENDING | Device groups / hierarchy | — |
| ◐ PENDING | Real-time control loop patterns | — |

## Comparison: Linux Kernel Driver Model

| Concept | Linux Kernel | Syscity |
|---|---|---|
| Driver code | `.ko` module or built-in | Compiled into binary or `.so`/`.dylib`/`.dll` plugin |
| Hardware detection | `probe()` via device tree / PCI ID | `DeviceDriver::probe()` + OS bridge udev events |
| Device object | `struct device` | `Device { capabilities }` |
| User interface | `/dev/`, sysfs, ioctl | `DeviceToolWrapper` → `Tool` → LLM function calling |
| Driver registration | `module_init()` / `module_exit()` | `DriverFactory::register()` or plugin C ABI |
| Event notification | uevent netlink | `OsDeviceMonitor` → `broadcast::channel` |
| Power management | Runtime PM, system suspend | `DeviceLifecycle` trait (future) |
| Module auto-load | `udev` + `modprobe` | `OsBridgeConfig.matchers` → `DriverFactory::build()` |
