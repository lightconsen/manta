# Manta Future Ideas

Long-term, high-complexity features that are under consideration but not actively planned.

---

### Multi-Node Coordination

**Status:** Under consideration
**Priority:** Low
**Complexity:** Very High

#### Description

Implement a distributed multi-node architecture allowing multiple devices (iOS, Android, macOS, other servers) to connect to a central Manta gateway. This enables remote skill execution, mobile app integration, and distributed workload processing.

#### Motivation

Currently, Manta runs as a single-node process. All skills execute locally:

```
User → Manta Gateway → Local Agent → Local Tools
```

This limits use cases:
- Cannot offload heavy tasks to a more powerful machine
- No mobile app integration (iOS/Android)
- Cannot access platform-specific tools from other devices
- No distributed computing capabilities

#### Use Cases

1. **Remote Execution**: "Transcode this video using my Mac Studio at home while I'm on my iPhone"
2. **Mobile Integration**: Use iPhone camera to capture and process images
3. **Skill Distribution**: Execute macOS-only tools on a Mac node from a Linux server
4. **Load Balancing**: Distribute agent workloads across multiple machines

#### Proposed Design

##### Phase 1: Node Registry and WebSocket Protocol

Implement a `NodeRegistry` to track connected nodes:

```rust
pub struct NodeRegistry {
    nodes: DashMap<String, NodeSession>,
}

pub struct NodeSession {
    pub node_id: String,
    pub platform: String,  // "ios", "android", "macos", "linux"
    pub capabilities: Vec<String>,
    pub commands: Vec<String>,  // Supported commands
    pub socket: WebSocket,
}
```

Nodes connect via WebSocket with authentication:

```rust
// Node authentication flow
1. Node sends pair.request with device info
2. Gateway shows approval UI / CLI prompt
3. User approves (node.pair.approve)
4. Node receives token for future connections
```

##### Phase 2: RPC Invocation System

Implement RPC to call commands on remote nodes:

```rust
impl NodeRegistry {
    pub async fn invoke(
        &self,
        node_id: &str,
        command: &str,
        params: serde_json::Value,
    ) -> Result<NodeInvokeResult>;
}
```

Example usage:

```rust
// Gateway calls remote node
let result = node_registry.invoke(
    "mac-studio-office",
    "system.run",
    json!({ "cmd": "ffmpeg", "args": ["-i", "input.mp4", "output.mp3"] }),
).await?;
```

##### Phase 3: Mobile Support (APNs)

For iOS/Android nodes that suspend in background:

```rust
pub async fn maybe_wake_node_with_apns(
    node_id: &str,
    force: bool,
) -> Result<WakeAttempt> {
    // Send Apple Push Notification to wake app
    // Wait for node to reconnect
    // Retry with exponential backoff
}
```

Features:
- Push notification wake for background apps
- Pending action queue (actions queued until app foregrounds)
- Graceful degradation when node is offline

##### Phase 4: Remote Skill Discovery

Probe remote nodes for available tools/binaries:

```rust
pub async fn refresh_remote_bins(&self) {
    for node in self.nodes.iter() {
        if node.supports_command("system.which") {
            let result = self.invoke(&node.id, "system.which",
                json!({ "bins": ["ffmpeg", "python3", "node"] })
            ).await;
            // Store available bins for routing decisions
        }
    }
}
```

##### Phase 5: Session Event Broadcasting

Distribute agent events to subscribed nodes:

```rust
pub enum GatewayEvent {
    AgentResponse {
        session_id: String,
        content: String,
        // ...
    },
}

// Subscribe nodes to sessions
node_subscriptions.subscribe(node_id, session_id);

// Broadcast to all subscribed nodes
for node_id in node_subscriptions.get_subscribers(session_id) {
    node_registry.send_event(node_id, "chat", payload);
}
```

#### Configuration

```toml
[nodes]
enabled = true

[nodes.pairing]
require_approval = true
auto_approve_local = true

[nodes.devices.mac-studio]
node_id = "mac-studio-office"
platform = "macos"
capabilities = ["heavy_compute", "video_processing"]

[nodes.push_notifications]
apns_enabled = true  # For iOS wake
fcm_enabled = true   # For Android wake
```

#### Implementation Notes

**Rust crates to consider:**
- `tokio-tungstenite` for WebSocket server
- `dashmap` for concurrent node registry
- `a2` or `apns2` for Apple Push Notifications
- `jsonwebtoken` for node authentication tokens

**Key design decisions:**
- Nodes are optional - Manta works standalone without nodes
- Secure pairing flow required (no automatic trust)
- Command allowlisting per node (security)
- Graceful handling of node disconnection
- Local execution preferred when possible

**Security considerations:**
- Nodes authenticate with signed tokens
- Commands are allowlisted per node
- Path traversal protection on file operations
- Rate limiting for node.invoke

#### Reference

See OpenClaw's implementation:
- `src/gateway/node-registry.ts` - Node registry management
- `src/gateway/server-methods/nodes.ts` - RPC handlers (1000+ lines)
- `src/gateway/server-node-subscriptions.ts` - Event subscription manager
- `src/gateway/server-mobile-nodes.ts` - iOS/Android detection
- `src/infra/skills-remote.ts` - Remote skill discovery

#### Acceptance Criteria

- [ ] Node WebSocket protocol implemented
- [ ] Pairing flow (request/approve/reject) working
- [ ] RPC invoke/result mechanism functional
- [ ] Node subscription manager for events
- [ ] Mobile wake support (APNs integration)
- [ ] Remote skill discovery (bin probing)
- [ ] Command allowlisting per node
- [ ] Documentation with setup examples
- [ ] Security audit passed
