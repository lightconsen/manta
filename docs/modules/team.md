# Team Module

Agent team management for organizing multiple agents into coordinated groups.

## Design

Provides team management for organizing multiple agents into coordinated groups with defined hierarchies and communication patterns.

- **`Team`** — Team configuration with members, hierarchy, and communication pattern
- **`TeamMember`** — Individual member with role, level, and capabilities
- **`TeamMeshManager`** — Mesh-based team coordination
- **`TeamMeshSession`** — Active mesh session with message routing

### Team Types

| Type | Structure |
|------|-----------|
| `Flat` | All agents are peers |
| `Hierarchical` | Managers and workers |
| `Network` | Agents connect as needed |

### Communication Patterns

| Pattern | Description |
|---------|-------------|
| `Broadcast` | All messages go to all agents |
| `Chain` | Messages flow through a chain |
| `Star` | Central coordinator distributes messages |
| `Mesh` | Agents communicate directly |

## Key Types

```rust
pub struct Team {
    pub name: String,
    pub description: Option<String>,
    pub team_type: TeamType,
    pub members: HashMap<String, TeamMember>,
    pub hierarchy: HashMap<String, Vec<String>>,
    pub communication: CommunicationPattern,
    pub shared_memory: Option<String>,
    pub active: bool,
    pub created_at: String,
    pub updated_at: String,
}

pub struct TeamMember {
    pub name: String,
    pub role: String,
    pub level: u8,
    pub can_delegate: bool,
    pub capabilities: Vec<String>,
    pub joined_at: String,
}

pub enum TeamType {
    Flat,
    Hierarchical,
    Network,
}

pub enum CommunicationPattern {
    Broadcast,
    Chain,
    Star,
    Mesh,
}

pub struct TeamMeshManager {
    sessions: HashMap<String, TeamMeshSession>,
}

pub struct TeamMeshSession {
    pub team: Team,
    pub message_log: Vec<TeamMessageResult>,
}
```

## Data Flow

```
Team Message
    │
    ▼
TeamMeshManager::route_message()
    │
    ├──▶ Broadcast → all members
    ├──▶ Chain → next in sequence
    ├──▶ Star → central coordinator
    └──▶ Mesh → direct member-to-member
```

## Implemented Features

- Team creation with configurable type and communication pattern
- Member management (add, remove, set role/level)
- Hierarchy structure with manager-worker relationships
- Delegation capability per member
- Mesh-based team coordination
- Message routing by communication pattern
- Team persistence with timestamps
- Integration with `team_communicate` tool

