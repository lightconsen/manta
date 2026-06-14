# Canvas Module

A2UI (Agent-to-UI) dynamic UI generation system for Syscity.

## Design

Provides real-time dynamic user interface generation through WebSocket updates. Supports forms, buttons, progress indicators, and real-time content streaming.

- **`CanvasManager`** — Manages active UI sessions and component trees
- **`CanvasComponent`** — Enum of all supported UI component types
- **`CanvasUpdate`** — Delta update messages sent to clients
- **`CanvasEvent`** — User interaction events from the UI

### Component Types

| Component | Description |
|-----------|-------------|
| `Container` | Layout container with vertical/horizontal/grid layout |
| `Text` | Styled text display |
| `Markdown` | Markdown-rendered content |
| `Input` | Single-line text input |
| `Textarea` | Multi-line text input |
| `Button` | Clickable button with variants |
| `Select` | Dropdown selection |
| `Checkbox` | Boolean toggle |
| `RadioGroup` | Exclusive selection group |
| `Progress` | Progress bar with value/max |
| `Spinner` | Loading indicator |
| `Image` | Image display with alt text |
| `Code` | Syntax-highlighted code block |
| `Table` | Data table with headers and rows |
| `Divider` | Visual separator |
| `Alert` | Notification banner with levels |

## Key Types

```rust
pub struct CanvasId(pub String);

pub enum CanvasComponent {
    Container { id: String, children: Vec<CanvasComponent>, layout: Option<ContainerLayout> },
    Text { id: String, content: String, style: Option<TextStyle> },
    Markdown { id: String, content: String },
    Input { id: String, label: Option<String>, placeholder: Option<String>, value: Option<String>, input_type: Option<String>, required: Option<bool> },
    Button { id: String, label: String, variant: Option<String>, disabled: Option<bool> },
    Select { id: String, label: Option<String>, options: Vec<SelectOption>, value: Option<String> },
    Progress { id: String, value: f64, max: Option<f64>, label: Option<String> },
    Spinner { id: String, label: Option<String> },
    Image { id: String, src: String, alt: Option<String> },
    Code { id: String, content: String, language: Option<String> },
    Table { id: String, headers: Vec<String>, rows: Vec<Vec<String>> },
    Divider { id: String },
    Alert { id: String, level: String, message: String },
}

pub enum CanvasUpdate {
    Init { canvas_id: String, root: CanvasComponent },
    Update { component_id: String, component: CanvasComponent },
    Remove { component_id: String },
    Append { parent_id: String, component: CanvasComponent },
    Notify { level: String, message: String },
}

pub enum CanvasEvent {
    ButtonClick { component_id: String },
    InputChange { component_id: String, value: String },
    SelectChange { component_id: String, value: String },
    CheckboxChange { component_id: String, checked: bool },
    RadioChange { component_id: String, value: String },
    FormSubmit { component_id: String, values: HashMap<String, Value> },
    Close,
}
```

## Data Flow

```
Agent Output
    │
    ▼
CanvasManager::render()
    │
    ├──▶ CanvasUpdate::Init (full tree)
    ├──▶ CanvasUpdate::Update (single component)
    └──▶ CanvasUpdate::Append (add to container)
            │
            ▼
        WebSocket Broadcast
            │
            ▼
        Client UI Rendering
            │
            ▼
        CanvasEvent (user interaction)
            │
            ▼
        Agent Input Processing
```

## Implemented Features

- 16 UI component types with full serialization
- Delta update protocol (init, update, remove, append, notify)
- Bidirectional event handling (user interactions → agent)
- Container layouts (vertical, horizontal, grid)
- WebSocket-based real-time updates
- Canvas session management with unique IDs

